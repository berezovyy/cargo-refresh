use std::io::{self, BufRead, Write};
fn serve(mut probe: impl FnMut() -> String) {
    for line in io::stdin().lock().lines() {
        let line = line.expect("stdin");
        match line.as_str() {
            "probe" => {
                println!("probe={}", probe());
                io::stdout().flush().unwrap();
            }
            "quit" => break,
            _ => panic!("expected probe or quit"),
        }
    }
}

use std::sync::{LazyLock, OnceLock};
static ONCE: OnceLock<u64> = OnceLock::new();
static LAZY: LazyLock<u64> = LazyLock::new(initial);
static EXTERNAL: once_cell::sync::Lazy<u64> = once_cell::sync::Lazy::new(initial);
#[inline(never)]
fn initial() -> u64 {
    7
}
#[inline(never)]
fn probe() -> String {
    format!("v1:{},{},{}", ONCE.get().unwrap(), *LAZY, *EXTERNAL)
}
fn main() {
    ONCE.get_or_init(initial);
    LazyLock::force(&LAZY);
    once_cell::sync::Lazy::force(&EXTERNAL);
    serve(probe);
}
