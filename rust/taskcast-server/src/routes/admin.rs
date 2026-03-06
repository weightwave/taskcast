use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::json;
use taskcast_core::config::TaskcastConfig;

use crate::auth::AuthMode;

// ─── Admin State ────────────────────────────────────────────────────────────

/// State shared with the admin route handler.
#[derive(Clone)]
pub struct AdminState {
    pub config: Arc<TaskcastConfig>,
    pub auth_mode: Arc<AuthMode>,
}

// ─── Request/Response Types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminTokenRequest {
    admin_token: Option<String>,
    scopes: Option<Vec<String>>,
    expires_in: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminTokenResponse {
    token: String,
    expires_at: u64,
}

// ─── JWT Claims ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminJwtClaims {
    sub: String,
    scope: Vec<String>,
    task_ids: String,
    exp: u64,
    iat: u64,
}

// ─── Handler ────────────────────────────────────────────────────────────────

/// POST /admin/token — exchange admin token for a JWT.
///
/// This handler is intentionally NOT behind the normal auth middleware.
/// It authenticates via admin token and issues JWTs (when auth mode is jwt).
pub async fn admin_token(
    State(state): State<Arc<AdminState>>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let config = &state.config;

    // 1. If adminApi is not enabled, this endpoint does not exist
    if config.admin_api != Some(true) {
        return (StatusCode::NOT_FOUND, axum::Json(json!({ "error": "Not found" }))).into_response();
    }

    // 2. Parse and validate request body
    let req: AdminTokenRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "Invalid request body" })),
            )
                .into_response();
        }
    };

    let admin_token = match &req.admin_token {
        Some(t) if !t.is_empty() => t.as_str(),
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({ "error": "Invalid admin token" })),
            )
                .into_response();
        }
    };

    // 3. Validate admin token — ALWAYS, regardless of server auth mode
    let expected_token = match &config.admin_token {
        Some(t) => t.as_str(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({ "error": "Invalid admin token" })),
            )
                .into_response();
        }
    };

    if admin_token != expected_token {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "error": "Invalid admin token" })),
        )
            .into_response();
    }

    // 4. Parse optional fields
    let scopes: Vec<String> = req.scopes.unwrap_or_else(|| vec!["*".to_string()]);
    let expires_in: u64 = match req.expires_in {
        Some(e) if e > 0 => e as u64,
        _ => 86400, // default: 24h
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expires_at = now + expires_in;

    // 5. If auth mode is JWT, sign a real token
    if let AuthMode::Jwt(jwt_config) = state.auth_mode.as_ref() {
        if let Some(ref secret) = jwt_config.secret {
            let claims = AdminJwtClaims {
                sub: "admin".to_string(),
                scope: scopes,
                task_ids: "*".to_string(),
                exp: expires_at,
                iat: now,
            };

            let header = Header::new(jwt_config.algorithm);
            let key = EncodingKey::from_secret(secret.as_bytes());

            match encode(&header, &claims, &key) {
                Ok(token) => {
                    return axum::Json(AdminTokenResponse {
                        token,
                        expires_at,
                    })
                    .into_response();
                }
                Err(_) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        axum::Json(json!({ "error": "Failed to sign JWT" })),
                    )
                        .into_response();
                }
            }
        }
    }

    // 6. Non-JWT mode: return placeholder token
    axum::Json(AdminTokenResponse {
        token: String::new(),
        expires_at,
    })
    .into_response()
}
