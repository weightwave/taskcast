use std::sync::Arc;

use axum_test::TestServer;
use std::future::Future;
use std::pin::Pin;
use taskcast_core::{
    DependencyName, DependencyUnavailableError, LongTermStore, MemoryBroadcastProvider,
    MemoryLongTermStore, MemoryShortTermStore, ShortTermStore, StorageWriterRegistration,
    TaskEngine, TaskEngineOptions,
};
use taskcast_server::{
    create_app, create_app_with_runtime_health_and_routes, AuthMode, CorsConfig, DependencyCheck,
    DependencyHealthRegistry, JwtConfig, RuntimeAdapterDescriptors, RuntimeAppOptions,
    RuntimeHealth, StderrHttpFailureLogger,
};

fn make_server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    TestServer::new(app)
}

#[tokio::test]
async fn runtime_app_options_default_preserves_default_health_detail() {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app_with_runtime_health_and_routes(
        engine,
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
        Arc::new(StderrHttpFailureLogger::new(
            taskcast_server::LogLevel::Info,
        )),
        RuntimeAppOptions::default(),
    );

    let body: serde_json::Value = TestServer::new(app).get("/health/detail").await.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert!(body["adapters"]["longTermStore"].is_null());
}

#[tokio::test]
async fn root_returns_server_info_and_links() {
    let server = make_server();
    let res = server.get("/").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["name"], "taskcast");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["apiVersion"], "v1");
    assert_eq!(body["links"]["health"], "/health");
    assert_eq!(body["links"]["healthReady"], "/health/ready");
    assert_eq!(body["links"]["healthDetail"], "/health/detail");
    assert_eq!(body["links"]["openapi"], "/openapi.json");
    assert_eq!(body["links"]["docs"], "/docs");
}

#[tokio::test]
async fn health_returns_version_handshake_fields() {
    let server = make_server();
    let res = server.get("/health").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["name"], "taskcast");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["apiVersion"], "v1");
}

#[tokio::test]
async fn health_detail_returns_ok_and_uptime() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["name"], "taskcast");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["apiVersion"], "v1");
    assert!(body["uptime"].is_number());
    // Uptime should parse as a valid u64 (non-negative by type)
    let _uptime = body["uptime"]
        .as_u64()
        .expect("uptime should be a valid u64");
}

#[tokio::test]
async fn health_detail_reports_auth_mode() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["auth"]["mode"], "none");
}

#[tokio::test]
async fn health_detail_reports_memory_adapters_by_default() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["broadcast"]["status"], "ok");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["status"], "ok");
}

#[tokio::test]
async fn health_detail_reports_writer_protocol_readiness() {
    let hot = Arc::new(MemoryShortTermStore::new());
    hot.register_storage_writer(
        StorageWriterRegistration {
            instance_id: "legacy-writer".to_string(),
            storage_protocol_version: 1,
            build: "old".to_string(),
            expires_at: 0.0,
        },
        30_000,
    )
    .await
    .unwrap();
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: Some(durable as Arc<dyn LongTermStore>),
        hooks: None,
    }));
    let server = make_server_from_engine(engine);
    let body = server
        .get("/health/detail")
        .await
        .json::<serde_json::Value>();
    assert_eq!(
        body["storage"],
        serde_json::json!({
            "releaseReady": false,
            "requiredStorageProtocolVersion": 2,
            "activeWriterCount": 2,
            "incompatibleWriterIds": ["legacy-writer"]
        })
    );
}

fn make_server_from_engine(engine: Arc<TaskEngine>) -> TestServer {
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    TestServer::new(app)
}

fn make_jwt_server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let auth = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some("test-secret-key-for-jwt-signing".to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let (app, _) = create_app(engine, auth, None, None, CorsConfig::default());
    TestServer::new(app)
}

