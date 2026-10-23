use axum::{Router, routing::get, extract::Path, body::{Body, Bytes}, response::Response};
use std::{sync::{Mutex, atomic::{AtomicU64, Ordering}}, future::Future, pin::Pin, time::Duration};
static NEXT: AtomicU64 = AtomicU64::new(0);
static RELEASE: AtomicU64 = AtomicU64::new(0);
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn record(id:u64, label:&str, value:String) { EVENTS.lock().unwrap().push(format!("{id}:{label}:{value}")); }
#[inline(never)]
fn helper() -> &'static str { "helper-v1" }
async fn wait_timer(id:u64) { while RELEASE.load(Ordering::SeqCst)<id { tokio::time::sleep(Duration::from_millis(20)).await; } }
pub fn routes() -> Router<crate::AppState> {
 Router::new().route("/async/start/{kind}", get(start)).route("/async/release/{id}",get(release)).route("/async/status",get(status)).route("/async/stream",get(stream))
}
fn launch(id:u64, kind:&str, fut:Pin<Box<dyn Future<Output=()> + Send>>) -> String {
 let raw: (*const (), *const usize) = unsafe { std::mem::transmute(&*fut as &(dyn Future<Output=()>+Send)) };
 let (dropfn,pollfn)=unsafe {(*raw.1, *raw.1.add(3))};
 let msg=format!("id={id},kind={kind},size={},poll={pollfn:x},drop={dropfn:x}",std::mem::size_of_val(&*fut));
 record(id,"launch",msg.clone()); tokio::spawn(fut); msg
}
async fn start(Path(kind):Path<String>) -> String {
 let id=NEXT.fetch_add(1,Ordering::SeqCst)+1;
 let f:Pin<Box<dyn Future<Output=()>+Send>>=match kind.as_str() {
 "timer"=>Box::pin(timer_task(id)), "select"=>Box::pin(select_task(id)), "background"=>Box::pin(background_task(id)), _=>return "unknown".into() };
 launch(id,&kind,f)
}
async fn release(Path(id):Path<u64>) -> String { RELEASE.store(id,Ordering::SeqCst); "released".into() }
async fn status() -> String { EVENTS.lock().unwrap().join("\n") }
async fn stream() -> Response {
 let id=NEXT.fetch_add(1,Ordering::SeqCst)+1;
 let (tx,rx)=tokio::sync::mpsc::channel::<String>(8);
 launch(id,"stream",Box::pin(stream_task(id,tx)));
 let stream=futures_util::stream::unfold(rx,|mut rx| async move {rx.recv().await.map(|s|(Ok::<Bytes,std::convert::Infallible>(Bytes::from(s)),rx))});
 Response::new(Body::from_stream(stream))
}

async fn timer_task(id:u64) {
    let carried = String::from("before-v1"); // BEFORE
    // EXTRA_LOCAL
    record(id,"pending",format!("timer:{}",carried));
    wait_timer(id).await; // MAIN_WAIT
    // EXTRA_AWAIT
    record(id,"done",format!("timer:{}:{}:{}:{}",carried,"after-v1",helper(),"loop-v1")); // AFTER
}

async fn stream_task(id:u64, tx:tokio::sync::mpsc::Sender<String>) {
    let carried = String::from("before-v1"); // BEFORE
    // EXTRA_LOCAL
    record(id,"pending",format!("stream:{}",carried));
    tx.send(format!("id={id}:head\n")).await.unwrap();
    wait_timer(id).await; // MAIN_WAIT
    // EXTRA_AWAIT
    record(id,"done",format!("stream:{}:{}:{}:{}",carried,"after-v1",helper(),"loop-v1")); // AFTER
    tx.send(format!("id={id}:tail:{}:{}\n",carried,helper())).await.unwrap();
}

async fn select_task(id:u64) {
    for turn in 0..2 { // SELECT_LOOP
    let carried = String::from("before-v1"); // BEFORE
    // EXTRA_LOCAL
    record(id,"pending",format!("select:{}",carried));
    tokio::select! { _=wait_timer(id)=>{}, _=std::future::pending::<()>()=>{} }; // MAIN_WAIT
    // EXTRA_AWAIT
    record(id,"done",format!("select:{}:{}:{}:{}",carried,"after-v1",helper(),"loop-v1")); // AFTER
    record(id,"turn",turn.to_string());
    }
}

async fn background_task(id:u64) {
    let mut clock=tokio::time::interval(Duration::from_millis(20));
    for turn in 0..2 {
        clock.tick().await;
    let carried = String::from("before-v1"); // BEFORE
    // EXTRA_LOCAL
    record(id,"pending",format!("background:{}",carried));
    wait_timer(id).await; // MAIN_WAIT
    // EXTRA_AWAIT
    record(id,"done",format!("background:{}:{}:{}:{}",carried,"after-v1",helper(),"loop-v2")); // AFTER
    record(id,"turn",turn.to_string());
    }
}
