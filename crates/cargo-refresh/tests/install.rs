//! Hand-run release integration; no daemon is started by normal cargo test.
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires a built release archive; runs an owned rustd session in the resource lane"]
fn archive_first_run_live_save_matches_stock() {
    let archive = std::env::var_os("CARGO_REFRESH_TEST_ARCHIVE")
        .map(PathBuf::from)
        .expect(
            "set CARGO_REFRESH_TEST_ARCHIVE to the release .tar.zst (adjacent .sha256 required)",
        );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lane = std::env::var_os("CARGO_REFRESH_TEST_LANE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../rust-fast-compiler/tools/lane.sh"));
    assert!(
        lane.is_file(),
        "set CARGO_REFRESH_TEST_LANE to tools/lane.sh"
    );
    let work = tempfile::tempdir().unwrap();
    let status = Command::new(lane)
        .args(["rustd", "--", "python3"])
        .arg(root.join("scripts/run-guarded.py"))
        .arg("python3")
        .arg(root.join("scripts/integration-install.py"))
        .args([
            "--controller",
            env!("CARGO_BIN_EXE_cargo-refresh"),
            "--archive",
        ])
        .arg(archive)
        .arg("--work")
        .arg(work.path().join("run"))
        .status()
        .unwrap();
    assert!(status.success(), "archive first-run integration failed");
}