#[tokio::test]
async fn health_bypasses_jwt_auth() {
    let server = make_jwt_server();
    // No Bearer token — should still return 200
    let res = server.get("/health").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn health_detail_bypasses_jwt_auth() {
    let server = make_jwt_server();
    // No Bearer token — should still return 200
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert!(body["uptime"].is_number());
    assert_eq!(body["auth"]["mode"], "jwt");
}

#[tokio::test]
async fn health_ready_bypasses_jwt_auth() {
    let server = make_jwt_server();
    let res = server.get("/health/ready").await;
    res.assert_status_ok();
    assert_eq!(res.json::<serde_json::Value>()["ok"], true);
}

#[tokio::test]
async fn authenticated_routes_still_require_jwt() {
    let server = make_jwt_server();
    // No Bearer token — should return 401 for task routes
    let res = server.get("/tasks").await;
    res.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn openapi_and_docs_bypass_jwt_auth() {
    let server = make_jwt_server();

    let openapi = server.get("/openapi.json").await;
    openapi.assert_status_ok();

    let docs = server.get("/docs").await;
    docs.assert_status_ok();

    let tasks = server.get("/tasks").await;
    tasks.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

// ─── health_detail with config adapter overrides ────────────────────────────

use taskcast_core::config::{AdapterEntry, AdaptersConfig, TaskcastConfig};

fn make_server_with_config(config: TaskcastConfig) -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(
        engine,
        AuthMode::None,
        None,
        Some(config),
        CorsConfig::default(),
    );
    TestServer::new(app)
}

#[tokio::test]
async fn health_detail_with_config_broadcast_override() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: Some("redis://localhost:6379".to_string()),
            }),
            short_term_store: None,
            long_term_store: None,
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "redis");
    // shortTermStore should still default to "memory"
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    // No longTermStore configured
    assert!(body["adapters"]["longTermStore"].is_null());
}

#[tokio::test]
async fn health_detail_with_config_short_term_store_override() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: None,
            short_term_store: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: None,
            }),
            long_term_store: None,
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "redis");
    // broadcast should still default to "memory"
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
}

#[tokio::test]
async fn health_detail_with_config_long_term_store() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: None,
            short_term_store: None,
            long_term_store: Some(AdapterEntry {
                provider: "postgres".to_string(),
                url: Some("postgresql://localhost/taskcast".to_string()),
            }),
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["longTermStore"]["provider"], "postgres");
    assert_eq!(body["adapters"]["longTermStore"]["status"], "ok");
}

#[tokio::test]
async fn health_detail_with_all_adapters_configured() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: Some("redis://localhost:6379".to_string()),
            }),
            short_term_store: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: Some("redis://localhost:6379".to_string()),
            }),
            long_term_store: Some(AdapterEntry {
                provider: "postgres".to_string(),
                url: Some("postgresql://localhost/taskcast".to_string()),
            }),
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "redis");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "redis");
    assert_eq!(body["adapters"]["longTermStore"]["provider"], "postgres");
    assert_eq!(body["adapters"]["longTermStore"]["status"], "ok");
}

#[tokio::test]
async fn health_detail_uses_effective_runtime_adapters_over_file_config() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: None,
            }),
            short_term_store: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: None,
            }),
            long_term_store: Some(AdapterEntry {
                provider: "postgres".to_string(),
                url: None,
            }),
        }),
        ..Default::default()
    };
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let runtime_health = RuntimeHealth {
        effective_adapters: Some(RuntimeAdapterDescriptors {
            broadcast: "memory".to_string(),
            short_term_store: "memory".to_string(),
            long_term_store: None,
        }),
        ..Default::default()
    };
    let (app, _) = create_app_with_runtime_health_and_routes(
        engine,
        AuthMode::None,
        None,
        Some(config),
        CorsConfig::default(),
        Arc::new(StderrHttpFailureLogger::new(
            taskcast_server::LogLevel::Info,
        )),
        RuntimeAppOptions {
            runtime_health,
            additional_routes: axum::Router::new(),
        },
    );
    let body: serde_json::Value = TestServer::new(app).get("/health/detail").await.json();

    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert!(body["adapters"]["longTermStore"].is_null());
}

