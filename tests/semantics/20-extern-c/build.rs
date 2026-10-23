fn main() {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    assert!(
        std::process::Command::new("cc")
            .args(["-c", "caller.c", "-o"])
            .arg(out.join("caller.o"))
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("ar")
            .arg("crs")
            .arg(out.join("libcaller.a"))
            .arg(out.join("caller.o"))
            .status()
            .unwrap()
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=caller");
    println!("cargo:rerun-if-changed=caller.c");
}
