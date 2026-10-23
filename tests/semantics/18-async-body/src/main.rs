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
async fn changed() -> u64 {
    std::future::ready(()).await;
    1
}
fn main() {
    serve(|| futures::executor::block_on(changed()).to_string());
}
