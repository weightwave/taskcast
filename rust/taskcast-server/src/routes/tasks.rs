use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use serde::Deserialize;
use serde_json::json;
use taskcast_core::{
    AssignMode, BlockedRequest, CleanupConfig, CreateTaskInput, DisconnectPolicy, EngineError,
    EventQueryOptions, Level, PublishEventInput, SeriesMode, SinceCursor, TaskAuthConfig,
    TaskEngine, TaskError, TaskStatus, TransitionPayload, WebhookConfig,
};

use crate::auth::{check_scope, AuthContext};
use crate::error::AppError;
use crate::routes::sse::get_subscriber_count;

// ─── Request Bodies ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskBody {
    pub id: Option<String>,
    pub r#type: Option<String>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub ttl: Option<u64>,
    pub webhooks: Option<Vec<WebhookConfig>>,
    pub cleanup: Option<CleanupConfig>,
    pub auth_config: Option<TaskAuthConfig>,
    pub tags: Option<Vec<String>>,
    pub assign_mode: Option<AssignMode>,
    pub cost: Option<u32>,
    pub disconnect_policy: Option<DisconnectPolicy>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransitionBody {
    pub status: TaskStatus,
    pub result: Option<HashMap<String, serde_json::Value>>,
    pub error: Option<TaskErrorBody>,
    pub reason: Option<String>,
    pub ttl: Option<u64>,
    pub resume_after_ms: Option<f64>,
    pub blocked_request: Option<BlockedRequest>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskErrorBody {
    pub code: Option<String>,
    pub message: String,
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublishEventBody {
    pub r#type: String,
    pub level: Level,
    pub data: serde_json::Value,
    pub series_id: Option<String>,
    pub series_mode: Option<SeriesMode>,
    pub series_acc_field: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EventsBody {
    Batch(Vec<PublishEventBody>),
    Single(PublishEventBody),
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct HistoryQuery {
    #[serde(rename = "since.index")]
    pub since_index: Option<u64>,
    #[serde(rename = "since.timestamp")]
    pub since_timestamp: Option<f64>,
    #[serde(rename = "since.id")]
    pub since_id: Option<String>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/tasks",
    tag = "Tasks",
    summary = "Create a new task",
    security(("Bearer" = [])),
    request_body = CreateTaskBody,
    responses(
        (status = 201, description = "Task created", body = taskcast_core::Task),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn create_task(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    axum::Json(body): axum::Json<CreateTaskBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(
        &auth,
        taskcast_core::PermissionScope::TaskCreate,
        None,
    ) {
        return Err(AppError::Forbidden);
    }

    let input = CreateTaskInput {
        id: body.id,
        r#type: body.r#type,
        params: body.params,
        metadata: body.metadata,
        ttl: body.ttl,
        webhooks: body.webhooks,
        cleanup: body.cleanup,
        auth_config: body.auth_config,
        tags: body.tags,
        assign_mode: body.assign_mode,
        cost: body.cost,
        disconnect_policy: body.disconnect_policy,
    };

    let task = engine.create_task(input).await?;
    Ok((StatusCode::CREATED, axum::Json(task)))
}

#[utoipa::path(
    get,
    path = "/tasks/{task_id}",
    tag = "Tasks",
    summary = "Get task by ID",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID")),
    responses(
        (status = 200, description = "Task details", body = taskcast_core::Task),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn get_task(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(
        &auth,
        taskcast_core::PermissionScope::EventSubscribe,
        Some(&task_id),
    ) {
        return Err(AppError::Forbidden);
    }

    let task = engine
        .get_task(&task_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

    let subscriber_count = get_subscriber_count(&task_id).await;
    let mut task_json = serde_json::to_value(&task).unwrap();
    if let Some(obj) = task_json.as_object_mut() {
        obj.insert("hot".to_string(), json!(true));
        obj.insert("subscriberCount".to_string(), json!(subscriber_count));
    }

    Ok(axum::Json(task_json))
}

#[utoipa::path(
    patch,
    path = "/tasks/{task_id}/status",
    tag = "Tasks",
    summary = "Transition task status",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID")),
    request_body = TransitionBody,
    responses(
        (status = 200, description = "Updated task", body = taskcast_core::Task),
        (status = 400, description = "Invalid transition"),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn transition_task(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    axum::Json(body): axum::Json<TransitionBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(
        &auth,
        taskcast_core::PermissionScope::TaskManage,
        Some(&task_id),
    ) {
        return Err(AppError::Forbidden);
    }

    let payload = if body.result.is_some()
        || body.error.is_some()
        || body.reason.is_some()
        || body.ttl.is_some()
        || body.resume_after_ms.is_some()
        || body.blocked_request.is_some()
    {
        let error = body.error.map(|e| TaskError {
            code: e.code,
            message: e.message,
            details: e.details,
        });
        Some(TransitionPayload {
            result: body.result,
            error,
            reason: body.reason,
            ttl: body.ttl,
            resume_after_ms: body.resume_after_ms,
            blocked_request: body.blocked_request,
        })
    } else {
        None
    };

    let task = engine
        .transition_task(&task_id, body.status, payload)
        .await
        .map_err(|e| match &e {
            EngineError::TaskNotFound(_) => AppError::NotFound(e.to_string()),
            EngineError::InvalidTransition { .. } => AppError::BadRequest(e.to_string()),
            EngineError::TaskTerminal(_) => AppError::BadRequest(e.to_string()),
            _ => AppError::Engine(e),
        })?;

    Ok(axum::Json(task))
}

#[utoipa::path(
    post,
    path = "/tasks/{task_id}/events",
    tag = "Events",
    summary = "Publish events to a task",
    description = "Supports single event or batch (array) publishing.",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID")),
    responses(
        (status = 201, description = "Events published"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn publish_events(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(
        &auth,
        taskcast_core::PermissionScope::EventPublish,
        Some(&task_id),
    ) {
        return Err(AppError::Forbidden);
    }

    let is_batch = body.is_array();

    let inputs: Vec<PublishEventBody> = if is_batch {
        serde_json::from_value(body)
            .map_err(|e| AppError::BadRequest(e.to_string()))?
    } else {
        let single: PublishEventBody = serde_json::from_value(body)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        vec![single]
    };

    let mut events = Vec::new();
    for input in inputs {
        let event_input = PublishEventInput {
            r#type: input.r#type,
            level: input.level,
            data: input.data,
            series_id: input.series_id,
            series_mode: input.series_mode,
            series_acc_field: input.series_acc_field,
        };
        let event = engine
            .publish_event(&task_id, event_input)
            .await
            .map_err(|e| match &e {
                EngineError::TaskNotFound(_) => AppError::NotFound(e.to_string()),
                EngineError::TaskTerminal(_) => AppError::BadRequest(e.to_string()),
                _ => AppError::Engine(e),
            })?;
        events.push(serde_json::to_value(&event).unwrap());
    }

    let body = if is_batch {
        json!(events)
    } else {
        events.into_iter().next().unwrap()
    };

    Ok((StatusCode::CREATED, axum::Json(body)))
}

#[utoipa::path(
    get,
    path = "/tasks/{task_id}/events/history",
    tag = "Events",
    summary = "Query event history",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID"), HistoryQuery),
    responses(
        (status = 200, description = "Event list", body = Vec<taskcast_core::TaskEvent>),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn get_event_history(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(
        &auth,
        taskcast_core::PermissionScope::EventHistory,
        Some(&task_id),
    ) {
        return Err(AppError::Forbidden);
    }

    // Check task exists
    let _task = engine
        .get_task(&task_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

    let opts = if query.since_id.is_some()
        || query.since_index.is_some()
        || query.since_timestamp.is_some()
    {
        Some(EventQueryOptions {
            since: Some(SinceCursor {
                id: query.since_id,
                index: query.since_index,
                timestamp: query.since_timestamp,
            }),
            limit: None,
        })
    } else {
        None
    };

    let events = engine.get_events(&task_id, opts).await?;
    Ok(axum::Json(events))
}

// ─── Resolve / Request Handlers ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveBody {
    pub data: serde_json::Value,
}

pub async fn resolve_task(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    axum::Json(body): axum::Json<ResolveBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(
        &auth,
        taskcast_core::PermissionScope::TaskResolve,
        Some(&task_id),
    ) {
        return Err(AppError::Forbidden);
    }

    let task = engine
        .get_task(&task_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

    if task.status != TaskStatus::Blocked {
        return Err(AppError::BadRequest("Task is not blocked".to_string()));
    }

    let result = if body.data.is_object() {
        let map: HashMap<String, serde_json::Value> =
            serde_json::from_value(body.data).unwrap_or_default();
        Some(map)
    } else {
        let mut map = HashMap::new();
        map.insert("resolution".to_string(), body.data);
        Some(map)
    };

    let updated = engine
        .transition_task(
            &task_id,
            TaskStatus::Running,
            Some(TransitionPayload {
                result,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(axum::Json(updated))
}

pub async fn get_blocked_request(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(
        &auth,
        taskcast_core::PermissionScope::TaskResolve,
        Some(&task_id),
    ) {
        return Err(AppError::Forbidden);
    }

    let task = engine
        .get_task(&task_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

    if task.status != TaskStatus::Blocked {
        return Err(AppError::NotFound("No blocked request".to_string()));
    }

    match task.blocked_request {
        Some(request) => Ok(axum::Json(serde_json::to_value(request).unwrap())),
        None => Err(AppError::NotFound("No blocked request".to_string())),
    }
}
