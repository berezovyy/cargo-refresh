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

lazy_static::lazy_static! { static ref VALUE: u64 = initial(); }
#[inline(never)]
fn initial() -> u64 {
    7
}
#[inline(never)]
fn probe() -> String {
    format!("v1:{}", *VALUE)
}
fn main() {
    lazy_static::initialize(&VALUE);
    serve(probe);
}
