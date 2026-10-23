use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

struct Running {
    child: Child,
}
impl Drop for Running {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let _ = self.child.wait();
    }
}

fn wait_until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "controller condition timed out");
        thread::sleep(Duration::from_millis(2));
    }
}

fn rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("src")).unwrap();
    fs::write(
        directory.path().join("Cargo.toml"),
        r#"
[package]
name = "watch-fixture"
version = "0.0.0"
edition = "2024"
[package.metadata.refresh]
edit = ["watch-fixture"]
allow_tracing_metadata = false
after_patch_hook = true
strict = true
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("src/main.rs"),
        "fn main() { loop { std::thread::sleep(std::time::Duration::from_millis(10)); } }\n",
    )
    .unwrap();
    directory
}

#[test]
fn direct_and_cargo_style_run_resolve_config_build_and_log_without_self_events() {
    for invocation in ["direct", "cargo-style", "cargo"] {
        let directory = fixture();
        let root = directory.path();
        let log = root.join("src/events.jsonl");
        let stdout = root.join("stdout.txt");
        let stderr = root.join("stderr.txt");
        let executable = Path::new(env!("CARGO_BIN_EXE_cargo-refresh"));
        let mut command = Command::new(if invocation == "cargo" {
            Path::new("cargo")
        } else {
            executable
        });
        if invocation == "cargo" {
            let inherited_path = std::env::var_os("PATH").unwrap();
            let path = std::iter::once(executable.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&inherited_path));
            command.env("PATH", std::env::join_paths(path).unwrap());
        }
        command
            .current_dir(root)
            .env("RUSTUP_TOOLCHAIN", "1.99.0")
            .env("REFRESH_RUSTD", "/nonexistent/refresh-test-rustd")
            .env("CARGO_REFRESH_SUPPORT", root.join("missing-support"));
        if invocation != "direct" {
            command.arg("refresh");
        }
        command
            .args([
                "run",
                "--bin",
                "watch-fixture",
                "--package",
                "watch-fixture",
                "--no-check",
                "--events",
                "src/../src/events.jsonl",
                "-v",
                "--",
                "--port",
                "8080",
            ])
            .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
            .stderr(Stdio::from(fs::File::create(&stderr).unwrap()));
        let mut running = Running {
            child: command.spawn().unwrap(),
        };
        wait_until(|| {
            if let Some(status) = running.child.try_wait().unwrap() {
                panic!(
                    "controller exited with {status}: {}",
                    fs::read_to_string(&stderr).unwrap()
                );
            }
            rows(&log).iter().any(|row| row["event"] == "ready")
        });
        // Compare resolved roots to the stock cargo metadata result for this exact fixture.
        let stock = Command::new("cargo")
            .current_dir(root)
            .env("RUSTUP_TOOLCHAIN", "1.99.0")
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .output()
            .unwrap();
        assert!(
            stock.status.success(),
            "{}",
            String::from_utf8_lossy(&stock.stderr)
        );
        let stock: Value = serde_json::from_slice(&stock.stdout).unwrap();
        let output = fs::read_to_string(&stdout).unwrap();
        let config: Value = serde_json::Deserializer::from_str(&output)
            .into_iter()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(config["package"], stock["packages"][0]["name"]);
        assert_eq!(config["bin"], stock["packages"][0]["targets"][0]["name"]);
        let source = PathBuf::from(
            stock["packages"][0]["targets"][0]["src_path"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(
            config["source_roots"],
            serde_json::json!([source.parent().unwrap()])
        );
        assert_eq!(config["strict"], true);
        assert_eq!(config["check"], false);
        assert_eq!(config["allow_tracing_metadata"], false);
        assert_eq!(config["after_patch_hook"], true);
        assert_eq!(config["edit"], serde_json::json!(["watch-fixture"]));
        assert_eq!(config["args"], serde_json::json!(["--port", "8080"]));
        assert_eq!(config["verbose"], 1);

        fs::write(&source, "fn main() { loop { println!(\"edited\"); std::thread::sleep(std::time::Duration::from_millis(10)); } }\n").unwrap();
        wait_until(|| rows(&log).iter().any(|row| row["path"] == "stock"));
        wait_until(|| fs::read_to_string(&stdout).unwrap().contains("edited"));
        let count = rows(&log).iter().filter(|r| r["event"] == "save").count();
        // This bound is a functional assertion against a recursive logging loop, not a latency claim.
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            rows(&log).iter().filter(|r| r["event"] == "save").count(),
            count,
            "event log triggered its own save events"
        );
        fs::write(&source, "fn main() { loop { println!(\"edited\"); std::thread::sleep(std::time::Duration::from_millis(10)); } }\n").unwrap();
        wait_until(|| rows(&log).iter().any(|row| row["path"] == "skip"));
        wait_until(|| fs::read_to_string(&stderr).unwrap().contains("· unchanged"));
        for row in rows(&log).iter().filter(|row| row["event"] == "save") {
            assert!(row["generation"].as_u64().unwrap() >= 1);
            assert!(
                row["phases"]["sources"].as_f64().unwrap()
                    >= row["phases"]["notice"].as_f64().unwrap()
            );
            assert!(row["revision"].as_u64().unwrap() > 0);
        }
    }
}

