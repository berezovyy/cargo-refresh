use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Validation(#[from] domain::ValidationError),
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error("{0}")]
    BadRequest(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match &self {
            Self::Validation(_) | Self::BadRequest(_) => {
                (StatusCode::BAD_REQUEST, "validation_failed")
            }
            Self::Store(store::StoreError::NotFound(_)) => (StatusCode::NOT_FOUND, "not_found"),
            Self::Store(store::StoreError::Duplicate) => (StatusCode::CONFLICT, "already_exists"),
            Self::Store(store::StoreError::Conflict { .. }) => {
                (StatusCode::CONFLICT, "revision_conflict")
            }
        };
        (
            status,
            Json(ErrorBody {
                error,
                message: self.to_string(),
            }),
        )
            .into_response()
    }
}
