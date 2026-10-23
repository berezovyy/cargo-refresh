use axum::{Router, routing::get};
struct Value;
impl stock_dispatch::Label for Value {
    fn label(&self) -> u64 { 1 }
}
fn owned() -> u64 { 10 }
async fn owned_handler() -> String { owned().to_string() }
async fn handler() -> String { stock_dispatch::dispatch(&Value).to_string() }
#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "19551".into());
    let socket = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
    axum::serve(socket, Router::new().route("/", get(handler)).route("/owned", get(owned_handler))).await.unwrap();
}
