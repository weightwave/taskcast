use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use serde::Deserialize;
use serde_json::json;
use taskcast_core::{
    CreateTaskInput, EngineError, EventQueryOptions, Level, PublishEventInput, SeriesMode,
    SinceCursor, TaskEngine, TaskError, TaskStatus,
};

use crate::auth::{check_scope, AuthContext};
use crate::error::AppError;

// ─── Request Bodies ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskBody {
    pub id: Option<String>,
    pub r#type: Option<String>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub ttl: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionBody {
    pub status: TaskStatus,
    pub result: Option<HashMap<String, serde_json::Value>>,
    pub error: Option<TaskErrorBody>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskErrorBody {
    pub code: Option<String>,
    pub message: String,
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishEventBody {
    pub r#type: String,
    pub level: Level,
    pub data: serde_json::Value,
    pub series_id: Option<String>,
    pub series_mode: Option<SeriesMode>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EventsBody {
    Batch(Vec<PublishEventBody>),
    Single(PublishEventBody),
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(rename = "since.index")]
    pub since_index: Option<u64>,
    #[serde(rename = "since.timestamp")]
    pub since_timestamp: Option<f64>,
    #[serde(rename = "since.id")]
    pub since_id: Option<String>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

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
        ..Default::default()
    };

    let task = engine.create_task(input).await?;
    Ok((StatusCode::CREATED, axum::Json(task)))
}

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

    Ok(axum::Json(task))
}

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

    let payload = if body.result.is_some() || body.error.is_some() {
        let error = body.error.map(|e| TaskError {
            code: e.code,
            message: e.message,
            details: e.details,
        });
        Some(taskcast_core::TransitionPayload {
            result: body.result,
            error,
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
