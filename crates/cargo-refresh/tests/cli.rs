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
            .env("REFRESH_RUSTD", "/nonexistent/refresh-test-rustd");
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
