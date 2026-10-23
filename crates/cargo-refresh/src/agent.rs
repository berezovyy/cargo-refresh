//! One bounded request per connection. Ambiguous transport failures require restart.
use anyhow::{Context, Result, ensure};
use refresh_proto::wire::{self, Record};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub txid: u32,
    #[serde(default)]
    pub generation: u64,
    #[serde(flatten)]
    pub counters: serde_json::Map<String, serde_json::Value>,
}
impl Reply {
    pub fn detail(&self) -> &str {
        if self.message.is_empty() {
            &self.error
        } else {
            &self.message
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Applied,
    Retry,
    Intact,
    Restart,
}
pub fn action(reply: &Reply) -> Action {
    if reply.ok {
        Action::Applied
    } else {
        match reply.code.as_str() {
            "STACK_COLLISION" | "PARK_TIMEOUT" => Action::Retry,
            "ROLLED_BACK" => Action::Intact,
            _ => Action::Restart,
        }
    }
}
fn connect(path: &Path, deadline: Instant) -> Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    ensure!(
        !bytes.contains(&0) && bytes.len() < address.sun_path.len(),
        "invalid agent socket path"
    );
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    ensure!(
        raw >= 0,
        "create agent socket: {}",
        std::io::Error::last_os_error()
    );
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let length =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    loop {
        ensure!(Instant::now() < deadline, "agent connect timed out");
        let rc = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        };
        if rc == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EISCONN) => break,
            Some(libc::EINTR) => continue,
            // Unix sockets use EAGAIN for a full listener backlog. Retry the
            // nonblocking connect, never block in the kernel past our deadline.
            Some(libc::EAGAIN) | Some(libc::EINPROGRESS) | Some(libc::EALREADY) => {
                thread::sleep(Duration::from_millis(1))
            }
            _ => return Err(error).context("connect to application agent"),
        }
    }
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

