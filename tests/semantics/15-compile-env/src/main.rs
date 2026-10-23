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
fn probe() -> String {
    format!(
        "{}:{}",
        env!("M4_SEMANTICS_REQUIRED"),
        option_env!("M4_SEMANTICS_OPTIONAL").unwrap_or("missing")
    )
}
fn main() {
    serve(probe);
}
