fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let value = 1;
    std::fs::write(
        std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("generated.rs"),
        format!("fn generated() -> u64 {{ {value} }}"),
    )
    .unwrap();
}
