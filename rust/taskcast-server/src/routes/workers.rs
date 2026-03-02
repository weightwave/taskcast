use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Extension;
use serde::Deserialize;
use serde_json::json;
use taskcast_core::worker_manager::{DeclineOptions, WorkerManager, WorkerUpdate};
use taskcast_core::PermissionScope;

use crate::auth::{check_scope, AuthContext};
use crate::error::AppError;

// ─── Query Parameters ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullQuery {
    pub worker_id: String,
    pub weight: Option<u32>,
}

// ─── Request Bodies ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclineBody {
    pub worker_id: String,
    #[serde(default)]
    pub blacklist: Option<bool>,
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// GET / — List all workers (scope: WorkerManage)
pub async fn list_workers(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }

    let workers = manager.list_workers(None).await.map_err(manager_error)?;
    Ok(axum::Json(workers))
}

/// GET /pull — Long-poll for task (scope: WorkerConnect)
pub async fn pull_task(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<PullQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }

    let worker_id = &query.worker_id;

    // If weight provided, update the worker
    if let Some(weight) = query.weight {
        let update = WorkerUpdate {
            weight: Some(weight),
            ..Default::default()
        };
        manager
            .update_worker(worker_id, update)
            .await
            .map_err(manager_error)?;
    }

    // Heartbeat
    manager
        .heartbeat(worker_id)
        .await
        .map_err(manager_error)?;

    // Wait for task with 30s timeout
    let task = manager
        .wait_for_task(worker_id, 30_000)
        .await
        .map_err(manager_error)?;

    match task {
        Some(task) => Ok((StatusCode::OK, axum::Json(json!(task))).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

/// GET /:workerId — Get single worker (scope: WorkerManage)
pub async fn get_worker(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(worker_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }

    let worker = manager
        .get_worker(&worker_id)
        .await
        .map_err(manager_error)?
        .ok_or_else(|| AppError::NotFound("Worker not found".to_string()))?;

    Ok(axum::Json(worker))
}

/// DELETE /:workerId — Force disconnect (scope: WorkerManage)
pub async fn delete_worker(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(worker_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }

    let worker = manager
        .get_worker(&worker_id)
        .await
        .map_err(manager_error)?;
    if worker.is_none() {
        return Err(AppError::NotFound(format!("Worker {}", worker_id)));
    }

    manager
        .unregister_worker(&worker_id)
        .await
        .map_err(manager_error)?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /tasks/:taskId/decline — Decline task (scope: WorkerConnect)
pub async fn decline_task(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    axum::Json(body): axum::Json<DeclineBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }

    let opts = body.blacklist.map(|blacklist| DeclineOptions { blacklist });

    manager
        .decline_task(&task_id, &body.worker_id, opts)
        .await
        .map_err(manager_error)?;

    Ok(axum::Json(json!({ "ok": true })))
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn workers_router() -> axum::Router<Arc<WorkerManager>> {
    axum::Router::new()
        .route("/", get(list_workers))
        .route("/pull", get(pull_task))
        .route("/{worker_id}", get(get_worker).delete(delete_worker))
        .route("/tasks/{task_id}/decline", post(decline_task))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn manager_error(e: Box<dyn std::error::Error + Send + Sync>) -> AppError {
    AppError::BadRequest(e.to_string())
}
