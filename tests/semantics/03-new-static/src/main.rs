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

use std::sync::atomic::{AtomicU64, Ordering};
#[inline(never)]
fn probe() -> String {
    // EDIT: add static
    "absent".to_string()
}
fn main() {
    serve(probe);
}
