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

trait Value {
    fn value(&self) -> u64;
}
struct Item;
impl Value for Item {
    #[inline(never)]
    fn value(&self) -> u64 {
        1
    }
}
fn main() {
    let old: Box<dyn Value> = Box::new(Item);
    let saved: fn(&Item) -> u64 = <Item as Value>::value;
    serve(|| {
        let new: Box<dyn Value> = Box::new(Item);
        format!(
            "old={};new={};saved={};direct={}",
            old.value(),
            new.value(),
            saved(&Item),
            Item.value()
        )
    });
}
