//! Owned daemon lifecycle and Cargo-lock-free worker requests.
//!
//! The caller must hold the research `rustd` lane for integration runs. The
//! product memory floor defaults to 2 GiB; research callers should use 6 GiB.
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

const SCRUB: &[&str] = &[
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_TARGET_DIR",
    "LD_PRELOAD",
    "E077_AGENT",
    "REFRESH_AGENT_SOCKET",
    "REFRESH_HOST_EXE",
];
#[derive(Clone, Debug)]
pub struct Config {
    pub project: PathBuf,
    pub rustd: PathBuf,
    pub binary: String,
    pub package: Option<String>,
    pub edit: Vec<String>,
    pub features: crate::cli::Features,
    pub home: PathBuf,
    pub memory_min_bytes: u64,
    pub ready_timeout: Duration,
    pub command_timeout: Duration,
    pub extra_env: BTreeMap<OsString, OsString>,
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
impl Config {
    pub fn new(
        project: impl AsRef<Path>,
        rustd: impl AsRef<Path>,
        binary: impl Into<String>,
    ) -> Result<Self> {
        let project = fs::canonicalize(project)?;
        let rustd = fs::canonicalize(rustd)?;
        Ok(Self {
            home: project.join("target/refresh/d"),
            project,
            rustd,
            binary: binary.into(),
            package: None,
            edit: Vec::new(),
            features: crate::cli::Features::default(),
            memory_min_bytes: 2 * 1024 * 1024 * 1024,
            ready_timeout: Duration::from_secs(240),
            command_timeout: Duration::from_secs(240),
            extra_env: BTreeMap::new(),
            cancel: None,
        })
    }
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.current_dir(&self.project);
        cmd.envs(&self.extra_env);
        for &key in SCRUB {
            cmd.env_remove(key);
        }
        // Keep the project/toolchain environment used by install::Toolchain::detect.
        // A version label here could select a different compiler than the verified one.
        cmd.env("RUSTD_HOME", &self.home)
            .env("RUSTD_SPARE", "0")
            .env("RUSTD_CACHE", "off")
            .env("RUSTD_IDLE_SECS", "0")
            .env("CARGO_BUILD_JOBS", "4");
        let mut paths = vec![self.rustd.parent().unwrap().to_owned()];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        if let Ok(path) = std::env::join_paths(paths) {
            cmd.env("PATH", path);
        }
        cmd.process_group(0);
        cmd
    }
}
pub fn memory_available() -> Result<u64> {
    let text = fs::read_to_string("/proc/meminfo")?;
    let kb = text
        .lines()
        .find_map(|l| {
            l.strip_prefix("MemAvailable:")
                .and_then(|s| s.split_whitespace().next())
        })
        .context("MemAvailable missing")?
        .parse::<u64>()?;
    kb.checked_mul(1024).context("MemAvailable overflow")
}
fn terminate(child: &mut Child, grace: Duration) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    // This group was created by CommandExt::process_group(0), never discovered
    // by process name. Do not signal unrelated daemons or compilers.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    let until = Instant::now() + grace;
    while Instant::now() < until {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}