#[test]
fn status_stop_and_help_are_available_in_both_forms() {
    for cargo_style in [false, true] {
        for subcommand in ["status", "stop", "--help"] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-refresh"));
            command.env_remove("CARGO_TARGET_DIR");
            if cargo_style {
                command.arg("refresh");
            }
            let output = command.arg(subcommand).output().unwrap();
            assert!(output.status.success());
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(stdout.contains(if subcommand == "--help" {
                "Fast Refresh for Rust"
            } else {
                "not implemented yet"
            }));
        }
    }
}

#[test]
fn feature_selection_reaches_metadata_initial_and_stock_builds() {
    let directory = fixture();
    let root = directory.path();
    let manifest = root.join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[features]\ndefault = [\"unwanted\"]\nunwanted = []\nchosen = []\n");
    fs::write(manifest, text).unwrap();
    let source = root.join("src/main.rs");
    let initial = "#[cfg(any(feature = \"unwanted\", not(feature = \"chosen\")))] compile_error!(\"wrong feature selection\");\nfn main() { loop { println!(\"chosen-before\"); std::thread::sleep(std::time::Duration::from_millis(20)); } }\n";
    fs::write(&source, initial).unwrap();
    let events = root.join("events.jsonl");
    let stdout = root.join("stdout");
    let stderr = root.join("stderr");
    let mut running = Running {
        child: Command::new(env!("CARGO_BIN_EXE_cargo-refresh"))
            .env_remove("CARGO_TARGET_DIR")
            .current_dir(root)
            .env("RUSTUP_TOOLCHAIN", "1.99.0")
            .env("REFRESH_RUSTD", "/nonexistent/feature-test-rustd")
            .env("CARGO_REFRESH_SUPPORT", root.join("missing-support"))
            .args([
                "run",
                "--no-default-features",
                "--features=chosen",
                "--events",
            ])
            .arg(&events)
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    };
    wait_until(|| {
        assert!(
            running.child.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(&stderr).unwrap()
        );
        rows(&events).iter().any(|r| r["event"] == "ready")
    });
    wait_until(|| {
        fs::read_to_string(&stdout)
            .unwrap()
            .contains("chosen-before")
    });
    fs::write(&source, initial.replace("chosen-before", "chosen-after")).unwrap();
    wait_until(|| rows(&events).iter().any(|r| r["path"] == "stock"));
    wait_until(|| {
        fs::read_to_string(&stdout)
            .unwrap()
            .contains("chosen-after")
    });
    // The same edited source, using stock Cargo and the same selection.
    let stock = Command::new("cargo")
        .current_dir(root)
        .env("RUSTUP_TOOLCHAIN", "1.99.0")
        .args([
            "check",
            "--offline",
            "--no-default-features",
            "--features=chosen",
        ])
        .output()
        .unwrap();
    assert!(
        stock.status.success(),
        "{}",
        String::from_utf8_lossy(&stock.stderr)
    );
}

