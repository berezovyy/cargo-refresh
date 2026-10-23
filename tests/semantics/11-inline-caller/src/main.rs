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

#[inline(always)]
fn leaf(x: u64) -> u64 {
    x + 1
}
#[inline(never)]
fn left(x: u64) -> u64 {
    leaf(x)
}
#[inline(never)]
fn right(x: u64) -> u64 {
    leaf(x) * 2
}
fn main() {
    serve(|| {
        let x = std::hint::black_box(10);
        format!("{},{}", left(x), right(x))
    });
}
