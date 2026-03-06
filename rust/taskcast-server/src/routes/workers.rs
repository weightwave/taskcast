use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use axum::Extension;
use serde::Deserialize;
use serde_json::json;
use taskcast_core::worker_manager::{DeclineOptions, WorkerManager, WorkerUpdate, WorkerUpdateStatus};
use taskcast_core::PermissionScope;

use crate::auth::{check_scope, AuthContext};
use crate::error::AppError;

// ─── Query Parameters ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct PullQuery {
    pub worker_id: String,
    pub weight: Option<u32>,
    /// Long-poll timeout in milliseconds (default: 30000)
    pub timeout: Option<u64>,
}

// ─── Request Bodies ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeclineBody {
    pub worker_id: String,
    #[serde(default)]
    pub blacklist: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerStatusUpdateBody {
    pub status: WorkerStatusUpdateValue,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum WorkerStatusUpdateValue {
    Draining,
    Idle,
}

// ─── Handlers ───────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/workers",
    tag = "Workers",
    summary = "List all workers",
    security(("Bearer" = [])),
    responses(
        (status = 200, description = "Worker list", body = Vec<taskcast_core::Worker>),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn list_workers(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }

    let workers = manager.list_workers(None).await.map_err(manager_error)?;
    Ok(axum::Json(json!({ "workers": workers })))
}

#[utoipa::path(
    get,
    path = "/workers/pull",
    tag = "Workers",
    summary = "Long-poll for task assignment",
    security(("Bearer" = [])),
    params(PullQuery),
    responses(
        (status = 200, description = "Task assigned"),
        (status = 204, description = "Timeout, no task"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn pull_task(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<PullQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }

    let worker_id = &query.worker_id;

    // Enforce auth.worker_id matches requested workerId
    if let Some(ref token_worker_id) = auth.worker_id {
        if token_worker_id != worker_id {
            return Err(AppError::Forbidden);
        }
    }

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

    // Wait for task with configurable timeout (default 30s)
    let timeout_ms = query.timeout.unwrap_or(30_000);
    let task = manager
        .wait_for_task(worker_id, timeout_ms)
        .await
        .map_err(manager_error)?;

    match task {
        Some(task) => Ok((StatusCode::OK, axum::Json(json!(task))).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

#[utoipa::path(
    get,
    path = "/workers/{worker_id}",
    tag = "Workers",
    summary = "Get worker by ID",
    security(("Bearer" = [])),
    params(("worker_id" = String, Path, description = "Worker ID")),
    responses(
        (status = 200, description = "Worker details", body = taskcast_core::Worker),
        (status = 404, description = "Not found"),
        (status = 403, description = "Forbidden"),
    )
)]
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

#[utoipa::path(
    delete,
    path = "/workers/{worker_id}",
    tag = "Workers",
    summary = "Delete worker",
    security(("Bearer" = [])),
    params(("worker_id" = String, Path, description = "Worker ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found"),
        (status = 403, description = "Forbidden"),
    )
)]
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

#[utoipa::path(
    patch,
    path = "/workers/{worker_id}/status",
    tag = "Workers",
    summary = "Update worker status (drain/resume)",
    description = "Set worker to draining or idle. Cannot manually set busy.",
    security(("Bearer" = [])),
    params(("worker_id" = String, Path, description = "Worker ID")),
    request_body = WorkerStatusUpdateBody,
    responses(
        (status = 200, description = "Updated worker", body = taskcast_core::Worker),
        (status = 400, description = "Invalid status"),
        (status = 404, description = "Not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn update_worker_status(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(worker_id): Path<String>,
    axum::Json(body): axum::Json<WorkerStatusUpdateBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }

    let update_status = match body.status {
        WorkerStatusUpdateValue::Draining => WorkerUpdateStatus::Draining,
        WorkerStatusUpdateValue::Idle => WorkerUpdateStatus::Idle,
    };

    let update = WorkerUpdate {
        status: Some(update_status),
        ..Default::default()
    };

    let worker = manager
        .update_worker(&worker_id, update)
        .await
        .map_err(manager_error)?
        .ok_or_else(|| AppError::NotFound("Worker not found".to_string()))?;

    Ok(axum::Json(worker))
}

#[utoipa::path(
    post,
    path = "/workers/tasks/{task_id}/decline",
    tag = "Workers",
    summary = "Worker declines a task",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID")),
    request_body = DeclineBody,
    responses(
        (status = 200, description = "Declined"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn decline_task(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    axum::Json(body): axum::Json<DeclineBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }

    // Enforce auth.worker_id matches requested workerId
    if let Some(ref token_worker_id) = auth.worker_id {
        if token_worker_id != &body.worker_id {
            return Err(AppError::Forbidden);
        }
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
        .route("/{worker_id}/status", patch(update_worker_status))
        .route("/tasks/{task_id}/decline", post(decline_task))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn manager_error(e: Box<dyn std::error::Error + Send + Sync>) -> AppError {
    AppError::BadRequest(e.to_string())
}
