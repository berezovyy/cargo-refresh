// S6_NOOP
mod workload;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use std::time::Duration;
fn literal_probe() -> &'static str { "S1:v1" }
fn leaf_logic(x: u64) -> u64 { x + 1 /* S2_LOGIC */ }
fn handle(mut stream: TcpStream, ticks: u64, checksum: u64) {
    stream.set_read_timeout(Some(Duration::from_millis(200))).ok();
    stream.set_write_timeout(Some(Duration::from_millis(200))).ok();
    let mut request = [0_u8; 2048];
    let n = match stream.read(&mut request) { Ok(n) => n, Err(_) => return };
    let probe = request[..n].starts_with(b"GET /__probe ");
    let (status, body) = if probe {
        ("200 OK", format!("probe={}|S2:v{}|{}|{}|{}|{}\nticks={}\npid={}\nchecksum={}\n",
            literal_probe(), leaf_logic(0), workload::deep_probe(), workload::signature_probe(),
            workload::derive_probe(), workload::new_fn_probe(), ticks, std::process::id(), checksum))
    } else { ("404 Not Found", "not found\n".to_owned()) };
    let response = format!("HTTP/1.1 {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, body.len(), body);
    stream.write_all(response.as_bytes()).ok();
}
fn main() -> std::io::Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "18083".to_owned());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))?;
    let ticks = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&ticks);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(10));
        clock.fetch_add(1, Ordering::Relaxed);
    });
    // Exercise all generated code once, keeping the request path cheap.
    let checksum = workload::workload();
    for stream in listener.incoming() {
        if let Ok(stream) = stream { handle(stream, ticks.load(Ordering::Relaxed), checksum); }
    }
    Ok(())
}