#[tokio::test]
async fn health_detail_with_config_but_no_adapters_section() {
    // Config exists but adapters is None -- defaults should be used
    let config = TaskcastConfig {
        adapters: None,
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
}

#[tokio::test]
async fn health_detail_with_config_empty_adapters() {
    // Config has adapters section but all fields are None
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: None,
            short_term_store: None,
            long_term_store: None,
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    // All defaults
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert!(body["adapters"]["longTermStore"].is_null());
}

fn runtime_check<F, Fut>(function: F) -> DependencyCheck
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), DependencyUnavailableError>> + Send + 'static,
{
    Arc::new(move || {
        let future: Pin<Box<dyn Future<Output = Result<(), DependencyUnavailableError>> + Send>> =
            Box::pin(function());
        future
    })
}

#[tokio::test]
async fn liveness_stays_up_while_readiness_and_detail_track_recovery() {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let pubsub_healthy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let registry = Arc::new(DependencyHealthRegistry::new());
    registry
        .register(
            DependencyName::RedisCommand,
            runtime_check(|| async { Ok(()) }),
        )
        .unwrap();
    registry
        .register(
            DependencyName::RedisPubSub,
            runtime_check({
                let healthy = pubsub_healthy.clone();
                move || {
                    let healthy = healthy.clone();
                    async move {
                        if healthy.load(std::sync::atomic::Ordering::SeqCst) {
                            Ok(())
                        } else {
                            Err(DependencyUnavailableError::new(
                                DependencyName::RedisPubSub,
                                taskcast_core::DependencyErrorKind::ConnectionClosed,
                                std::io::Error::other("private Redis endpoint"),
                            ))
                        }
                    }
                }
            }),
        )
        .unwrap();
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: None,
            }),
            short_term_store: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: None,
            }),
            long_term_store: None,
        }),
        ..Default::default()
    };
    let auth = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some("test-secret-key-for-jwt-signing".to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let runtime_health = RuntimeHealth {
        registry: Some(registry),
        ..Default::default()
    };
    let (app, _) = create_app_with_runtime_health_and_routes(
        engine,
        auth,
        None,
        Some(config),
        CorsConfig::default(),
        Arc::new(StderrHttpFailureLogger::new(
            taskcast_server::LogLevel::Info,
        )),
        RuntimeAppOptions {
            runtime_health,
            additional_routes: axum::Router::new(),
        },
    );
    let server = TestServer::new(app);

    server.get("/health").await.assert_status_ok();
    let unavailable = server.get("/health/ready").await;
    unavailable.assert_status(axum_test::http::StatusCode::SERVICE_UNAVAILABLE);
    let unavailable_json: serde_json::Value = unavailable.json();
    assert_eq!(unavailable_json["ok"], false);
    assert_eq!(
        unavailable_json["dependencies"]["redisPubSub"]["errorKind"],
        "connection_closed"
    );

    let detail = server.get("/health/detail").await;
    detail.assert_status_ok();
    let detail_json: serde_json::Value = detail.json();
    assert_eq!(detail_json["ok"], false);
    assert_eq!(detail_json["adapters"]["broadcast"]["status"], "error");
    assert_eq!(detail_json["adapters"]["shortTermStore"]["status"], "ok");
    assert_eq!(
        detail_json["dependencies"]["redisPubSub"]["lastErrorKind"],
        "connection_closed"
    );

    pubsub_healthy.store(true, std::sync::atomic::Ordering::SeqCst);
    let recovered = server.get("/health/ready").await;
    recovered.assert_status_ok();
    assert_eq!(recovered.json::<serde_json::Value>()["ok"], true);
    assert_eq!(
        server
            .get("/health/detail")
            .await
            .json::<serde_json::Value>()["adapters"]["broadcast"]["status"],
        "ok"
    );
}

#[tokio::test]
async fn default_detail_omits_dependencies() {
    let server = make_server();
    let body: serde_json::Value = server.get("/health/detail").await.json();
    assert!(body["dependencies"].is_null());
}
