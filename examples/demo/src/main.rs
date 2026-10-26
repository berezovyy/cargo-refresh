use std::{thread, time::Duration};

struct State {
    ticks: u64,
    total: u64,
}

// Try editing these function bodies while the program runs.
fn message() -> &'static str {
    "hello from version 1"
}

fn step(n: u64) -> u64 {
    n * 1
}

fn main() {
    let mut state = State { ticks: 0, total: 0 };
    println!("demo started, pid {}", std::process::id());
    let once = std::env::var_os("DEMO_ONCE").is_some();
    let delay = if std::env::var_os("DEMO_FAST").is_some() { 20 } else { 1000 };
    loop {
        state.ticks += 1;
        state.total += step(state.ticks);
        println!(
            "tick {:>4} | total {:>8} | {}",
            state.ticks,
            state.total,
            message()
        );
        if once { break; }
        thread::sleep(Duration::from_millis(delay));
    }
}
