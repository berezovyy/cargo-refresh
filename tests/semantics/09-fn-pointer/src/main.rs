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

#[inline(never)]
fn value() -> u64 {
    1
}
fn main() {
    let saved: fn() -> u64 = value;
    serve(|| format!("saved={};direct={}", saved(), value()));
}
