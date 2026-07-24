use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use taskcast_core::EngineError;

use crate::http_failure::{HttpFailureDetail, HttpFailureKind};

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
        let (status, message, detail) = match &self {
            AppError::Engine(e) => match e {
                EngineError::TaskNotFound(msg) => (StatusCode::NOT_FOUND, msg.clone(), None),
                EngineError::TaskConflict(msg) => (
                    StatusCode::CONFLICT,
                    format!("Task already exists: {msg}"),
                    None,
                ),
                EngineError::InvalidTransition { from, to } => (
                    StatusCode::CONFLICT,
                    format!("Invalid transition: {from:?} \u{2192} {to:?}"),
                    None,
                ),
                EngineError::TaskTerminal(status) => (
                    StatusCode::CONFLICT,
                    format!("Cannot publish to task in terminal status: {status:?}"),
                    None,
                ),
                EngineError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone(), None),
                EngineError::Archive(error) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                    Some(HttpFailureDetail::new(
                        HttpFailureKind::Archive,
                        error.to_string(),
                    )),
                ),
                EngineError::Store(error) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                    Some(HttpFailureDetail::new(
                        HttpFailureKind::Store,
                        error.to_string(),
                    )),
                ),
            },
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone(), None),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone(), None),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden".to_string(), None),
            AppError::MissingToken => (
                StatusCode::UNAUTHORIZED,
                "Missing Bearer token".to_string(),
                None,
            ),
            AppError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "Invalid or expired token".to_string(),
                None,
            ),
            AppError::NotImplemented(msg) => (
                StatusCode::NOT_IMPLEMENTED,
                msg.clone(),
                Some(HttpFailureDetail::new(
                    HttpFailureKind::Internal,
                    msg.clone(),
                )),
            ),
        };

        let mut response = (status, axum::Json(json!({ "error": message }))).into_response();
        if let Some(detail) = detail {
            response.extensions_mut().insert(detail);
        }
        response
    }
}
