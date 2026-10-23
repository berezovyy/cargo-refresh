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

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
};
#[derive(Clone)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
static LINE: AtomicU32 = AtomicU32::new(0);
struct Capture;
impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Capture {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _: &tracing::Id,
        _: Context<'_, S>,
    ) {
        LINE.store(attrs.metadata().line().unwrap(), Ordering::SeqCst);
    }
}
// EDIT: insert above instrument
#[tracing::instrument]
#[inline(never)]
fn changed() -> u64 {
    1
}
fn main() {
    let bytes = Buffer(Arc::new(Mutex::new(Vec::new())));
    let output = bytes.clone();
    let fmt = tracing_subscriber::fmt::layer()
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        .without_time()
        .with_ansi(false)
        .with_writer(move || output.clone());
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("trace"))
            .with(Capture)
            .with(fmt),
    )
    .unwrap();
    serve(|| {
        bytes.0.lock().unwrap().clear();
        let value = changed();
        let formatted = String::from_utf8(bytes.0.lock().unwrap().clone()).unwrap();
        let (_, location) = formatted
            .split_once("main.rs:")
            .expect("fmt emits file:line");
        let fmt_line: u32 = location
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap();
        format!(
            "value={};line={};fmt-line={}",
            value,
            LINE.load(Ordering::SeqCst),
            fmt_line
        )
    });
}
