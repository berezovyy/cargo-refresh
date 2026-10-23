// STUB_HEADER
//! Small dependency-free fixture. It deliberately uses real HTTP and process state.
use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Shape {
    pub width: u32,
}

struct State {
    ticks: AtomicU64,
    requests: AtomicU64,
    started: Instant,
}

impl State {
    fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            started: Instant::now(),
        }
    }

    fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::Relaxed);
    }

    fn tick_count(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    fn request_count(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }
}

fn leaf_literal() -> &'static str {
    "S1:v2"
}

fn leaf_logic(value: u32) -> u32 {
    value + 1 // S2_EDIT
}

fn probe_value() -> String {
    format!(
        "{}|S2:{}|S4:{}",
        leaf_literal(),
        leaf_logic(41),
        size_of::<Shape>()
    )
}

fn probe_body(state: &State) -> String {
    format!(
        "probe={}\nticks={}\npid={}\n",
        probe_value(),
        state.tick_count(),
        std::process::id(),
    )
}

fn stdout_probe(state: &State) -> String {
    format!(
        "PROBE probe={} ticks={} pid={}",
        probe_value(),
        state.tick_count(),
        std::process::id(),
    )
}

fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

fn metrics_body(state: &State) -> String {
    format!(
        "{{\"probe\":\"{}\",\"ticks\":{},\"requests\":{},\"uptime_ms\":{}}}\n",
        json_escape(&probe_value()),
        state.tick_count(),
        state.request_count(),
        state.started.elapsed().as_millis(),
    )
}

fn route(path: &str, state: &State) -> (&'static str, &'static str, String) {
    match path {
        "/__probe" => ("200 OK", "text/plain", probe_body(state)),
        "/health" => ("200 OK", "text/plain", "ok\n".to_string()),
        "/metrics" => ("200 OK", "application/json", metrics_body(state)),
        "/" => (
            "200 OK",
            "text/plain",
            format!("Bench stub: {}\n", probe_value()),
        ),
        _ => ("404 Not Found", "text/plain", "not found\n".to_string()),
    }
}

fn response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    )
}

fn serve(mut stream: TcpStream, state: &State) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let mut line = String::new();
    let mut reader = BufReader::new(stream.try_clone()?);
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or("/");
    // Consume headers so closing the connection does not reset unread incoming data.
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" {
            break;
        }
    }
    state.record_request();
    let (status, content_type, body) = route(path, state);
    stream.write_all(response(status, content_type, &body).as_bytes())?;
    stream.flush()
}

fn start_clock(state: Arc<State>) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(1));
            state.tick();
        }
    });
}

fn start_stdout(state: Arc<State>) {
    thread::spawn(move || {
        loop {
            println!("{}", stdout_probe(&state));
            let _ = io::stdout().flush();
            thread::sleep(Duration::from_millis(10));
        }
    });
}

fn port() -> Result<u16, Box<dyn std::error::Error>> {
    Ok(env::var("PORT")
        .unwrap_or_else(|_| "18080".to_string())
        .parse()?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(State::new());
    start_clock(Arc::clone(&state));
    if env::var("STUB_STDOUT").as_deref() == Ok("1") {
        start_stdout(Arc::clone(&state));
    }
    let address = format!("127.0.0.1:{}", port()?);
    let listener = TcpListener::bind(&address)?;
    eprintln!("bench-stub listening on {address}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = serve(stream, &state) {
                    eprintln!("request error: {error}");
                }
            }
            Err(error) => eprintln!("accept error: {error}"),
        }
    }
    Ok(())
}
