use std::{env, path::PathBuf, process::Command};
fn main() {
    let agent = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../agent");
    println!("cargo:rerun-if-changed={}", agent.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        agent.join("Makefile").display()
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("agent");
    let status = Command::new("make")
        .arg("-C")
        .arg(&agent)
        .arg(format!("BUILD={}", out.display()))
        // The production agent excludes the test-only libunwind CFI walker.
        .arg("LDLIBS=-Wl,-z,now -ldl -lpthread")
        .arg("all")
        .status()
        .expect("build bundled agent with make");
    assert!(status.success(), "bundled refresh agent build failed");
    println!(
        "cargo:rustc-env=REFRESH_BUNDLED_AGENT={}",
        out.join("librefresh.so").display()
    );
}