pub struct Client {
    socket: PathBuf,
    legacy: bool,
    txid: u32,
}
impl Client {
    pub fn new(socket: impl AsRef<Path>) -> Self {
        Self {
            socket: socket.as_ref().to_owned(),
            legacy: false,
            txid: 0,
        }
    }
    fn request(&mut self, kind: u16, records: &[Record]) -> Result<Reply> {
        let body = wire::encode(records)?;
        ensure!(
            body.len() <= 268435456 - 16,
            "agent request exceeds protocol budget"
        );
        self.txid = self.txid.wrapping_add(1);
        let mut bytes = Vec::new();
        if self.legacy {
            if kind == 2 {
                bytes.extend(0xe099ca11_u32.to_le_bytes());
                bytes.extend(0_u32.to_le_bytes());
            } else {
                ensure!(kind == 0, "legacy agent has no PING");
                bytes = body;
            }
        } else {
            bytes.extend(b"RFR1");
            bytes.extend(1_u16.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend((records.len() as u32).to_le_bytes());
            bytes.extend(self.txid.to_le_bytes());
            bytes.extend((24_u64 + body.len() as u64 - 8).to_le_bytes());
            bytes.extend(&body[8..]);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = connect(&self.socket, deadline)?;
        let remaining = || {
            deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .context("agent request timed out")
        };
        let mut sent = 0;
        while sent < bytes.len() {
            stream.set_write_timeout(Some(remaining()?))?;
            let count = stream.write(&bytes[sent..]).context("send agent request")?;
            ensure!(count != 0, "agent disconnected while sending request");
            sent += count;
        }
        let mut line = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            stream.set_read_timeout(Some(remaining()?))?;
            let count = stream
                .read(&mut buffer)
                .context("read agent acknowledgement")?;
            ensure!(count != 0, "agent disconnected before acknowledgement");
            line.extend_from_slice(&buffer[..count]);
            if let Some(end) = line.iter().position(|b| *b == b'\n') {
                line.truncate(end + 1);
                break;
            }
            ensure!(line.len() <= 65536, "invalid agent reply length");
        }
        ensure!(line.len() <= 65536, "invalid agent reply length");
        let reply: Reply = serde_json::from_slice(&line).context("decode agent acknowledgement")?;
        if !self.legacy {
            ensure!(
                reply.txid == self.txid
                    || (!reply.ok
                        && reply.txid == 0
                        && (reply.code == "BAD_PROTOCOL" || reply.code.is_empty())),
                "agent transaction mismatch"
            );
        }
        Ok(reply)
    }
    /// Negotiation never sends an empty legacy APPLY (which advances generation).
    pub fn ping(&mut self) -> Result<Reply> {
        let reply = self.request(1, &[])?;
        if !reply.ok && (reply.code == "BAD_PROTOCOL" || reply.code.is_empty()) {
            self.legacy = true;
        }
        Ok(reply)
    }
    pub fn hook(&mut self) -> Result<Reply> {
        self.request(2, &[])
    }
    pub fn apply(&mut self, records: &[Record]) -> Result<Reply> {
        let first = self.request(0, records)?;
        if action(&first) == Action::Retry {
            thread::sleep(Duration::from_millis(50));
            self.request(0, records)
        } else {
            Ok(first)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    #[test]
    fn v1_header_retry_and_hook() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("a");
        let listener = UnixListener::bind(&socket).unwrap();
        let host = thread::spawn(move || {
            for (kind, code) in [(1, "OK"), (0, "STACK_COLLISION"), (0, "OK"), (2, "OK")] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut header = [0; 24];
                stream.read_exact(&mut header).unwrap();
                assert_eq!(&header[..4], b"RFR1");
                assert_eq!(u16::from_le_bytes(header[6..8].try_into().unwrap()), kind);
                let len = u64::from_le_bytes(header[16..].try_into().unwrap());
                assert_eq!(len, 24);
                let txid = u32::from_le_bytes(header[12..16].try_into().unwrap());
                writeln!(
                    stream,
                    "{}",
                    serde_json::json!({"ok":code=="OK","code":code,"txid":txid})
                )
                .unwrap();
            }
        });
        let mut client = Client::new(socket);
        assert!(client.ping().unwrap().ok);
        assert!(client.apply(&[]).unwrap().ok);
        assert!(client.hook().unwrap().ok);
        host.join().unwrap();
    }
    #[test]
    fn legacy_negotiation_does_not_apply_an_empty_patch() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("a");
        let listener = UnixListener::bind(&socket).unwrap();
        let host = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut header = [0; 24];
            stream.read_exact(&mut header).unwrap();
            assert_eq!(&header[..4], b"RFR1");
            writeln!(
                stream,
                "{}",
                serde_json::json!({"ok":false,"error":"bad magic"})
            )
            .unwrap();
            drop(stream);
            let (mut stream, _) = listener.accept().unwrap();
            let mut header = [0; 8];
            stream.read_exact(&mut header).unwrap();
            assert_eq!(
                u32::from_le_bytes(header[..4].try_into().unwrap()),
                wire::MAGIC
            );
            assert_eq!(u32::from_le_bytes(header[4..].try_into().unwrap()), 0);
            writeln!(stream, "{}", serde_json::json!({"ok":true})).unwrap();
        });
        let mut client = Client::new(socket);
        assert!(!client.ping().unwrap().ok);
        assert!(client.apply(&[]).unwrap().ok);
        host.join().unwrap();
    }
    #[test]
    fn full_listener_backlog_respects_the_connect_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("a");
        let listener = UnixListener::bind(&socket).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let _queued = UnixStream::connect(&socket).unwrap();
        let error = connect(&socket, Instant::now() + Duration::from_millis(20)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }
    #[test]
    fn rolled_back_preserves_app_and_unknown_fails_closed() {
        let mut reply: Reply =
            serde_json::from_value(serde_json::json!({"ok":false,"code":"ROLLED_BACK"})).unwrap();
        assert_eq!(action(&reply), Action::Intact);
        reply.code = "FUTURE_CODE".into();
        assert_eq!(action(&reply), Action::Restart);
    }
}