#[derive(Debug)]
pub struct Build {
    pub manifest_path: PathBuf,
    pub candidate_path: PathBuf,
    pub manifest: Vec<u8>,
}
impl Build {
    /// A new generation number is not evidence that an admitted source edit
    /// reached machine code. Manifests carry delta pages but cumulative writes;
    /// old redirects must not make an empty build look successful.
    pub fn ensure_compiled_change(&self, previous: &crate::manifest::State) -> Result<()> {
        let manifest = refresh_proto::manifest::parse(&self.manifest)?;
        let code_page = manifest.pages.iter().any(|page| {
            page.prot & 4 != 0
                && previous
                    .pages
                    .get(&page.addr)
                    .is_none_or(|old| **old != *page)
        });
        let redirect = manifest
            .writes
            .iter()
            .any(|write| write.kind == 1 && previous.writes.get(&write.addr) != Some(write));
        let restore = manifest
            .restores
            .iter()
            .any(|write| write.kind == 1 && previous.writes.contains_key(&write.addr));
        ensure!(
            code_page || redirect || restore,
            "compiler produced no compiled change for changed Rust source; using Cargo"
        );
        Ok(())
    }
}
struct SocketAlias {
    _dir: tempfile::TempDir,
    home: PathBuf,
}
pub struct Client {
    pub config: Config,
    daemon: Option<Child>,
    last_manifest: Vec<u8>,
    commands: u64,
    socket_alias: Option<SocketAlias>,
}
impl Client {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            daemon: None,
            last_manifest: vec![],
            commands: 0,
            socket_alias: None,
        }
    }
    pub fn guard_memory(&mut self) -> Result<()> {
        ensure!(
            !self
                .config
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst)),
            "compiler operation cancelled"
        );
        let available = memory_available()?;
        if available < self.config.memory_min_bytes {
            self.stop()?;
            bail!(
                "rustd stopped: MemAvailable {available} below {} bytes; switch to stock Cargo",
                self.config.memory_min_bytes
            );
        }
        Ok(())
    }
    pub fn start(&mut self) -> Result<()> {
        ensure!(self.daemon.is_none(), "daemon already started");
        self.guard_memory()?;
        ensure!(
            !self.config.home.join("daemon.sock").exists(),
            "RUSTD_HOME already has a daemon socket; refusing to attach to an unowned daemon"
        );
        fs::create_dir_all(&self.config.home)?;
        fs::set_permissions(&self.config.home, fs::Permissions::from_mode(0o700))?;
        // Worker paths add a 32-byte hash and a socket suffix. Deep project
        // paths exceed Linux's sockaddr_un limit even if daemon.sock fits.
        // Keep all daemon state under target/refresh/d, but address its sockets
        // through a short, private symlink for this owned daemon's lifetime.
        let result = (|| -> Result<()> {
            let alias = tempfile::Builder::new()
                .prefix("crfd-")
                .tempdir_in("/tmp")?;
            // The final component must be a real directory: rustd deliberately
            // refuses RUSTD_HOME itself being a symlink. Only alias its parent.
            let home = fs::canonicalize(&self.config.home)?;
            let parent = home.parent().context("daemon home has no parent")?;
            let name = home.file_name().context("daemon home has no basename")?;
            std::os::unix::fs::symlink(parent, alias.path().join("p"))?;
            let short = alias.path().join("p").join(name);
            ensure!(
                short.as_os_str().len() + 52 < 108,
                "daemon home basename too long for worker sockets; use a short name such as d"
            );
            self.socket_alias = Some(SocketAlias {
                _dir: alias,
                home: short,
            });
            let log = File::create(self.config.home.join("daemon.log"))?;
            let mut cmd = self.command(&self.config.rustd);
            cmd.arg("daemon").stdout(log.try_clone()?).stderr(log);
            self.daemon = Some(crate::process::spawn(&mut cmd).context("start rustd daemon")?);
            self.initialize()?;
            if let Ok(path) = newest_manifest(&self.config.project) {
                self.last_manifest = fs::read(path)?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.stop();
        }
        result
    }
    fn transport_home(&self) -> PathBuf {
        self.socket_alias
            .as_ref()
            .map_or_else(|| self.config.home.clone(), |alias| alias.home.clone())
    }
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = self.config.command(program);
        command.env("RUSTD_HOME", self.transport_home());
        command
    }
    fn initialize(&mut self) -> Result<()> {
        let deadline = Instant::now() + self.config.ready_timeout;
        while !self.config.home.join("daemon.sock").exists() {
            self.check_daemon()?;
            self.guard_memory()?;
            ensure!(Instant::now() < deadline, "daemon socket timeout");
            thread::sleep(Duration::from_millis(25));
        }
        // cargo-rustd captures the rustc invocation using its workspace wrapper.
        // Setup builds are low priority; saves below bypass Cargo entirely.
        let mut capture = if let Some(lane) = std::env::var_os("REFRESH_LANE_TOOL") {
            let mut c = self.command(lane);
            c.args(["build", "--"]);
            c.arg("nice");
            c
        } else {
            self.command("nice")
        };
        capture.args([
            "-n",
            "19",
            "cargo",
            "rustd",
            "build",
            "-j",
            "4",
            "--bin",
            &self.config.binary,
        ]);
        if let Some(package) = &self.config.package {
            capture.args(["--package", package]);
        }
        capture.args(self.config.features.cargo_args());
        // M1-T2 owns the full closure. M1-T3 exposes frontier selection; opt in
        // only when using a compiler whose interface supports it.
        if std::env::var_os("REFRESH_RUSTD_FRONTIER").is_some() {
            for edit in &self.config.edit {
                capture.args(["--edit", edit]);
            }
        }
        let base_started = Instant::now();
        eprintln!("[refresh] base build (normal cargo, first run only)…");
        self.run(capture, self.config.command_timeout)?;
        eprintln!(
            "[refresh] base build done in {:.2} s",
            base_started.elapsed().as_secs_f64()
        );
        let warm_started = Instant::now();
        eprintln!("[refresh] warming compiler…");
        while Instant::now() < deadline {
            self.check_daemon()?;
            self.guard_memory()?;
            let mut status = self.command("cargo");
            status.args(["rustd", "status"]);
            let text = self.run(status, Duration::from_secs(15))?;
            let project = self.config.project.to_string_lossy();
            let lines: Vec<_> = text
                .lines()
                .filter(|l| l.contains(project.as_ref()))
                .collect();
            ensure!(
                !lines.iter().any(|l| l.contains(": failed")),
                "worker failed: {text}"
            );
            if lines.iter().any(|l| l.contains(": ready"))
                && !lines.iter().any(|l| l.contains("loading for"))
            {
                eprintln!(
                    "[refresh] warming compiler… ready in {:.2} s",
                    warm_started.elapsed().as_secs_f64()
                );
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!("rustd worker readiness timeout")
    }
    fn check_daemon(&mut self) -> Result<()> {
        let child = self.daemon.as_mut().context("rustd not started")?;
        ensure!(child.try_wait()?.is_none(), "rustd daemon exited");
        Ok(())
    }
    fn run(&mut self, mut command: Command, timeout: Duration) -> Result<String> {
        self.guard_memory()?;
        self.commands += 1;
        let path = self
            .config
            .home
            .join(format!("command-{}.log", self.commands));
        let log = File::create(&path)?;
        command.stdout(log.try_clone()?).stderr(log);
        let mut child = crate::process::spawn(&mut command)?;
        let deadline = Instant::now() + timeout;
        let result = (|| -> Result<_> {
            loop {
                if let Some(status) = child.try_wait()? {
                    let text = fs::read_to_string(&path).unwrap_or_default();
                    ensure!(status.success(), "rustd command {status}: {text}");
                    return Ok(text);
                }
                self.guard_memory()?;
                ensure!(Instant::now() < deadline, "rustd command timeout");
                thread::sleep(Duration::from_millis(5));
            }
        })();
        if result.is_err() {
            terminate(&mut child, Duration::from_secs(1));
        }
        // Keep failures for diagnostics; successful per-save logs do not grow.
        if result.is_ok() {
            let _ = fs::remove_file(path);
        }
        result
    }
    pub fn build(&mut self) -> Result<Build> {
        self.build_with_async_certificates(false)
    }
    pub fn build_with_async_certificates(&mut self, async_certs: bool) -> Result<Build> {
        self.check_daemon()?;
        self.guard_memory()?;
        let sockets = sockets(&self.transport_home().join("workers"))?;
        ensure!(
            sockets.len() == 1,
            "expected one direct worker socket, found {}",
            sockets.len()
        );
        let mut command = self.command(&self.config.rustd);
        command
            .arg("sync")
            .arg(&sockets[0])
            .args(["--async-certs", if async_certs { "true" } else { "false" }]);
        self.run(command, self.config.command_timeout)?;
        let manifest_path = newest_manifest(&self.config.project)?;
        let manifest = fs::read(&manifest_path)?;
        ensure!(
            manifest != self.last_manifest,
            "no fresh compiler manifest; restart required"
        );
        refresh_proto::manifest::parse(&manifest)?;
        self.last_manifest = manifest.clone();
        let candidate_path = manifest_path.with_file_name("bin");
        ensure!(
            candidate_path.is_file(),
            "manifest candidate binary missing"
        );
        Ok(Build {
            manifest_path,
            candidate_path,
            manifest,
        })
    }
    pub fn stop(&mut self) -> Result<()> {
        let Some(mut daemon) = self.daemon.take() else {
            self.socket_alias = None;
            return Ok(());
        };
        let mut cmd = self.command("cargo");
        cmd.args(["rustd", "stop"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(mut child) = crate::process::spawn(&mut cmd) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(None) => thread::sleep(Duration::from_millis(25)),
                    Ok(Some(_)) | Err(_) => break,
                }
            }
            terminate(&mut child, Duration::from_millis(100));
        }
        terminate(&mut daemon, Duration::from_secs(5));
        self.socket_alias = None;
        Ok(())
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
fn sockets(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_socket() {
            out.push(entry.into_path());
        }
    }
    Ok(out)
}
pub fn newest_manifest(project: &Path) -> Result<PathBuf> {
    let root = project.join("target/rustd");
    let mut found: Option<(SystemTime, PathBuf)> = None;
    if root.exists() {
        for e in walkdir::WalkDir::new(&root).min_depth(4).max_depth(4) {
            let e = e?;
            if e.file_type().is_file()
                && e.file_name() == "patch.manifest"
                && e.path()
                    .parent()
                    .and_then(Path::parent)
                    .is_some_and(|p| p.ends_with("programs"))
            {
                let modified = e.metadata()?.modified()?;
                if found.as_ref().is_none_or(|(old, _)| modified > *old) {
                    found = Some((modified, e.into_path()));
                }
            }
        }
    }
    found
        .map(|(_, p)| p)
        .context("no target/rustd/*/programs/*/patch.manifest")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn executable(path: &Path, text: &str) {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[test]
    fn env_scrubbing_and_manifest_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let rustd = tmp.path().join("rustd");
        fs::write(&rustd, "").unwrap();
        let config = Config::new(tmp.path(), &rustd, "web").unwrap();
        let c = config.command("cargo");
        let env: BTreeMap<_, _> = c.get_envs().collect();
        for key in SCRUB {
            assert_eq!(env.get(std::ffi::OsStr::new(key)), Some(&None));
        }
        assert!(!env.contains_key(std::ffi::OsStr::new("RUSTUP_TOOLCHAIN")));
        let dir = tmp.path().join("target/rustd/a/programs/b");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("patch.manifest"), b"test").unwrap();
        assert_eq!(
            newest_manifest(tmp.path()).unwrap(),
            dir.join("patch.manifest")
        );
    }
    #[test]
    fn cumulative_redirects_do_not_count_as_a_new_compilation() {
        use crate::manifest::{Page, State, Write};
        use std::sync::Arc;
        let page = Page {
            addr: 0x4000,
            prot: 5,
            payload: vec![1; 4096],
            fixups: vec![],
        };
        let write = Write {
            addr: 0x1000,
            kind: 1,
            payload: vec![0xe9, 0, 0, 0, 0],
            fixups: vec![],
            name: vec![],
        };
        let mut previous = State::default();
        previous.pages.insert(page.addr, Arc::new(page.clone()));
        previous.writes.insert(write.addr, write.clone());
        let encode = |page: Option<&Page>, write: Option<&Write>, restore: Option<&Write>| {
            let mut bytes = b"RDM09001".to_vec();
            bytes.extend(2_u64.to_le_bytes());
            bytes.extend(0_u32.to_le_bytes()); // segments not needed for this policy check
            bytes.extend(u32::from(page.is_some()).to_le_bytes());
            if let Some(page) = page {
                bytes.extend(page.addr.to_le_bytes());
                bytes.extend(page.prot.to_le_bytes());
                bytes.extend((page.payload.len() as u32).to_le_bytes());
                bytes.extend(&page.payload);
                bytes.extend(0_u32.to_le_bytes());
            }
            for (write, named) in [(write, true), (restore, false)] {
                bytes.extend(u32::from(write.is_some()).to_le_bytes());
                if let Some(write) = write {
                    bytes.extend(write.addr.to_le_bytes());
                    bytes.extend(write.kind.to_le_bytes());
                    bytes.extend((write.payload.len() as u32).to_le_bytes());
                    bytes.extend(&write.payload);
                    bytes.extend(0_u32.to_le_bytes());
                    if named {
                        bytes.extend(0_u32.to_le_bytes());
                    }
                }
            }
            bytes.extend([0; 24]);
            Build {
                manifest: bytes,
                manifest_path: "unused".into(),
                candidate_path: "unused".into(),
            }
        };
        assert!(
            encode(None, Some(&write), None)
                .ensure_compiled_change(&previous)
                .is_err()
        );
        assert!(
            encode(Some(&page), Some(&write), None)
                .ensure_compiled_change(&previous)
                .is_err()
        );
        let mut changed = page.clone();
        changed.payload[0] = 2;
        assert!(
            encode(Some(&changed), Some(&write), None)
                .ensure_compiled_change(&previous)
                .is_ok()
        );
        let mut changed = write.clone();
        changed.payload[1] = 2;
        assert!(
            encode(None, Some(&changed), None)
                .ensure_compiled_change(&previous)
                .is_ok()
        );
        assert!(
            encode(None, None, Some(&write))
                .ensure_compiled_change(&previous)
                .is_ok()
        );
    }

    #[test]
    fn fake_daemon_direct_build_stale_and_memory_stop() {
        // These executables simulate the command contract; no rustd or Cargo
        // build is started, and no shared compiler resource lane is needed.
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("tools");
        fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("rustd"),
            r#"#!/usr/bin/env python3
import os,socket,pathlib,sys,struct,time
home=pathlib.Path(os.environ['RUSTD_HOME'])
if sys.argv[1]=='daemon':
 home.joinpath('workers').mkdir(exist_ok=True)
 a=socket.socket(socket.AF_UNIX);a.bind(str(home/'daemon.sock'))
 b=socket.socket(socket.AF_UNIX);b.bind(str(home/'workers/worker.sock'))
 (home/'owned.pid').write_text(str(os.getpid()))
 while True:time.sleep(.1)
elif sys.argv[1]=='sync':
 assert sys.argv[2]==str(home/'workers/worker.sock')
 p=pathlib.Path.cwd()/'target/rustd/a/programs/b';p.mkdir(parents=True,exist_ok=True)
 (p/'bin').write_bytes(b'candidate')
 (p/'patch.manifest').write_bytes(b'RDM09001'+struct.pack('<Q',1)+bytes(40))
else:raise Exception(sys.argv)
"#,
        );
        executable(
            &bin.join("cargo"),
            r#"#!/usr/bin/env python3
import sys,os,pathlib,signal
assert sys.argv[1]=='rustd'
if sys.argv[2]=='status':print(str(pathlib.Path.cwd())+': ready')
elif sys.argv[2]=='build':
 assert '-j' in sys.argv
 assert sys.argv[sys.argv.index('--features')+1]=='chosen,other'
 assert '--no-default-features' in sys.argv and '--all-features' in sys.argv
elif sys.argv[2]=='stop':
 p=pathlib.Path(os.environ['RUSTD_HOME'])/'owned.pid'
 if p.exists():
  try:os.kill(int(p.read_text()),signal.SIGTERM)
  except ProcessLookupError:pass
else:raise Exception(sys.argv)
"#,
        );
        let mut cfg = Config::new(tmp.path(), bin.join("rustd"), "fake").unwrap();
        cfg.features = crate::cli::Features {
            features: vec!["chosen,other".into()],
            no_default_features: true,
            all_features: true,
        };
        cfg.home = tmp.path().join("deep").join("x".repeat(150)).join("d");
        cfg.memory_min_bytes = 0;
        cfg.ready_timeout = Duration::from_secs(5);
        let mut c = Client::new(cfg);
        c.start().unwrap();
        let pid = c.daemon.as_ref().unwrap().id();
        let transport = c.transport_home();
        assert!(transport.as_os_str().len() < 64);
        assert!(
            fs::metadata(c.config.home.join("daemon.sock"))
                .unwrap()
                .file_type()
                .is_socket()
        );
        fs::write(c.config.home.join("retain"), b"persistent").unwrap();
        let build = c.build().unwrap();
        assert!(
            build
                .ensure_compiled_change(&crate::manifest::State::default())
                .unwrap_err()
                .to_string()
                .contains("no compiled change")
        );
        assert_eq!(
            refresh_proto::manifest::parse(&build.manifest)
                .unwrap()
                .generation,
            1
        );
        assert!(c.build().unwrap_err().to_string().contains("no fresh"));
        c.config.memory_min_bytes = u64::MAX;
        assert!(
            c.guard_memory()
                .unwrap_err()
                .to_string()
                .contains("MemAvailable")
        );
        assert!(c.daemon.is_none());
        assert!(!transport.exists(), "owned socket alias must be removed");
        assert_eq!(
            fs::read(c.config.home.join("retain")).unwrap(),
            b"persistent"
        );
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
}
