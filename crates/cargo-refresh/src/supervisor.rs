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
pub struct Supervisor {
    child: Option<Child>,
    run: Option<tempfile::TempDir>,
    pub executable: PathBuf,
    pub socket: PathBuf,
    project: PathBuf,
    args: Vec<OsString>,
    library: PathBuf,
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
            executable: PathBuf::new(),
            socket: PathBuf::new(),
            project: project.into(),
            args,
            library: library.canonicalize()?,
            hook,
            name,
            exit_reported: false,
            symbols: Arc::default(),
            stderr: None,
            load_base: 0,
        })
    }
    pub fn configure(&mut self, hook: bool, name: OsString) {
        self.hook = hook;
        self.name = name;
    }
    pub fn start(&mut self, binary: &Path) -> Result<()> {
        let started = Instant::now();
        self.stop();
        let stopped = started.elapsed();
        // /tmp keeps Unix socket names within sockaddr_un even in deep workspaces.
        let run = tempfile::Builder::new().prefix("refresh-run-").tempdir()?;
        fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700))?;
        let executable = run.path().join("app");
        fs::copy(binary, &executable).context("snapshot application executable")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o500))?;
        let snapshotted = started.elapsed();
        let socket = run.path().join("agent.sock");
        let mut command = Command::new(&executable);
        command
            .arg0(&self.name)
            .current_dir(&self.project)
            .args(&self.args)
            .process_group(0)
            .stderr(Stdio::piped())
            .env("LD_PRELOAD", &self.library)
            .env("REFRESH_AGENT_SOCKET", &socket)
            .env("REFRESH_HOST_EXE", &executable)
            .env("REFRESH_AFTER_PATCH", if self.hook { "1" } else { "0" });
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
        self.symbols = Arc::default();
        self.stderr = Some(crate::symbolize::StderrPump::start(
            stderr,
            self.symbols.clone(),
        )?);
        let spawned = started.elapsed();
        self.load_base = crate::symbolize::load_base(self.pid().unwrap(), &self.executable)?;
        let located = started.elapsed();
        *self.symbols.write().unwrap() =
            crate::symbolize::SymbolIndex::baseline(&self.executable, self.load_base)?;
        let indexed = started.elapsed();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
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
        if std::env::var_os("REFRESH_PROFILE_RESTART").is_some() {
            eprintln!("restart-timeline-us: stop={} snapshot={} spawn={} load_base={} symbols={} agent={}",
                stopped.as_micros(), (snapshotted - stopped).as_micros(),
                (spawned - snapshotted).as_micros(), (located - spawned).as_micros(),
                (indexed - located).as_micros(), (started.elapsed() - indexed).as_micros());
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
        agent.ping()?;
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
        supervisor.stop();
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
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
