//! Stock Cargo is the authoritative slow rung, with isolated artifacts and fencing.
use crate::{config::Config, supervisor::terminate};
use anyhow::{Context, Result, bail};
use std::{
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Fence {
    pub revision: u64,
    pub latest: Arc<AtomicU64>,
    pub stopping: Arc<AtomicBool>,
}
impl Fence {
    pub fn current(&self) -> bool {
        !self.stopping.load(Ordering::SeqCst) && self.latest.load(Ordering::SeqCst) == self.revision
    }
}

pub fn cargo(config: &Config, project: &Path, target: &Path, verb: &str) -> Command {
    let mut command = if let Some(lane) = std::env::var_os("REFRESH_LANE_TOOL") {
        let mut c = Command::new(lane);
        c.args(["build", "--", "nice", "-n", "19", "cargo"]);
        c
    } else {
        let mut c = Command::new("nice");
        c.args(["-n", "19", "cargo"]);
        c
    };
    command
        .current_dir(project)
        .args([
            verb,
            "--offline",
            "-j",
            "4",
            "--package",
            &config.package,
            "--bin",
            &config.bin,
            "--message-format=json",
        ])
        .args(config.features.cargo_args())
        .env("RUSTUP_TOOLCHAIN", "1.99.0")
        .env("CARGO_TARGET_DIR", target)
        .process_group(0)
        .stdin(Stdio::null());
    for key in [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "LD_PRELOAD",
        "REFRESH_AGENT_SOCKET",
        "REFRESH_HOST_EXE",
        "E077_AGENT",
        "RUSTD_HOME",
    ] {
        command.env_remove(key);
    }
    command
}
#[derive(Debug)]
pub struct CargoResult {
    pub success: bool,
    pub diagnostic: String,
    pub executable: Option<PathBuf>,
}
pub fn parse_output(text: &str, success: bool) -> CargoResult {
    let mut diagnostic = None;
    let mut executable = None;
    for line in text.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if value["reason"] == "compiler-message"
                && value["message"]["level"] == "error"
                && diagnostic.is_none()
            {
                let code = value["message"]["code"]["code"].as_str().unwrap_or("");
                diagnostic = Some(
                    format!(
                        "{code} {}",
                        value["message"]["message"]
                            .as_str()
                            .unwrap_or("compilation failed")
                    )
                    .trim()
                    .to_owned(),
                );
            }
            if value["reason"] == "compiler-artifact"
                && let Some(path) = value["executable"].as_str()
            {
                executable = Some(path.into());
            }
        }
    }
    CargoResult {
        success,
        executable,
        diagnostic: diagnostic.unwrap_or_else(|| {
            text.lines()
                .rev()
                .find(|s| !s.trim().is_empty() && !s.starts_with('{'))
                .unwrap_or("Cargo failed without diagnostics")
                .to_owned()
        }),
    }
}
pub fn run(
    mut command: Command,
    log: &Path,
    fence: &Fence,
    memory_floor: u64,
) -> Result<CargoResult> {
    fs::create_dir_all(log.parent().context("log has no parent")?)?;
    let output = fs::File::create(log)?;
    command.stdout(output.try_clone()?).stderr(output);
    let mut child = command.spawn().context("start stock Cargo")?;
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        if !fence.current() {
            terminate(&mut child, Duration::from_millis(100));
            bail!("superseded");
        }
        let available = match crate::rustd::memory_available() {
            Ok(value) => value,
            Err(error) => {
                terminate(&mut child, Duration::from_millis(100));
                return Err(error);
            }
        };
        if available < memory_floor {
            terminate(&mut child, Duration::from_millis(100));
            bail!("available memory below safety floor");
        }
        if let Some(status) = child.try_wait()? {
            return Ok(parse_output(&fs::read_to_string(log)?, status.success()));
        }
        if Instant::now() > deadline {
            terminate(&mut child, Duration::from_millis(100));
            bail!("Cargo timed out");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
pub fn build(config: &Config, project: &Path, fence: &Fence, floor: u64) -> Result<CargoResult> {
    let storage = config.target_directory.join("refresh");
    run(
        cargo(config, project, &storage.join("stock"), "build"),
        &storage.join("stock-build.log"),
        fence,
        floor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    #[test]
    fn checks_and_stock_builds_share_feature_selection() {
        let metadata: crate::config::Metadata = serde_json::from_value(serde_json::json!({
            "packages": [{"id": "app", "name": "app", "source": null,
                "manifest_path": "/demo/Cargo.toml", "metadata": {}, "default_run": null,
                "targets": [{"name": "app", "kind": ["bin"], "src_path": "/demo/src/main.rs"}]}],
            "workspace_root": "/demo", "target_directory": "/demo/target"
        }))
        .unwrap();
        let args = crate::cli::RunArgs {
            features: crate::cli::Features {
                features: vec!["one,two".into(), "app/three".into()],
                no_default_features: true,
                all_features: true,
            },
            ..Default::default()
        };
        let config = Config::resolve(args, &metadata, Path::new("/demo")).unwrap();
        for verb in ["check", "build"] {
            let command = cargo(&config, Path::new("/demo"), Path::new("/demo/target"), verb);
            let args: Vec<_> = command.get_args().collect();
            let expected = config.features.cargo_args();
            assert!(args.ends_with(&expected.iter().map(|s| s.as_os_str()).collect::<Vec<_>>()));
        }
    }

    #[test]
    fn newer_revision_cancels_an_owned_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let pidfile = directory.path().join("pid");
        let mut command = Command::new("python3");
        command.args(["-c","import os,pathlib,sys,time; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(30)"])
            .arg(&pidfile).process_group(0);
        let latest = Arc::new(AtomicU64::new(1));
        let fence = Fence {
            revision: 1,
            latest: latest.clone(),
            stopping: Arc::new(AtomicBool::new(false)),
        };
        let log = directory.path().join("build.log");
        let task = thread::spawn(move || run(command, &log, &fence, 0));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pidfile.exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        let pid = fs::read_to_string(pidfile).unwrap();
        latest.store(2, Ordering::SeqCst);
        assert!(
            task.join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("superseded")
        );
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
}
