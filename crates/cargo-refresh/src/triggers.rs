//! Optional no-code notifications after an acknowledged live patch.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{FileTimes, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    os::unix::fs::FileExt,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

#[derive(Debug)]
pub struct Http {
    authority: String,
    host: String,
    port: u16,
    path: String,
}
impl Http {
    pub fn parse(url: &str) -> Result<Self> {
        ensure!(
            url.len() <= 8192
                && url.is_ascii()
                && !url.bytes().any(|b| b.is_ascii_control() || b == b' '),
            "invalid after_patch_http URL"
        );
        let rest = url
            .strip_prefix("http://")
            .context("after_patch_http requires http://")?;
        ensure!(
            !rest.contains('#'),
            "after_patch_http does not support fragments or credentials"
        );
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..end];
        ensure!(
            !authority.contains('@'),
            "after_patch_http does not support credentials"
        );
        let suffix = &rest[end..];
        let path = if suffix.starts_with('/') {
            suffix.to_owned()
        } else {
            format!("/{suffix}")
        };
        let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
            let end = ipv6.find(']').context("invalid HTTP IPv6 host")?;
            let host = &ipv6[..end];
            host.parse::<std::net::Ipv6Addr>()
                .context("invalid HTTP IPv6 host")?;
            let suffix = &ipv6[end + 1..];
            (
                host,
                if suffix.is_empty() {
                    80
                } else {
                    suffix
                        .strip_prefix(':')
                        .context("invalid HTTP authority")?
                        .parse()?
                },
            )
        } else {
            let (host, port) = authority
                .split_once(':')
                .map_or(Ok((authority, 80)), |(h, p)| {
                    p.parse::<u16>().map(|p| (h, p))
                })?;
            ensure!(
                !host.is_empty()
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')),
                "invalid HTTP hostname"
            );
            (host, port)
        };
        ensure!(port != 0, "invalid HTTP port");
        Ok(Self {
            authority: authority.into(),
            host: host.into(),
            port,
            path,
        })
    }
    fn resolve(self) -> Result<Endpoint> {
        let addresses: Vec<_> = (self.host.as_str(), self.port).to_socket_addrs()?.collect();
        ensure!(
            !addresses.is_empty(),
            "after_patch_http host has no addresses"
        );
        Ok(Endpoint {
            addresses,
            request: format!(
                "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
                self.path, self.authority
            ),
        })
    }
}
struct Endpoint {
    addresses: Vec<SocketAddr>,
    request: String,
}
impl Endpoint {
    fn get(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let remaining = || -> Result<Duration> {
            let left = deadline.saturating_duration_since(Instant::now());
            ensure!(!left.is_zero(), "after_patch_http timed out");
            Ok(left)
        };
        let mut connected = None;
        let mut last = None;
        for address in &self.addresses {
            match TcpStream::connect_timeout(address, remaining()?) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(error) => last = Some(error),
            }
        }
        let mut stream =
            connected.with_context(|| format!("after_patch_http connect failed: {last:?}"))?;
        let mut request = self.request.as_bytes();
        while !request.is_empty() {
            stream.set_write_timeout(Some(remaining()?))?;
            let n = stream.write(request)?;
            ensure!(n != 0, "after_patch_http write returned zero");
            request = &request[n..];
        }
        let mut header = Vec::new();
        loop {
            stream.set_read_timeout(Some(remaining()?))?;
            let mut bytes = [0; 1024];
            let n = stream.read(&mut bytes)?;
            ensure!(n != 0, "after_patch_http closed before response headers");
            header.extend_from_slice(&bytes[..n]);
            ensure!(
                header.len() <= 16384,
                "after_patch_http headers exceed 16 KiB"
            );
            if header.windows(4).any(|b| b == b"\r\n\r\n") {
                break;
            }
        }
        let line = header
            .split(|b| *b == b'\n')
            .next()
            .context("missing HTTP status")?;
        let line = std::str::from_utf8(line)?;
        let mut fields = line.split_ascii_whitespace();
        ensure!(
            matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")),
            "invalid HTTP response"
        );
        let status: u16 = fields.next().context("missing HTTP status")?.parse()?;
        if !(200..300).contains(&status) {
            bail!("after_patch_http returned {status}");
        }
        Ok(())
    }
}

pub struct Triggers {
    touch: Vec<PathBuf>,
    http: Option<Endpoint>,
}
impl Triggers {
    pub fn new(touch: Vec<PathBuf>, http: Option<&str>) -> Result<Self> {
        // Resolve DNS once during setup/configuration, never on the per-save path.
        Ok(Self {
            touch,
            http: http
                .map(Http::parse)
                .transpose()?
                .map(Http::resolve)
                .transpose()?,
        })
    }
    pub fn configured(&self) -> bool {
        !self.touch.is_empty() || self.http.is_some()
    }
    pub fn fire(&self) -> Result<()> {
        for path in &self.touch {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .with_context(|| format!("after_patch_touch {}", path.display()))?;
            ensure!(
                file.metadata()?.is_file(),
                "after_patch_touch requires a regular file"
            );
            // Rewriting one identical byte emits a data-modified event even for
            // watchers that ignore metadata events. Never truncate or replace an
            // editor's file, and do not read/copy the whole input on every save.
            let mut byte = [0];
            if file.read_at(&mut byte, 0)? != 0 {
                file.write_all_at(&byte, 0)?;
            }
            file.set_times(FileTimes::new().set_modified(SystemTime::now()))?;
        }
        if let Some(http) = &self.http {
            http.get(Duration::from_secs(2))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, net::TcpListener, thread};
    #[test]
    fn touch_preserves_bytes_and_does_not_create_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.md");
        let content = [0, 255, 0, 42];
        fs::write(&input, content).unwrap();
        Triggers::new(vec![input.clone()], None)
            .unwrap()
            .fire()
            .unwrap();
        assert_eq!(fs::read(&input).unwrap(), content);
        fs::write(&input, []).unwrap();
        Triggers::new(vec![input.clone()], None)
            .unwrap()
            .fire()
            .unwrap();
        assert!(fs::read(&input).unwrap().is_empty());
        fs::remove_file(&input).unwrap();
        assert!(
            Triggers::new(vec![input.clone()], None)
                .unwrap()
                .fire()
                .is_err()
        );
        assert!(!input.exists());
    }
    #[test]
    fn http_get_and_failure_status() {
        for status in [204, 503] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.extend(byte);
                }
                assert!(
                    String::from_utf8(request)
                        .unwrap()
                        .starts_with("GET /refresh?now=1 HTTP/1.0\r\n")
                );
                write!(
                    stream,
                    "HTTP/1.0 {status} Result\r\nContent-Length: 0\r\n\r\n"
                )
                .unwrap();
            });
            let triggers =
                Triggers::new(vec![], Some(&format!("http://{address}/refresh?now=1"))).unwrap();
            assert_eq!(triggers.fire().is_ok(), status == 204);
            server.join().unwrap();
        }
    }
    #[test]
    fn http_deadline_and_url_validation() {
        for bad in [
            "https://localhost/",
            "http:///bad",
            "http://user@host/",
            "http://host/#fragment",
            "http://host:0/",
            "http://host/\r\ninjected",
            "http://[bad]/",
            "http://host:123:456/",
        ] {
            assert!(Http::parse(bad).is_err(), "{bad}");
        }
        assert!(Http::parse("http://[::1]:8080/path").is_ok());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        let endpoint = Http::parse(&format!("http://{address}"))
            .unwrap()
            .resolve()
            .unwrap();
        assert!(endpoint.get(Duration::from_millis(20)).is_err());
        server.join().unwrap();
    }
}
