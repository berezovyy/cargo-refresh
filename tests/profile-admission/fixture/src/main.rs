use std::io::{self, BufRead, Write};
use std::hint::black_box;
#[inline(never)]
fn version() -> u32 { 1 }
fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    for line in io::stdin().lock().lines() {
        match line.unwrap().as_str() {
            "probe" => {
                let debug = std::panic::catch_unwind(|| debug_assert!(black_box(false))).is_err();
                let overflow = std::panic::catch_unwind(|| black_box(u8::MAX) + black_box(1u8)).is_err();
                println!("probe=version={};cfg={};debug={};overflow={}", version(), cfg!(debug_assertions), debug, overflow);
                io::stdout().flush().unwrap();
            }
            "quit" => break,
            _ => panic!("expected probe"),
        }
    }
}
