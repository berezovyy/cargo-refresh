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

use std::{cell::Cell, sync::mpsc, thread};
thread_local! { static VALUE: Cell<u64> = const { Cell::new(0) }; }
#[inline(never)]
fn tick() -> u64 {
    let delta = 1;
    VALUE.with(|v| {
        v.set(v.get() + delta);
        v.get()
    })
}
fn main() {
    let mut workers = Vec::new();
    for seed in [100, 200] {
        let (tx, rx) = mpsc::channel();
        let (reply, result) = mpsc::channel();
        thread::spawn(move || {
            VALUE.with(|v| v.set(seed));
            while rx.recv().is_ok() {
                reply.send(tick()).unwrap();
            }
        });
        workers.push((tx, result));
    }
    serve(|| {
        let mut values = Vec::new();
        for (tx, rx) in &workers {
            tx.send(()).unwrap();
            values.push(rx.recv().unwrap().to_string());
        }
        values.join(",")
    });
}
