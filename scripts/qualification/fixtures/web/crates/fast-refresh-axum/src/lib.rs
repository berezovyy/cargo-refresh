//! Development-only request admission. Tokens can move across executor threads.
use axum::{body::{Body,to_bytes}, extract::Request, middleware::Next, response::{Response,IntoResponse}, http::StatusCode};
use std::sync::OnceLock;
unsafe extern "C" { fn dlsym(handle:*mut std::ffi::c_void,name:*const std::ffi::c_char)->*mut std::ffi::c_void; }
struct Api { enter:unsafe extern "C" fn()->i32, leave:unsafe extern "C" fn() }
static API:OnceLock<Option<Api>>=OnceLock::new();
fn api()->Option<&'static Api> { API.get_or_init(||unsafe {
 let a=dlsym(std::ptr::null_mut(),c"f080_try_enter".as_ptr());let b=dlsym(std::ptr::null_mut(),c"f080_leave".as_ptr());
 if a.is_null()||b.is_null(){None}else{Some(Api{enter:std::mem::transmute(a),leave:std::mem::transmute(b)})}
}).as_ref() }
pub struct Token(bool);
impl Token { pub async fn enter()->Self {
 if let Some(a)=api(){while unsafe{(a.enter)()}==0 {tokio::time::sleep(std::time::Duration::from_millis(1)).await;}Self(true)}else{Self(false)}
} }
impl Drop for Token {fn drop(&mut self){if self.0 {unsafe{(api().unwrap().leave)()}}}}
pub async fn buffered(req:Request,next:Next)->Response {
 match tokio::time::timeout(std::time::Duration::from_secs(1),async {
  let response=next.run(req).await;let (parts,body)=response.into_parts();
  match to_bytes(body,64*1024*1024).await {Ok(bytes)=>Response::from_parts(parts,Body::from(bytes)),Err(_)=>StatusCode::PAYLOAD_TOO_LARGE.into_response()}
 }).await {Ok(response)=>response,Err(_)=>StatusCode::GATEWAY_TIMEOUT.into_response()}
}

/// Middleware for applications without a checkpoint callback.
pub async fn middleware(req:Request,next:Next)->Response {
    let _token=Token::enter().await;
    buffered(req,next).await
}

#[cfg(test)]
mod tests {
 use super::*;
 use axum::{Router,routing::get};
 use tower::ServiceExt;
 #[tokio::test(flavor="multi_thread",worker_threads=2)]
 async fn normal_response_and_cancellation() {
  let app=Router::new().route("/ok",get(||async{"ok"}))
   .route("/slow",get(||async{std::future::pending::<String>().await}))
   .layer(axum::middleware::from_fn(middleware));
  let response=app.clone().oneshot(Request::builder().uri("/ok").body(Body::empty()).unwrap()).await.unwrap();
  assert_eq!(response.status(),StatusCode::OK);
  assert_eq!(&to_bytes(response.into_body(),1024).await.unwrap()[..],b"ok");
  let response=app.clone().oneshot(Request::builder().uri("/slow").body(Body::empty()).unwrap()).await.unwrap();
  assert_eq!(response.status(),StatusCode::GATEWAY_TIMEOUT);
  let task=tokio::spawn(app.oneshot(Request::builder().uri("/slow").body(Body::empty()).unwrap()));
  tokio::time::sleep(std::time::Duration::from_millis(10)).await;task.abort();assert!(task.await.unwrap_err().is_cancelled());
 }
 #[tokio::test]
 async fn delayed_body_is_consumed_inside_request() {
  use std::{pin::Pin,task::{Context,Poll},future::Future};
  struct Delayed { timer:Pin<Box<tokio::time::Sleep>>, sent:bool }
  impl futures_core::Stream for Delayed {
   type Item=Result<axum::body::Bytes,std::io::Error>;
   fn poll_next(mut self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<Option<Self::Item>> {
    if self.sent{return Poll::Ready(None)}
    if self.timer.as_mut().poll(cx).is_pending(){return Poll::Pending}
    self.sent=true;Poll::Ready(Some(Ok(axum::body::Bytes::from_static(b"complete"))))
   }
  }
  let app=Router::new().route("/body",get(||async{Body::from_stream(Delayed{timer:Box::pin(tokio::time::sleep(std::time::Duration::from_millis(40))),sent:false})}))
   .layer(axum::middleware::from_fn(middleware));
  let response=app.oneshot(Request::builder().uri("/body").body(Body::empty()).unwrap()).await.unwrap();
  assert_eq!(to_bytes(response.into_body(),1024).await.unwrap(),"complete");
 }
}

#[cfg(test)]
mod gate_tests {
 use super::*;
 unsafe fn symbol<T:Copy>(name:&std::ffi::CStr)->Option<T>{
  let p=unsafe{dlsym(std::ptr::null_mut(),name.as_ptr())};
  if p.is_null(){None}else{Some(unsafe{std::mem::transmute_copy(&p)})}
 }
 #[tokio::test(flavor="multi_thread",worker_threads=2)]
 async fn token_tracks_migration_waiting_and_abort() {
  if std::env::var_os("F080_TESTING").is_none(){return}
  let count:unsafe extern "C" fn()->u32=unsafe{symbol(c"f080_test_inflight").expect("test preload")};
  let close:unsafe extern "C" fn()->i32=unsafe{symbol(c"f080_test_close").unwrap()};
  let open:unsafe extern "C" fn()=unsafe{symbol(c"f080_test_open").unwrap()};
  let token=Token::enter().await;assert_eq!(unsafe{count()},1);
  let writer=tokio::task::spawn_blocking(move ||unsafe{close()});
  tokio::time::sleep(std::time::Duration::from_millis(30)).await;
  let pending=tokio::spawn(Token::enter());
  tokio::time::sleep(std::time::Duration::from_millis(10)).await;
  assert!(!pending.is_finished());assert_eq!(unsafe{count()},1);
  tokio::spawn(async move{tokio::task::yield_now().await;drop(token)}).await.unwrap();
  assert_eq!(writer.await.unwrap(),0);assert_eq!(unsafe{count()},0);
  unsafe{open()};let acquired=pending.await.unwrap();assert_eq!(unsafe{count()},1);drop(acquired);
  let task=tokio::spawn(async {let _token=Token::enter().await;std::future::pending::<()>().await});
  tokio::time::sleep(std::time::Duration::from_millis(10)).await;assert_eq!(unsafe{count()},1);
  task.abort();assert!(task.await.unwrap_err().is_cancelled());assert_eq!(unsafe{count()},0);
 }
}
