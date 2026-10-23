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
static DROPPED: AtomicU64 = AtomicU64::new(0);
struct Item;
impl Drop for Item {
    #[inline(never)]
    fn drop(&mut self) {
        DROPPED.store(1, Ordering::SeqCst);
    }
}
fn main() {
    let mut alive: Vec<Box<Item>> = (0..4).map(|_| Box::new(Item)).collect();
    serve(|| {
        drop(alive.pop().expect("four probes maximum"));
        DROPPED.load(Ordering::SeqCst).to_string()
    });
}
