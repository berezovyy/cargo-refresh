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

#[inline(never)]
async fn query(db: &mut sqlx::SqliteConnection) -> i32 {
    sqlx::query!("SELECT 1 AS value")
        .fetch_one(db)
        .await
        .unwrap()
        .value
}
fn main() {
    use sqlx::Connection;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut db = rt
        .block_on(sqlx::SqliteConnection::connect(":memory:"))
        .unwrap();
    serve(|| rt.block_on(query(&mut db)).to_string());
}
