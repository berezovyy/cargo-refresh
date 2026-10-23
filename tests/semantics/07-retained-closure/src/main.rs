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
fn make() -> Box<dyn Fn() -> String> {
    let captured = String::from("captured");
    Box::new(move || format!("{}:{}", captured, 1))
}
fn main() {
    let retained = make();
    serve(|| format!("old={};new={}", retained(), make()()));
}
