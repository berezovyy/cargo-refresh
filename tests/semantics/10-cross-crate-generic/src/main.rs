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

fn main() {
    serve(|| {
        format!(
            "app={},{};caller={}",
            generic_lib::value::<u8>(),
            generic_lib::value::<u64>(),
            caller::values()
        )
    });
}
