//! Immutable executable snapshots and owned application process groups.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};

pub const AGENT: &[u8] = include_bytes!(env!("REFRESH_BUNDLED_AGENT"));

/// Copy a completed candidate to a private inode without materializing its code-region hole.
fn snapshot_executable(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::io::{Read, Seek, SeekFrom};
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let mut input = fs::File::open(source)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)?;
        const FICLONE: libc::c_ulong = 0x4004_9409;
        if unsafe { libc::ioctl(output.as_raw_fd(), FICLONE, input.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let len = input.metadata()?.len();
        output.set_len(0)?;
        output.set_len(len)?;
        let mut at = 0;
        while at < len {
            let data =
                unsafe { libc::lseek(input.as_raw_fd(), at as libc::off_t, libc::SEEK_DATA) };
            if data < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ENXIO) {
                    break;
                }
                if matches!(error.raw_os_error(), Some(libc::EINVAL | libc::EOPNOTSUPP)) {
                    fs::copy(source, destination)?;
                    return Ok(());
                }
                return Err(error.into());
            }
            let data = data as u64;
            ensure!(data >= at && data < len, "invalid snapshot data extent");
            let hole =
                unsafe { libc::lseek(input.as_raw_fd(), data as libc::off_t, libc::SEEK_HOLE) };
            if hole < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let end = (hole as u64).min(len);
            ensure!(end > data, "invalid snapshot hole extent");
            input.seek(SeekFrom::Start(data))?;
            output.seek(SeekFrom::Start(data))?;
            // std's file-to-file copy uses copy_file_range where supported.
            let copied =
                std::io::copy(&mut Read::by_ref(&mut input).take(end - data), &mut output)?;
            ensure!(copied == end - data, "candidate truncated during snapshot");
            at = end;
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        fs::copy(source, destination)?;
        Ok(())
    }
}

