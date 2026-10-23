use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

mod patched;

fn respond(mut socket: TcpStream) {
    let mut request = [0; 1024];
    let n = socket.read(&mut request).unwrap();
    let trigger = request[..n].starts_with(b"GET /panic ");
    if trigger {
        std::io::stderr().write_all(b"ordinary-before\xff\0\r\n").unwrap();
    }
    let value = std::panic::catch_unwind(|| patched::panic_site(trigger)).unwrap_or(999);
    if trigger {
        std::io::stderr().write_all(b"ordinary-after\xfe\r\n").unwrap();
    }
    let body = value.to_string();
    write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
}

fn main() {
    let port = std::env::var("PORT").unwrap();
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    for socket in listener.incoming() { respond(socket.unwrap()); }
}
