use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;
use taskcast_core::PermissionScope;

// ─── AuthMode ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AuthMode {
    None,
    Jwt(JwtConfig),
}

#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub algorithm: Algorithm,
    pub secret: Option<String>,
    pub public_key: Option<String>,
    pub issuer: Option<String>,
    pub audience: Option<String>,
}

// ─── AuthContext ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub sub: Option<String>,
    pub task_ids: TaskIdAccess,
    pub scope: Vec<PermissionScope>,
}

#[derive(Debug, Clone)]
pub enum TaskIdAccess {
    All,
    List(Vec<String>),
}

impl AuthContext {
    pub fn open() -> Self {
        Self {
            sub: None,
            task_ids: TaskIdAccess::All,
            scope: vec![PermissionScope::All],
        }
    }
}

// ─── JWT Claims ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct JwtClaims {
    #[serde(default)]
    sub: Option<String>,
    #[serde(default, rename = "taskIds")]
    task_ids: Option<TaskIdsClaim>,
    #[serde(default)]
    scope: Option<Vec<PermissionScope>>,
    // Standard claims
    #[serde(default)]
    iss: Option<String>,
    #[serde(default)]
    aud: Option<serde_json::Value>,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(default)]
    iat: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum TaskIdsClaim {
    Wildcard(String),
    List(Vec<String>),
}

// ─── Scope checking ─────────────────────────────────────────────────────────

pub fn check_scope(auth: &AuthContext, required: PermissionScope, task_id: Option<&str>) -> bool {
    if let Some(task_id) = task_id {
        if let TaskIdAccess::List(ref ids) = auth.task_ids {
            if !ids.iter().any(|id| id == task_id) {
                return false;
            }
        }
    }
    auth.scope.contains(&PermissionScope::All) || auth.scope.contains(&required)
}

// ─── Auth Middleware ─────────────────────────────────────────────────────────

pub async fn auth_middleware(
    State(auth_mode): State<Arc<AuthMode>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    match auth_mode.as_ref() {
        AuthMode::None => {
            req.extensions_mut().insert(AuthContext::open());
            next.run(req).await
        }
        AuthMode::Jwt(config) => {
            let auth_header = req
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok());

            let token = match auth_header {
                Some(header) if header.starts_with("Bearer ") => &header[7..],
                _ => {
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        axum::Json(json!({ "error": "Missing Bearer token" })),
                    )
                        .into_response();
                }
            };

            match decode_jwt(token, config) {
                Ok(ctx) => {
                    req.extensions_mut().insert(ctx);
                    next.run(req).await
                }
                Err(_) => (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(json!({ "error": "Invalid or expired token" })),
                )
                    .into_response(),
            }
        }
    }
}

fn decode_jwt(token: &str, config: &JwtConfig) -> Result<AuthContext, jsonwebtoken::errors::Error> {
    let mut validation = Validation::new(config.algorithm);

    if let Some(ref issuer) = config.issuer {
        validation.set_issuer(&[issuer]);
    }

    if let Some(ref audience) = config.audience {
        validation.set_audience(&[audience]);
    } else {
        validation.validate_aud = false;
    }

    let key = if let Some(ref secret) = config.secret {
        DecodingKey::from_secret(secret.as_bytes())
    } else if let Some(ref public_key) = config.public_key {
        DecodingKey::from_rsa_pem(public_key.as_bytes())?
    } else {
        return Err(jsonwebtoken::errors::Error::from(
            jsonwebtoken::errors::ErrorKind::InvalidKeyFormat,
        ));
    };

    let token_data = decode::<JwtClaims>(token, &key, &validation)?;
    let claims = token_data.claims;

    let task_ids = match claims.task_ids {
        Some(TaskIdsClaim::Wildcard(ref s)) if s == "*" => TaskIdAccess::All,
        Some(TaskIdsClaim::List(ids)) => TaskIdAccess::List(ids),
        Some(TaskIdsClaim::Wildcard(_)) => TaskIdAccess::All,
        None => TaskIdAccess::All,
    };

    let scope = claims.scope.unwrap_or_default();

    Ok(AuthContext {
        sub: claims.sub,
        task_ids,
        scope,
    })
}
