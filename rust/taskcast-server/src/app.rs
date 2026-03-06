use std::sync::Arc;

use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use axum::Router;
use taskcast_core::worker_manager::WorkerManager;
use taskcast_core::TaskEngine;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use crate::auth::{auth_middleware, AuthMode};
use crate::openapi::ApiDoc;
use crate::routes::{sse, tasks};

/// Shared application state available to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<TaskEngine>,
    pub auth_mode: Arc<AuthMode>,
}

/// Create the Axum router with all taskcast routes mounted.
pub fn create_app(
    engine: Arc<TaskEngine>,
    auth_mode: AuthMode,
    worker_manager: Option<Arc<WorkerManager>>,
) -> Router {
    let auth_mode = Arc::new(auth_mode);

    let task_routes = Router::new()
        .route("/", post(tasks::create_task))
        .route("/{task_id}", get(tasks::get_task))
        .route("/{task_id}/status", patch(tasks::transition_task))
        .route(
            "/{task_id}/events",
            post(tasks::publish_events).get(sse::sse_events),
        )
        .route("/{task_id}/events/history", get(tasks::get_event_history))
        .with_state(Arc::clone(&engine));

    let mut app = Router::new()
        .route("/health", get(health))
        .nest("/tasks", task_routes);

    // Conditionally mount worker routes if a WorkerManager is provided
    if let Some(manager) = worker_manager {
        let worker_routes = crate::routes::workers::workers_router()
            .with_state(Arc::clone(&manager));

        // Mount WS route at top level for /workers/ws
        app = app
            .route(
                "/workers/ws",
                get(crate::routes::worker_ws::ws_handler).with_state(Arc::clone(&manager)),
            )
            .nest("/workers", worker_routes);
    }

    // OpenAPI spec and Scalar UI
    let openapi_spec = ApiDoc::openapi();
    app = app
        .route(
            "/openapi.json",
            get({
                let spec = openapi_spec.clone();
                move || async move { axum::Json(spec) }
            }),
        )
        .merge(Scalar::with_url("/docs", openapi_spec));

    // Auth middleware must be applied AFTER routes are mounted
    app.layer(middleware::from_fn_with_state(
        Arc::clone(&auth_mode),
        auth_middleware,
    ))
}

async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "ok": true }))
}
