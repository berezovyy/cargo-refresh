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

static mut COUNTER: u64 = 10;
#[inline(never)]
fn probe() -> String {
    let delta = 1;
    unsafe {
        COUNTER += delta;
        let value = COUNTER;
        value.to_string()
    }
}
fn main() {
    serve(probe);
}
