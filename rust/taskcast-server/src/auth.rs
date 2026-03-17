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
    pub jti: Option<String>,
    pub worker_id: Option<String>,
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
            jti: None,
            worker_id: None,
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
    #[serde(default)]
    jti: Option<String>,
    #[serde(default, rename = "workerId")]
    worker_id: Option<String>,
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
        match config.algorithm {
            Algorithm::ES256 | Algorithm::ES384 => {
                DecodingKey::from_ec_pem(public_key.as_bytes())?
            }
            _ => DecodingKey::from_rsa_pem(public_key.as_bytes())?,
        }
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
        jti: claims.jti,
        worker_id: claims.worker_id,
        task_ids,
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

    fn hs256_config() -> JwtConfig {
        JwtConfig {
            algorithm: Algorithm::HS256,
            secret: Some(TEST_SECRET.to_string()),
            public_key: None,
            issuer: None,
            audience: None,
        }
    }

    fn base_claims() -> JwtClaims {
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        JwtClaims {
            sub: Some("test-user".to_string()),
            task_ids: None,
            scope: Some(vec![PermissionScope::TaskCreate, PermissionScope::EventPublish]),
            iss: None,
            aud: None,
            exp: Some(exp),
            iat: None,
            jti: None,
            worker_id: None,
        }
    }

    fn make_token(claims: &JwtClaims) -> String {
        encode(
            &Header::default(),
            claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .expect("failed to encode JWT")
    }

    // ─── check_scope tests ──────────────────────────────────────────────────

    #[test]
    fn check_scope_all_grants_any_permission() {
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::All,
            scope: vec![PermissionScope::All],
            jti: None,
            worker_id: None,
        };
        assert!(check_scope(&auth, PermissionScope::TaskCreate, None));
        assert!(check_scope(&auth, PermissionScope::EventPublish, None));
        assert!(check_scope(&auth, PermissionScope::EventSubscribe, None));
    }

    #[test]
    fn check_scope_specific_scope_grants_matching() {
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::All,
            scope: vec![PermissionScope::TaskCreate],
            jti: None,
            worker_id: None,
        };
        assert!(check_scope(&auth, PermissionScope::TaskCreate, None));
    }

    #[test]
    fn check_scope_specific_scope_denies_non_matching() {
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::All,
            scope: vec![PermissionScope::TaskCreate],
            jti: None,
            worker_id: None,
        };
        assert!(!check_scope(&auth, PermissionScope::EventPublish, None));
    }

    #[test]
    fn check_scope_empty_scope_denies_all() {
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::All,
            scope: vec![],
            jti: None,
            worker_id: None,
        };
        assert!(!check_scope(&auth, PermissionScope::TaskCreate, None));
        assert!(!check_scope(&auth, PermissionScope::EventPublish, None));
        assert!(!check_scope(&auth, PermissionScope::All, None));
    }

    #[test]
    fn check_scope_task_id_access_all_allows_any_task() {
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::All,
            scope: vec![PermissionScope::TaskCreate],
            jti: None,
            worker_id: None,
        };
        assert!(check_scope(&auth, PermissionScope::TaskCreate, Some("any-task-id")));
        assert!(check_scope(&auth, PermissionScope::TaskCreate, Some("another-task")));
    }

    #[test]
    fn check_scope_task_id_list_allows_matching_task() {
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::List(vec!["task-1".to_string(), "task-2".to_string()]),
            scope: vec![PermissionScope::TaskCreate],
            jti: None,
            worker_id: None,
        };
        assert!(check_scope(&auth, PermissionScope::TaskCreate, Some("task-1")));
        assert!(check_scope(&auth, PermissionScope::TaskCreate, Some("task-2")));
    }

    #[test]
    fn check_scope_task_id_list_denies_non_matching_task() {
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::List(vec!["task-1".to_string()]),
            scope: vec![PermissionScope::TaskCreate],
            jti: None,
            worker_id: None,
        };
        assert!(!check_scope(&auth, PermissionScope::TaskCreate, Some("task-999")));
    }

    #[test]
    fn check_scope_both_scope_and_task_id_must_match() {
        // Right scope, wrong task_id
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::List(vec!["task-1".to_string()]),
            scope: vec![PermissionScope::TaskCreate],
            jti: None,
            worker_id: None,
        };
        assert!(!check_scope(&auth, PermissionScope::TaskCreate, Some("task-999")));

        // Right task_id, wrong scope
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::List(vec!["task-1".to_string()]),
            scope: vec![PermissionScope::EventPublish],
            jti: None,
            worker_id: None,
        };
        assert!(!check_scope(&auth, PermissionScope::TaskCreate, Some("task-1")));

        // Both match
        let auth = AuthContext {
            sub: None,
            task_ids: TaskIdAccess::List(vec!["task-1".to_string()]),
            scope: vec![PermissionScope::TaskCreate],
            jti: None,
            worker_id: None,
        };
        assert!(check_scope(&auth, PermissionScope::TaskCreate, Some("task-1")));
    }

    // ─── decode_jwt tests ───────────────────────────────────────────────────

    #[test]
    fn decode_jwt_valid_token_extracts_sub() {
        let claims = base_claims();
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).expect("should decode");
        assert_eq!(ctx.sub, Some("test-user".to_string()));
    }

    #[test]
    fn decode_jwt_valid_token_extracts_scope() {
        let claims = base_claims();
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).expect("should decode");
        assert_eq!(ctx.scope.len(), 2);
        assert!(ctx.scope.contains(&PermissionScope::TaskCreate));
        assert!(ctx.scope.contains(&PermissionScope::EventPublish));
    }

    #[test]
    fn decode_jwt_no_scope_defaults_to_empty() {
        let mut claims = base_claims();
        claims.scope = None;
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).expect("should decode");
        assert!(ctx.scope.is_empty());
    }

    #[test]
    fn decode_jwt_wildcard_task_ids_maps_to_all() {
        let mut claims = base_claims();
        claims.task_ids = Some(TaskIdsClaim::Wildcard("*".to_string()));
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).expect("should decode");
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
    }

    #[test]
    fn decode_jwt_non_star_wildcard_maps_to_all() {
        let mut claims = base_claims();
        claims.task_ids = Some(TaskIdsClaim::Wildcard("anything".to_string()));
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).expect("should decode");
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
    }

    #[test]
    fn decode_jwt_list_task_ids_preserved() {
        let mut claims = base_claims();
        claims.task_ids = Some(TaskIdsClaim::List(vec!["t1".to_string(), "t2".to_string()]));
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).expect("should decode");
        match ctx.task_ids {
            TaskIdAccess::List(ids) => {
                assert_eq!(ids, vec!["t1".to_string(), "t2".to_string()]);
            }
            _ => panic!("expected TaskIdAccess::List"),
        }
    }

    #[test]
    fn decode_jwt_no_task_ids_defaults_to_all() {
        let mut claims = base_claims();
        claims.task_ids = None;
        let token = make_token(&claims);
        let ctx = decode_jwt(&token, &hs256_config()).expect("should decode");
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
    }

    #[test]
    fn decode_jwt_expired_token_fails() {
        let mut claims = base_claims();
        claims.exp = Some(1000);
        let token = make_token(&claims);
        let result = decode_jwt(&token, &hs256_config());
        assert!(result.is_err());
    }

    #[test]
    fn decode_jwt_wrong_secret_fails() {
        let claims = base_claims();
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.secret = Some("wrong-secret-key-that-does-not-match-the-original".to_string());
        let result = decode_jwt(&token, &config);
        assert!(result.is_err());
    }

    #[test]
    fn decode_jwt_no_key_configured_fails() {
        let claims = base_claims();
        let token = make_token(&claims);
        let config = JwtConfig {
            algorithm: Algorithm::HS256,
            secret: None,
            public_key: None,
            issuer: None,
            audience: None,
        };
        let result = decode_jwt(&token, &config);
        assert!(result.is_err());
    }

    #[test]
    fn decode_jwt_issuer_validation_accepts_matching() {
        let mut claims = base_claims();
        claims.iss = Some("my-issuer".to_string());
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.issuer = Some("my-issuer".to_string());
        let result = decode_jwt(&token, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn decode_jwt_issuer_validation_rejects_wrong() {
        let mut claims = base_claims();
        claims.iss = Some("wrong-issuer".to_string());
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.issuer = Some("expected-issuer".to_string());
        let result = decode_jwt(&token, &config);
        assert!(result.is_err());
    }

    #[test]
    fn decode_jwt_audience_validation_accepts_matching() {
        let mut claims = base_claims();
        claims.aud = Some(serde_json::json!("my-audience"));
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.audience = Some("my-audience".to_string());
        let result = decode_jwt(&token, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn decode_jwt_audience_validation_rejects_wrong() {
        let mut claims = base_claims();
        claims.aud = Some(serde_json::json!("wrong-audience"));
        let token = make_token(&claims);
        let mut config = hs256_config();
        config.audience = Some("expected-audience".to_string());
        let result = decode_jwt(&token, &config);
        assert!(result.is_err());
    }

    #[test]
    fn decode_jwt_garbage_token_fails() {
        let result = decode_jwt("not-a-real-token", &hs256_config());
        assert!(result.is_err());
    }

    #[test]
    fn decode_jwt_empty_token_fails() {
        let result = decode_jwt("", &hs256_config());
        assert!(result.is_err());
    }

    #[test]
    fn auth_context_open_has_all_access() {
        let ctx = AuthContext::open();
        assert!(ctx.sub.is_none());
        assert!(matches!(ctx.task_ids, TaskIdAccess::All));
        assert_eq!(ctx.scope, vec![PermissionScope::All]);
    }

    // ─── EC (ES256) key tests ────────────────────────────────────────────────

    const EC_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg/6Svroa6zf7uKd0i
zrZ8KPrakdGXm6F/lpULNSuRYWqhRANCAAQ6TbQS2VOf2iV/EQInVdbvWJEOn/KX
EEyPwMDxwmDKHaZg6OU8BGyN6UdEFRgEq4CITvgsfDfOtGW2BazLuzVX
-----END PRIVATE KEY-----";

    const EC_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEOk20EtlTn9olfxECJ1XW71iRDp/y
lxBMj8DA8cJgyh2mYOjlPARsjelHRBUYBKuAiE74LHw3zrRltgWsy7s1Vw==
-----END PUBLIC KEY-----";

    fn es256_config() -> JwtConfig {
        JwtConfig {
            algorithm: Algorithm::ES256,
            secret: None,
            public_key: Some(EC_PUBLIC_KEY.to_string()),
            issuer: None,
            audience: None,
        }
    }

    fn make_es256_token(claims: &JwtClaims) -> String {
        encode(
            &Header::new(Algorithm::ES256),
            claims,
            &EncodingKey::from_ec_pem(EC_PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .expect("failed to encode ES256 JWT")
    }

    #[test]
    fn decode_jwt_es256_with_ec_public_key() {
        let claims = base_claims();
        let token = make_es256_token(&claims);
        let ctx = decode_jwt(&token, &es256_config()).expect("should decode ES256 JWT");
        assert_eq!(ctx.sub, Some("test-user".to_string()));
    }

    #[test]
    fn decode_jwt_es256_wrong_key_fails() {
        let claims = base_claims();
        let token = make_es256_token(&claims);
        let mut config = es256_config();
        // Use a different (invalid) public key
        config.public_key = Some("-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEAAAAAAAAAAAAAAAAAAAAAAAAAAAA\nAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n-----END PUBLIC KEY-----".to_string());
        let result = decode_jwt(&token, &config);
        assert!(result.is_err());
    }

    // ─── RSA (RS256) JWT tests ──────────────────────────────────────────────

    const RSA_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDac9Dkk2KIhPUD
PIQdAccvhEMLQcOfS1k93nSqpqO4RRATlUcFJ7OkTr+Lu0ZLwlzaN9zj5IR0MEF8
HxIUZ06+CfyGMYQ3RgwvwIV+0Es4EHTVbwDVQSxjhCXr7i0VvwVvIczzpMJQs+Ar
SR0kJVntHgeDrWI8WP8pfL0M7TgmS+ft3ovH4KwA3c2jcHsLjb5KOYGMN4PpEKsh
vIFvaA9eKeBEcHTudhUH/qoJ+69eZgYhksIHP1Ar1c9ZDezzxq7blWFfIZ2pUXjP
bPZIBye8dDJOEPvnSQdAIMfHY48KtXMYzBlihPTem3Olvhh0y72bv+TzS5eJj7Wj
AFFBUplJAgMBAAECggEACWwvHvo306ap1i1mj2++XTopwChih+edSPy5mzYb4BIO
ivcIkgbaifmhlnqJ94ZB22HHI9yn//D9lMfW+k1THqxZ7u8015jXDro8Uod+hG5l
SQ2ELi6k+609YNYBritH+zlXLffghSWfw441n75YCNEfnC+UNKvWub3j06ppyRuu
8ckAJCkGVSLQOD4Was/C/oZh1LBocwFzVRYwqSvfa3frzsjs6lpcfKYBiy0tT0GA
Gp0gV/Drm4e244uc06O3wf0DV9s66TzAPWiXdY1iXIx2SeP7olJiRVDShMIuCWXw
xFf9i1u/Wd9mygT3j3pfS+XbdpkNUQ+xWzDfSbhQFwKBgQD+9DTSFQQ5zIpdx62u
EAyZd9zefkOy5Pz5ww3JFuf5m/lqNPP9iak++KdaXE4uLEDxNx/YpL3GaJYqZbDb
54OqAtYKI/BAnqENTsIOZHCUcThOubDb2atdf4SM/d/LLqm90ws7GR9pl44kSX3m
XWqu2ZYwTjUhxG2+slSSAEZAowKBgQDbWUUWw/JsPjbJnTdOw9qTNFB/8NDFpK4o
pmeP8CBHUyGJ9xbS9KF422PaAip+qpAAVUZIwRKghzFbsZYOUsY8T7/1sa9TAe9z
xkN/WpWC9CkkfqlfGeKd3vCOgNJgDfoYG9xg8wVGLxdFqZTc2TceR725fnJ2XQ/+
64S3o35hIwKBgCX88yXeVcva6z7/8qP95jKzxmgsfh/YuMzqQFKXJCdwEyC9FSHQ
8jPczurK2CdPFroaUZdxLIm0qbNVhZ2uiFK7q3LFWhX2zN5zvrxwQMoNeKtaRbzs
WPlMd86eQUs//7C2EnVSz/hmyZokjSrW/n9hElzOOmJ2pzeD0NAUVGNrAoGBALCa
2XOQ5HieRsJhYIx8SFnI/x9jptPa2ZZFFJDhhzumBRTt6mSDLhb+nRCEOe6uKS77
eZFOUk8JSoZHH0f9ATrCAw4wt7afkkmm0eWR61r6dr25dWjqkHkHbKtvsPJBjqEm
nXM5d3+gnp4qAX8nLKemn2zgEAyYtS5MNhkc+739AoGBAMjEFWQWfHS0wtIJ6Dzv
aASJc3I+k0/+PPbCD29ES6OYnGcddzCeLpVCYiKYPaOczAsO/umEA56zVy0/i7ad
P4urk6wm7nh9WEjHUdgKNxWxPPb5WgenJY5gveXxjvGmxJL2sifUsYpheNxCUv9o
wFtaoSVhXzRhhDZtU+6uhp+A
-----END PRIVATE KEY-----";

    const RSA_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA2nPQ5JNiiIT1AzyEHQHH
L4RDC0HDn0tZPd50qqajuEUQE5VHBSezpE6/i7tGS8Jc2jfc4+SEdDBBfB8SFGdO
vgn8hjGEN0YML8CFftBLOBB01W8A1UEsY4Ql6+4tFb8FbyHM86TCULPgK0kdJCVZ
7R4Hg61iPFj/KXy9DO04Jkvn7d6Lx+CsAN3No3B7C42+SjmBjDeD6RCrIbyBb2gP
XingRHB07nYVB/6qCfuvXmYGIZLCBz9QK9XPWQ3s88au25VhXyGdqVF4z2z2SAcn
vHQyThD750kHQCDHx2OPCrVzGMwZYoT03ptzpb4YdMu9m7/k80uXiY+1owBRQVKZ
SQIDAQAB
-----END PUBLIC KEY-----";

    fn rs256_config() -> JwtConfig {
        JwtConfig {
            algorithm: Algorithm::RS256,
            secret: None,
            public_key: Some(RSA_PUBLIC_KEY.to_string()),
            issuer: None,
            audience: None,
        }
    }

    fn make_rs256_token(claims: &JwtClaims) -> String {
        encode(
            &Header::new(Algorithm::RS256),
            claims,
            &EncodingKey::from_rsa_pem(RSA_PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .expect("failed to encode RS256 JWT")
    }

    #[test]
    fn decode_jwt_rs256_with_rsa_public_key() {
        let claims = base_claims();
        let token = make_rs256_token(&claims);
        let ctx = decode_jwt(&token, &rs256_config()).expect("should decode RS256 JWT");
        assert_eq!(ctx.sub, Some("test-user".to_string()));
    }

    #[test]
    fn decode_jwt_rs256_wrong_key_fails() {
        let claims = base_claims();
        let token = make_rs256_token(&claims);
        let mut config = rs256_config();
        config.public_key = Some(EC_PUBLIC_KEY.to_string()); // Wrong key type
        let result = decode_jwt(&token, &config);
        assert!(result.is_err());
    }
}
