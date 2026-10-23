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
fn changed() {
    panic!("panic-v1");
}
fn probe() -> String {
    let e = std::panic::catch_unwind(changed).unwrap_err();
    format!("caught={}", e.downcast_ref::<&str>().unwrap())
}
fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    serve(probe);
}
