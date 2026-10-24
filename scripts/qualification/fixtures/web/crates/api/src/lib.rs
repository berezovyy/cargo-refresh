//! HTTP application boundary: routers, shared state, middleware and probe.
pub mod error;
pub mod handlers;
pub mod helpers;

use axum::{Router, routing::get};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer, trace::TraceLayer};

#[derive(Clone, Default)]
pub struct AppState {
    pub db: store::Database,
    pub ticks: Arc<AtomicU64>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/__probe", get(probe))
        .route("/health", get(|| async { "ok" }))
        .route(
            "/v1/projects",
            get(handlers::projects::list).post(handlers::projects::create),
        )
        .route("/v1/projects/summary", get(handlers::projects::summary))
        .route(
            "/v1/projects/{id}",
            get(handlers::projects::get)
                .patch(handlers::projects::update)
                .delete(handlers::projects::delete),
        )
        .route(
            "/v1/tickets",
            get(handlers::tickets::list).post(handlers::tickets::create),
        )
        .route("/v1/tickets/summary", get(handlers::tickets::summary))
        .route(
            "/v1/tickets/{id}",
            get(handlers::tickets::get)
                .patch(handlers::tickets::update)
                .delete(handlers::tickets::delete),
        )
        .route(
            "/v1/customers",
            get(handlers::customers::list).post(handlers::customers::create),
        )
        .route("/v1/customers/summary", get(handlers::customers::summary))
        .route(
            "/v1/customers/{id}",
            get(handlers::customers::get)
                .patch(handlers::customers::update)
                .delete(handlers::customers::delete),
        )
        .route(
            "/v1/invoices",
            get(handlers::invoices::list).post(handlers::invoices::create),
        )
        .route("/v1/invoices/summary", get(handlers::invoices::summary))
        .route(
            "/v1/invoices/{id}",
            get(handlers::invoices::get)
                .patch(handlers::invoices::update)
                .delete(handlers::invoices::delete),
        )
        .route(
            "/v1/notes",
            get(handlers::notes::list).post(handlers::notes::create),
        )
        .route("/v1/notes/summary", get(handlers::notes::summary))
        .route(
            "/v1/notes/{id}",
            get(handlers::notes::get)
                .patch(handlers::notes::update)
                .delete(handlers::notes::delete),
        )
        .route(
            "/v1/labels",
            get(handlers::labels::list).post(handlers::labels::create),
        )
        .route("/v1/labels/summary", get(handlers::labels::summary))
        .route(
            "/v1/labels/{id}",
            get(handlers::labels::get)
                .patch(handlers::labels::update)
                .delete(handlers::labels::delete),
        )
        .layer(
            ServiceBuilder::new()
                .layer(RequestBodyLimitLayer::new(64 * 1024))
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn probe(axum::extract::State(state): axum::extract::State<AppState>) -> String {
    let schema = serde_json::to_value(domain::ProbeSchema { enabled: true })
        .expect("probe schema is serializable");
    let derive_segment = schema
        .as_object()
        .expect("probe schema is an object")
        .keys()
        .next()
        .expect("probe schema contains one field");
    format!(
        "probe={}|S2:v{}|S3:v{}|S4:v{}|{}\nticks={}\npid={}\n",
        format!("{}{}", handlers::service_label(), handlers::refresh_async_label().await),
        handlers::priority_weight(0),
        domain::retention_days() - 30,
        std::mem::size_of::<domain::VersionStamp>() / std::mem::size_of::<u32>(),
        derive_segment,
        state.ticks.load(Ordering::Relaxed),
        std::process::id()
    )
}
