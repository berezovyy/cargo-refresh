use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=src/assertion_printer_driver.rs");
    if env::var_os("CARGO_FEATURE_RUSTC_ASSERT").is_none() {
        return;
    }
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let identity = Command::new(&rustc).arg("-Vv").output().expect("rustc identity");
    assert!(identity.status.success());
    let identity = String::from_utf8(identity.stdout).unwrap();
    let commit = identity.lines().find_map(|l| l.strip_prefix("commit-hash: ")).unwrap();
    assert_eq!(
        commit, "b940084d7eb6a299eb4bfeb8e34901bc051e7ac4",
        "assertion printer requires qualified rustc 1.99.0 + rustc-dev"
    );
    println!("cargo:rustc-env=RUSTD_ASSERT_RUSTC_COMMIT={commit}");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("assertion-printer");
    let mut command = Command::new(rustc);
    // Preserve the target linker's libc baseline in portable release builds.
    if let Some(linker) = env::var_os("RUSTC_LINKER") {
        command.arg("-C").arg(format!("linker={}", linker.to_string_lossy()));
    }
    let status = command
        .env("RUSTC_BOOTSTRAP", "1")
        .args(["--edition=2024", "-Copt-level=2", "-Cdebuginfo=0", "-Crpath"])
        .arg("src/assertion_printer_driver.rs")
        .arg("-o")
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success(), "building the exact-toolchain assertion printer");
}
