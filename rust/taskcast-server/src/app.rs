use std::sync::Arc;

use axum::middleware;
use axum::routing::{get, patch, post};
use axum::Router;
use taskcast_core::TaskEngine;

use crate::auth::{auth_middleware, AuthMode};
use crate::routes::{sse, tasks};

/// Shared application state available to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<TaskEngine>,
    pub auth_mode: Arc<AuthMode>,
}

/// Create the Axum router with all taskcast routes mounted.
pub fn create_app(engine: Arc<TaskEngine>, auth_mode: AuthMode) -> Router {
    let auth_mode = Arc::new(auth_mode);

    let task_routes = Router::new()
        .route("/", post(tasks::create_task))
        .route("/{task_id}", get(tasks::get_task))
        .route("/{task_id}/status", patch(tasks::transition_task))
        .route("/{task_id}/events", post(tasks::publish_events).get(sse::sse_events))
        .route("/{task_id}/events/history", get(tasks::get_event_history))
        .with_state(Arc::clone(&engine));

    Router::new()
        .nest("/tasks", task_routes)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth_mode),
            auth_middleware,
        ))
}
