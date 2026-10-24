use std::sync::atomic::Ordering;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    use statearena::Persist;
    if std::env::var_os("FAST_REFRESH_SCHEMA_ONLY").is_some() {
        println!("{}",serde_json::to_string(&store::Snapshot::schema())?);return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "web_server=info,tower_http=warn".into()),
        )
        .json()
        .init();
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "18081".into())
        .parse()?;
    let state = api::AppState::default();
    let arena = std::env::var_os("STATE_ARENA").map(|p| statearena::Arena::open(p).unwrap());
    let mut arena = arena;
    if let Some(a)=arena.as_mut() {
        if let Some((saved,_))=a.restore::<store::Snapshot>().unwrap() {
            if let Some(path)=std::env::var_os("STATE_AUDIT_PATH") { std::fs::write(path,serde_json::to_vec(&saved.encode())?)?; }
            state.ticks.store(saved.ticks,Ordering::Relaxed);state.db.restore(saved).await;
        }
        a.save(&state.db.snapshot(state.ticks.load(Ordering::Relaxed)).await).unwrap();
    }
    let arena=std::sync::Arc::new(tokio::sync::Mutex::new(arena));
    let transactions=std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let counter = state.ticks.clone();
    let ticker = tokio::spawn(async move {
        let mut clock = tokio::time::interval(std::time::Duration::from_millis(10));
        clock.tick().await; // Consume the immediate tick; first increment follows 10 ms later.
        loop {
            clock.tick().await;
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });

    tracing::info!(port, "listening on loopback");
    let checkpoint_state=state.clone();
    let app=api::router(state).layer(axum::middleware::from_fn(move |req, next:axum::middleware::Next| {
        let state=checkpoint_state.clone();let arena=arena.clone();let transactions=transactions.clone();
        async move {
            // Serialize full request+checkpoint transactions in this prototype.
            let _transaction=transactions.lock().await;
            let response=fast_refresh_axum::buffered(req,next).await;
            if let Some(a)=arena.lock().await.as_mut() {
                a.save(&state.db.snapshot(state.ticks.load(Ordering::Relaxed)).await).unwrap();
            }
            response
        }
    }));
    if let Some(path)=std::env::var_os("FAST_REFRESH_TEST_REQUESTS") {
        use tower::ServiceExt;
        let requests:Vec<serde_json::Value>=serde_json::from_slice(&std::fs::read(path)?)?;
        let mut responses=Vec::new();
        for r in requests {
            let request=axum::http::Request::builder().method(r["method"].as_str().unwrap()).uri(r["path"].as_str().unwrap())
                .header("content-type","application/json")
                .body(axum::body::Body::from(r.get("body").map(|v|v.to_string()).unwrap_or_default()))?;
            let response=app.clone().oneshot(request).await.unwrap();let status=response.status().as_u16();
            let body=axum::body::to_bytes(response.into_body(),64*1024*1024).await?;
            let text=String::from_utf8(body.to_vec())?;
            responses.push(serde_json::json!({"status":status,"body":serde_json::from_str::<serde_json::Value>(&text).unwrap_or(serde_json::Value::String(text))}));
        }
        println!("{}",serde_json::to_string(&responses)?);ticker.abort();return Ok(());
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    ticker.abort();
    result?;
    Ok(())
}