#[test]
fn zero_output_worker_routes_changed_source_to_stock_and_delivers_edit() {
    use std::os::unix::fs::PermissionsExt;
    let directory = fixture();
    let root = directory.path();
    let bin = root.join("tools");
    fs::create_dir(&bin).unwrap();
    let write_executable = |name: &str, source: &str| {
        let path = bin.join(name);
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let real_cargo = Command::new("rustup")
        .env("RUSTUP_TOOLCHAIN", "1.99.0")
        .args(["which", "cargo"])
        .output()
        .unwrap();
    assert!(real_cargo.status.success());
    let cargo = write_executable(
        "cargo",
        r#"#!/usr/bin/env python3
import json,os,pathlib,signal,subprocess,sys
args=sys.argv[1:]
if args[:2]==['rustd','status']:print(str(pathlib.Path.cwd())+': ready')
elif args[:2]==['rustd','stop']:
 p=pathlib.Path(os.environ['RUSTD_HOME'])/'owned.pid'
 if p.exists():
  try:os.kill(int(p.read_text()),signal.SIGTERM)
  except ProcessLookupError:pass
elif args[:2]==['rustd','build']:
 out=subprocess.check_output([os.environ['FIX1_REAL_CARGO'],'build','--offline','-j','4','--message-format=json'],stderr=sys.stderr)
 artifacts=[json.loads(line) for line in out.splitlines()]
 exe=next(x['executable'] for x in artifacts if x.get('executable'))
 p=pathlib.Path.cwd()/'target/rustd/fake/programs/watch_fixture-fake';p.mkdir(parents=True,exist_ok=True)
 import shutil
 shutil.copyfile(exe,p/'bin');(p/'bin').chmod(0o755)
else:sys.exit(subprocess.call([os.environ['FIX1_REAL_CARGO'],*args]))
"#,
    );
    let rustd = write_executable(
        "rustd",
        r#"#!/usr/bin/env python3
import os,pathlib,socket,struct,sys,time
home=pathlib.Path(os.environ['RUSTD_HOME'])
if sys.argv[1]=='daemon':
 (home/'workers').mkdir(exist_ok=True)
 a=socket.socket(socket.AF_UNIX);a.bind(str(home/'daemon.sock'))
 b=socket.socket(socket.AF_UNIX);b.bind(str(home/'workers/worker.sock'))
 (home/'owned.pid').write_text(str(os.getpid()))
 while True:time.sleep(.1)
elif sys.argv[1]=='sync':
 p=pathlib.Path.cwd()/'target/rustd/fake/programs/watch_fixture-fake'
 (p/'patch.manifest').write_bytes(b'RDM09001'+struct.pack('<Q',1)+bytes(40))
else:raise RuntimeError(sys.argv)
"#,
    );
    // The installer intentionally ignores REFRESH_RUSTD. Supply a complete,
    // checksummed local test package, keeping the production validation path.
    // Use an ELF launcher so runtime dependency validation also runs normally.
    let support = root.join("support");
    fs::create_dir(&support).unwrap();
    let launcher = root.join("launcher.c");
    fs::write(
        &launcher,
        r#"#include <stdlib.h>
#include <unistd.h>
int main(int argc, char **argv) {
    char **args = calloc((size_t)argc + 2, sizeof(char *));
    if (!args) return 99;
    args[0] = "python3";
    args[1] = getenv("FIX1_RUSTD_SCRIPT");
    for (int i = 1; i < argc; ++i) args[i + 1] = argv[i];
    execvp(args[0], args);
    return 99;
}
"#,
    )
    .unwrap();
    assert!(
        Command::new("cc")
            .arg(&launcher)
            .arg("-o")
            .arg(support.join("rustd"))
            .status()
            .unwrap()
            .success()
    );
    for shim in ["cargo-rustd", "rustd-rustc"] {
        fs::copy("/bin/true", support.join(shim)).unwrap();
    }
    fs::write(
        support.join("librefresh.so"),
        cargo_refresh::supervisor::AGENT,
    )
    .unwrap();
    fs::set_permissions(
        support.join("librefresh.so"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let commit = cargo_refresh::install::QUALIFIED_COMMIT;
    let target = cargo_refresh::install::TARGET;
    fs::write(support.join("VERSION"), format!(
        "cargo_refresh={}\nrustc_commit={commit}\ntarget={target}\ncargo_refresh_commit={commit}\nrustd_commit={commit}\nra_commit={commit}\nagent_static_libunwind=0\n",
        env!("CARGO_PKG_VERSION")
    )).unwrap();
    fs::write(
        support.join("abi-facts.bundle"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "commit_hash": commit, "target": target,
            "simd128": "indirect", "personality": "rust_eh_personality",
            "rustc_vv": format!("release: 1.99.0\ncommit-hash: {commit}\nhost: {target}\n")
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(support.join("RUNTIME.txt"), "test package\n").unwrap();
    fs::create_dir(support.join("licenses")).unwrap();
    for project in ["cargo-refresh", "rustd", "rust-analyzer"] {
        for license in ["MIT", "APACHE"] {
            fs::write(
                support.join(format!("licenses/{project}-LICENSE-{license}")),
                "test\n",
            )
            .unwrap();
        }
    }
    use sha2::{Digest, Sha256};
    fs::set_permissions(&support, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(support.join("licenses"), fs::Permissions::from_mode(0o700)).unwrap();
    let mut sums = String::new();
    for entry in walkdir::WalkDir::new(&support).min_depth(1) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            let name = entry.path().strip_prefix(&support).unwrap();
            let executable = ["rustd", "cargo-rustd", "rustd-rustc", "librefresh.so"]
                .iter()
                .any(|file| name == Path::new(file));
            fs::set_permissions(
                entry.path(),
                fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
            )
            .unwrap();
            let hash = Sha256::digest(fs::read(entry.path()).unwrap());
            sums.push_str(&format!("{hash:x}  {}\n", name.display()));
        }
    }
    fs::write(support.join("SHA256SUMS"), sums).unwrap();
    fs::set_permissions(
        support.join("SHA256SUMS"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let source = root.join("src/main.rs");
    let initial = "fn value() -> u64 { 1 }\nfn main() { loop { println!(\"value={}\", value()); std::thread::sleep(std::time::Duration::from_millis(20)); } }\n";
    fs::write(&source, initial).unwrap();
    let events = root.join("events.jsonl");
    let stdout = root.join("stdout");
    let stderr = root.join("stderr");
    let path = std::iter::once(bin)
        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))
        .collect::<Vec<_>>();
    let mut running = Running {
        child: Command::new(env!("CARGO_BIN_EXE_cargo-refresh"))
            .env_remove("CARGO_TARGET_DIR")
            .current_dir(root)
            .env("RUSTUP_TOOLCHAIN", "1.99.0")
            .env("PATH", std::env::join_paths(path).unwrap())
            .env("CARGO", cargo)
            .env(
                "FIX1_REAL_CARGO",
                String::from_utf8(real_cargo.stdout).unwrap().trim(),
            )
            .env("REFRESH_RUSTD", "/not/the/validated/compiler")
            .env("FIX1_RUSTD_SCRIPT", rustd)
            .env("CARGO_REFRESH_SUPPORT", support)
            .env("CARGO_REFRESH_WARM_PEAK_BYTES", "0")
            .env_remove("REFRESH_LANE_TOOL")
            .args(["run", "--no-check", "--events"])
            .arg(&events)
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    };
    wait_until(|| {
        assert!(
            running.child.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(&stderr).unwrap()
        );
        rows(&events).iter().any(|r| r["event"] == "ready")
    });
    wait_until(|| fs::read_to_string(&stdout).unwrap().contains("value=1"));
    fs::write(&source, initial.replace("{ 1 }", "{ 2 }")).unwrap();
    wait_until(|| rows(&events).iter().any(|r| r["event"] == "save"));
    let saves = rows(&events);
    let save = saves.iter().find(|r| r["event"] == "save").unwrap();
    assert_eq!(save["path"], "stock", "{saves:#?}");
    assert!(
        save["reason"]
            .as_str()
            .unwrap()
            .contains("no compiled change"),
        "{saves:#?}\n{}",
        fs::read_to_string(&stderr).unwrap()
    );
    assert!(saves.iter().any(|r| r["event"] == "compiler_no_change"));
    wait_until(|| fs::read_to_string(&stdout).unwrap().contains("value=2"));
    assert!(
        !rows(&events)
            .iter()
            .any(|r| r["path"] == "live" || r["path"] == "restart")
    );
}
