use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use taskcast_core::EngineError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Engine(#[from] EngineError),

    #[error("{0}")]
    BadRequest(String),

    #[error("Task not found")]
    NotFound(String),

    #[error("Forbidden")]
    Forbidden,

    #[error("Missing Bearer token")]
    MissingToken,

    #[error("Invalid or expired token")]
    InvalidToken,

    #[error("{0}")]
    NotImplemented(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Engine(e) => match e {
                EngineError::TaskNotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
                EngineError::TaskConflict(msg) => (StatusCode::CONFLICT, format!("Task already exists: {msg}")),
                EngineError::InvalidTransition { from, to } => (
                    StatusCode::CONFLICT,
                    format!("Invalid transition: {from:?} \u{2192} {to:?}"),
                ),
                EngineError::TaskTerminal(status) => (
                    StatusCode::CONFLICT,
                    format!("Cannot publish to task in terminal status: {status:?}"),
                ),
                EngineError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
                EngineError::Store(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            },
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden".to_string()),
            AppError::MissingToken => (StatusCode::UNAUTHORIZED, "Missing Bearer token".to_string()),
            AppError::InvalidToken => {
                (StatusCode::UNAUTHORIZED, "Invalid or expired token".to_string())
            }
            AppError::NotImplemented(msg) => {
                (StatusCode::NOT_IMPLEMENTED, msg.clone())
            }
        };

        (status, axum::Json(json!({ "error": message }))).into_response()
    }
}
