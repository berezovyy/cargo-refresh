use axum::{Router, routing::get};

struct Value { number: u64 }

fn value() -> u64 { Value { number: 1 }.number }

async fn handler() -> String { value().to_string() }

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "19551".into());
    let socket = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
    axum::serve(socket, Router::new().route("/", get(handler))).await.unwrap();
}
