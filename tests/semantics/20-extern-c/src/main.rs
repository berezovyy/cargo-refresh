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

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn exported() -> u64 {
    1
}
unsafe extern "C" {
    fn c_call(f: extern "C" fn() -> u64) -> u64;
}
fn main() {
    let saved: extern "C" fn() -> u64 = exported;
    serve(|| format!("c={};rust={}", unsafe { c_call(saved) }, exported()));
}