pub struct Supervisor {
    child: Option<Child>,
    run: Option<tempfile::TempDir>,
    sockets: Option<tempfile::TempDir>,
    pub executable: PathBuf,
    pub socket: PathBuf,
    project: PathBuf,
    args: Vec<OsString>,
    library: Option<PathBuf>,
    hook: bool,
    name: OsString,
    exit_reported: bool,
    symbols: Arc<RwLock<crate::symbolize::SymbolIndex>>,
    stderr: Option<crate::symbolize::StderrPump>,
    load_base: u64,
}
pub fn terminate(child: &mut Child, grace: Duration) {
    let group = -(child.id() as i32);
    // Signal the entire owned group even if the leader has already exited.
    unsafe {
        libc::kill(group, libc::SIGTERM);
    }
    let until = Instant::now() + grace;
    loop {
        let _ = child.try_wait();
        if unsafe { libc::kill(group, 0) } != 0 {
            break;
        }
        if Instant::now() >= until {
            unsafe {
                libc::kill(group, libc::SIGKILL);
            }
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.wait();
}
impl Supervisor {
    pub fn new(
        project: &Path,
        storage: &Path,
        args: Vec<OsString>,
        hook: bool,
        name: OsString,
    ) -> Result<Self> {
        fs::create_dir_all(storage)?;
        let library = storage.join("librefresh.so");
        // Write a new inode: a running host may still have the old library mapped.
        let temporary = storage.join(format!("librefresh-{}.tmp", std::process::id()));
        fs::write(&temporary, AGENT)?;
        fs::rename(&temporary, &library)?;
        Ok(Self {
            child: None,
            run: None,
            sockets: None,
            executable: PathBuf::new(),
            socket: PathBuf::new(),
            project: project.into(),
            args,
            library: Some(library.canonicalize()?),
            hook,
            name,
            exit_reported: false,
            symbols: Arc::default(),
            stderr: None,
            load_base: 0,
        })
    }
    /// Use only the validated release agent, or no injection in stock mode.
    pub fn configure_agent(&mut self, library: Option<&Path>) {
        self.library = library.map(Path::to_path_buf);
    }
    pub fn configure(&mut self, hook: bool, name: OsString) {
        self.hook = hook;
        self.name = name;
    }
    pub fn start(&mut self, binary: &Path) -> Result<()> {
        let started = Instant::now();
        // Executable snapshots follow TMPDIR on disk; sockets use a short namespace.
        let run = tempfile::Builder::new().prefix("refresh-run-").tempdir()?;
        fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700))?;
        let sockets = self
            .library
            .as_ref()
            .map(|_| crate::runtime_dir::socket_dir())
            .transpose()?;
        let socket = sockets
            .as_ref()
            .map(|dir| dir.path().join("agent.sock"))
            .unwrap_or_default();
        let startup_error = sockets
            .as_ref()
            .map(|dir| dir.path().join("agent.error"))
            .unwrap_or_default();
        if self.library.is_some() {
            crate::runtime_dir::validate_socket(&socket)?;
        }
        let executable = run.path().join("app");
        // Keep serving while preparing the immutable replacement. On disk a
        // sparse ELF snapshot can take much longer than process shutdown; doing
        // these concurrently turns all of that I/O into connection downtime.
        snapshot_executable(binary, &executable).context("snapshot application executable")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o500))?;
        let snapshotted = started.elapsed();
        self.stop();
        let stopped = started.elapsed();
        let mut command = Command::new(&executable);
        command
            .arg0(&self.name)
            .current_dir(&self.project)
            .args(&self.args)
            .process_group(0)
            .stderr(Stdio::piped());
        if let Some(library) = &self.library {
            command
                .env("LD_PRELOAD", library)
                .env("REFRESH_AGENT_SOCKET", &socket)
                .env("REFRESH_AGENT_ERROR", &startup_error)
                .env("REFRESH_HOST_EXE", &executable)
                .env("REFRESH_AFTER_PATCH", if self.hook { "1" } else { "0" });
        } else {
            for name in [
                "LD_PRELOAD",
                "REFRESH_AGENT_SOCKET",
                "REFRESH_AGENT_ERROR",
                "REFRESH_HOST_EXE",
                "REFRESH_AFTER_PATCH",
            ] {
                command.env_remove(name);
            }
        }
        // Rust 1.99 short traces omit instruction addresses even for unknown
        // frames. Request address-bearing output so patched PCs can be resolved.
        if std::env::var_os("RUST_BACKTRACE").is_some_and(|v| v == "1") {
            command.env("RUST_BACKTRACE", "full");
        }
        let mut child = crate::process::spawn(&mut command).context("start application")?;
        let stderr = child
            .stderr
            .take()
            .context("application stderr pipe missing")?;
        self.child = Some(child);
        self.exit_reported = false;
        self.executable = executable;
        self.socket = socket;
        self.run = Some(run);
        self.sockets = sockets;
        self.symbols = Arc::default();
        self.stderr = Some(crate::symbolize::StderrPump::start(
            stderr,
            self.symbols.clone(),
        )?);
        let spawned = started.elapsed();
        self.load_base = 0;
        if self.library.is_none() {
            return Ok(());
        }
        self.load_base = crate::symbolize::load_base(self.pid().unwrap(), &self.executable)?;
        let located = started.elapsed();
        *self.symbols.write().unwrap() =
            crate::symbolize::SymbolIndex::baseline(&self.executable, self.load_base)?;
        let indexed = started.elapsed();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(error) = fs::read_to_string(&startup_error)
                && !error.is_empty()
            {
                anyhow::bail!("application agent startup failed: {}", error.trim());
            }
            ensure!(
                self.child.as_mut().unwrap().try_wait()?.is_none(),
                "application exited before agent became ready"
            );
            if self.socket.exists() && std::os::unix::net::UnixStream::connect(&self.socket).is_ok()
            {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "application agent startup timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
        crate::agent::Client::new(&self.socket).ping_compatible()?;
        if std::env::var_os("REFRESH_PROFILE_RESTART").is_some() {
            eprintln!(
                "restart-timeline-us: stop={} snapshot={} spawn={} load_base={} symbols={} agent={}",
                (stopped - snapshotted).as_micros(),
                snapshotted.as_micros(),
                (spawned - stopped).as_micros(),
                (located - spawned).as_micros(),
                (indexed - located).as_micros(),
                (started.elapsed() - indexed).as_micros()
            );
        }
        Ok(())
    }
    /// Stage names before APPLY: its hook can panic before the acknowledgement.
    /// An explicit refusal removes staged names so reused addresses cannot keep
    /// stale source lines. Ambiguous transport failure already requires restart.
    pub fn apply_patch(
        &mut self,
        patch: &crate::extractor::PreparedPatch,
    ) -> Result<crate::agent::Reply> {
        let mut agent = crate::agent::Client::new(&self.socket);
        agent.ping_compatible()?;
        let inserted = self.symbols.write().unwrap().add_generation(
            self.load_base,
            patch.generation,
            &patch.symbols,
        );
        let reply = agent.apply(&patch.records);
        if reply.as_ref().is_ok_and(|reply| !reply.ok) {
            self.symbols.write().unwrap().discard(&inserted);
        }
        reply
    }
    pub fn running(&mut self) -> Result<bool> {
        match &mut self.child {
            Some(child) => Ok(child.try_wait()?.is_none()),
            None => Ok(false),
        }
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    pub fn poll_exit(&mut self) -> Result<Option<ExitStatus>> {
        if self.exit_reported {
            return Ok(None);
        }
        let status = match &mut self.child {
            Some(child) => child.try_wait()?,
            None => None,
        };
        if status.is_some() {
            self.exit_reported = true;
        }
        Ok(status)
    }
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate(&mut child, Duration::from_secs(3));
        }
        if let Some(stderr) = self.stderr.take() {
            stderr.stop();
        }
        self.run = None;
        self.sockets = None;
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn sparse_snapshot_keeps_bytes_holes_and_an_independent_inode() {
        use std::os::{
            fd::AsRawFd,
            unix::fs::{FileExt, MetadataExt},
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("snapshot");
        let file = fs::File::create(&source).unwrap();
        file.set_len(32 * 1024 * 1024).unwrap();
        file.write_all_at(b"first", 0).unwrap();
        file.write_all_at(b"last", 20 * 1024 * 1024).unwrap();
        snapshot_executable(&source, &target).unwrap();
        let copy = fs::File::open(&target).unwrap();
        let mut bytes = [0; 5];
        copy.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"first");
        copy.read_exact_at(&mut bytes[..4], 20 * 1024 * 1024)
            .unwrap();
        assert_eq!(&bytes[..4], b"last");
        copy.read_exact_at(&mut bytes, 10 * 1024 * 1024).unwrap();
        assert_eq!(bytes, [0; 5]);
        assert_eq!(
            copy.metadata().unwrap().len(),
            file.metadata().unwrap().len()
        );
        assert_ne!(
            copy.metadata().unwrap().ino(),
            file.metadata().unwrap().ino()
        );
        file.write_all_at(b"newer", 0).unwrap();
        copy.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"first");
        let probe = fs::File::open(&source).unwrap();
        if unsafe { libc::lseek(probe.as_raw_fd(), 4096, libc::SEEK_DATA) } > 4096 {
            assert!(copy.metadata().unwrap().blocks() * 512 < 1024 * 1024);
        }
        // A file consisting entirely of holes must retain its length, too.
        let empty = dir.path().join("empty");
        fs::File::create(&empty)
            .unwrap()
            .set_len(16 * 1024 * 1024)
            .unwrap();
        snapshot_executable(&empty, &dir.path().join("empty-copy")).unwrap();
        assert_eq!(
            fs::metadata(dir.path().join("empty-copy")).unwrap().len(),
            16 * 1024 * 1024
        );
    }
    #[test]
    fn immutable_copy_group_and_agent_host_identity() {
        let storage = tempfile::tempdir().unwrap();
        let mut supervisor = Supervisor::new(
            storage.path(),
            storage.path(),
            vec!["-c".into(), "sleep 30; :".into()],
            false,
            "bash".into(),
        )
        .unwrap();
        supervisor.start(Path::new("/bin/bash")).unwrap();
        let pid = supervisor.pid().unwrap();
        assert_eq!(unsafe { libc::getpgid(pid as i32) }, pid as i32);
        assert_eq!(
            fs::read(&supervisor.executable).unwrap(),
            fs::read("/bin/bash").unwrap()
        );
        assert_eq!(
            fs::metadata(&supervisor.executable)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
        assert_eq!(
            fs::metadata(supervisor.socket.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(
            crate::agent::Client::new(&supervisor.socket)
                .ping()
                .unwrap()
                .ok
        );
        let old_executable = supervisor.executable.clone();
        let old_socket = supervisor.socket.clone();
        assert!(
            supervisor
                .start(&storage.path().join("missing-candidate"))
                .is_err()
        );
        assert_eq!(supervisor.pid(), Some(pid));
        assert_eq!(supervisor.executable, old_executable);
        assert_eq!(supervisor.socket, old_socket);
        assert!(supervisor.poll_exit().unwrap().is_none());
        assert!(crate::agent::Client::new(&old_socket).ping().unwrap().ok);
        supervisor.start(Path::new("/bin/bash")).unwrap();
        let replacement = supervisor.pid().unwrap();
        assert_ne!(replacement, pid);
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert!(!old_executable.exists());
        supervisor.stop();
        assert!(!Path::new(&format!("/proc/{replacement}")).exists());
    }
    #[test]
    fn stock_mode_runs_without_an_agent_and_resets_symbols() {
        let storage = tempfile::tempdir().unwrap();
        let script = storage.path().join("stock-script");
        // Stock mode must neither wait for an agent nor require an ELF symbol table.
        fs::write(&script, "#!/bin/sh\necho stock-stderr >&2\nsleep 30\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let mut supervisor = Supervisor::new(
            storage.path(),
            storage.path(),
            vec![],
            false,
            "stock-script".into(),
        )
        .unwrap();
        supervisor.configure_agent(None);
        let old_symbols = supervisor.symbols.clone();
        supervisor.start(&script).unwrap();
        assert!(supervisor.stderr.is_some());
        assert!(!Arc::ptr_eq(&old_symbols, &supervisor.symbols));
        assert!(!supervisor.socket.exists());
        assert!(supervisor.poll_exit().unwrap().is_none());
        supervisor.stop();
    }

    #[test]
    fn unexpected_exit_is_observed_once() {
        let storage = tempfile::tempdir().unwrap();
        let mut supervisor = Supervisor::new(
            storage.path(),
            storage.path(),
            vec!["-c".into(), "sleep .1; exit 23".into()],
            false,
            "bash".into(),
        )
        .unwrap();
        supervisor.start(Path::new("/bin/bash")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = supervisor.poll_exit().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(status.code(), Some(23));
        assert!(supervisor.poll_exit().unwrap().is_none());
    }
}
