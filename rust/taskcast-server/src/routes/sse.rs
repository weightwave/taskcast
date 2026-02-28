use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, Sse};
use axum::Extension;
use futures::stream::Stream;
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;

use taskcast_core::{
    apply_filtered_index, matches_filter, Level, SSEEnvelope, SinceCursor, SubscribeFilter,
    TaskEngine, TaskEvent, TaskStatus,
};

use crate::auth::{check_scope, AuthContext};
use crate::error::AppError;

// ─── Query Parameters ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SseQuery {
    pub types: Option<String>,
    pub levels: Option<String>,
    #[serde(rename = "includeStatus")]
    pub include_status: Option<String>,
    pub wrap: Option<String>,
    #[serde(rename = "since.id")]
    pub since_id: Option<String>,
    #[serde(rename = "since.index")]
    pub since_index: Option<String>,
    #[serde(rename = "since.timestamp")]
    pub since_timestamp: Option<String>,
}

// ─── Filter Parsing ─────────────────────────────────────────────────────────

fn parse_filter(query: &SseQuery) -> SubscribeFilter {
    let types = query
        .types
        .as_ref()
        .map(|t| t.split(',').filter(|s| !s.is_empty()).map(String::from).collect());

    let levels = query.levels.as_ref().map(|l| {
        l.split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| serde_json::from_value(serde_json::Value::String(s.to_string())).ok())
            .collect::<Vec<Level>>()
    });

    let include_status = query.include_status.as_ref().map(|v| v != "false");
    let wrap = query.wrap.as_ref().map(|v| v != "false");

    let since = if query.since_id.is_some()
        || query.since_index.is_some()
        || query.since_timestamp.is_some()
    {
        Some(SinceCursor {
            id: query.since_id.clone(),
            index: query.since_index.as_ref().and_then(|s| s.parse().ok()),
            timestamp: query.since_timestamp.as_ref().and_then(|s| s.parse().ok()),
        })
    } else {
        None
    };

    SubscribeFilter {
        types,
        levels,
        include_status,
        wrap,
        since,
    }
}

// ─── Envelope Conversion ────────────────────────────────────────────────────

fn to_envelope(event: &TaskEvent, filtered_index: u64) -> SSEEnvelope {
    SSEEnvelope {
        filtered_index,
        raw_index: event.index,
        event_id: event.id.clone(),
        task_id: event.task_id.clone(),
        r#type: event.r#type.clone(),
        timestamp: event.timestamp,
        level: event.level.clone(),
        data: event.data.clone(),
        series_id: event.series_id.clone(),
        series_mode: event.series_mode.clone(),
    }
}

// ─── Terminal Status Check ──────────────────────────────────────────────────

fn is_terminal_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Timeout | TaskStatus::Cancelled
    )
}

// ─── SSE Handler ────────────────────────────────────────────────────────────

pub async fn sse_events(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    Query(query): Query<SseQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
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

    let filter = parse_filter(&query);
    let wrap = filter.wrap.unwrap_or(true);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);

    let task_status = task.status.clone();
    let task_id_clone = task_id.clone();

    tokio::spawn(async move {
        // Helper closures
        let send_event = |tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
                          event: &TaskEvent,
                          filtered_index: u64,
                          wrap: bool| {
            let payload: serde_json::Value = if wrap {
                serde_json::to_value(to_envelope(event, filtered_index)).unwrap()
            } else {
                serde_json::to_value(event).unwrap()
            };
            let sse_event = Event::default()
                .event("taskcast.event")
                .data(serde_json::to_string(&payload).unwrap())
                .id(event.id.clone());
            let _ = tx.try_send(Ok(sse_event));
        };

        let send_done =
            |tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>, reason: &str| {
                let data = serde_json::json!({ "reason": reason });
                let sse_event = Event::default()
                    .event("taskcast.done")
                    .data(serde_json::to_string(&data).unwrap());
                let _ = tx.try_send(Ok(sse_event));
            };

        // Replay history
        let history = match engine.get_events(&task_id_clone, None).await {
            Ok(events) => events,
            Err(_) => return,
        };

        let filtered = apply_filtered_index(&history, &filter);
        for fe in &filtered {
            send_event(&tx, &fe.event, fe.filtered_index, wrap);
        }

        // If already terminal, send done and return
        if is_terminal_status(&task_status) {
            let status_str =
                serde_json::to_value(&task_status).unwrap_or(serde_json::Value::Null);
            send_done(&tx, status_str.as_str().unwrap_or("completed"));
            return;
        }

        // Subscribe to live events
        let next_filtered_index = if let Some(last) = filtered.last() {
            last.filtered_index + 1
        } else {
            0
        };

        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let done_tx = Arc::new(tokio::sync::Mutex::new(Some(done_tx)));

        let filter_for_sub = filter.clone();
        let tx_for_sub = tx.clone();
        let done_tx_for_sub = Arc::clone(&done_tx);

        // We need to use a shared mutable counter for the subscription callback
        let next_idx = Arc::new(std::sync::atomic::AtomicU64::new(next_filtered_index));

        let unsub = engine
            .subscribe(
                &task_id_clone,
                Box::new(move |event| {
                    if !matches_filter(&event, &filter_for_sub) {
                        return;
                    }
                    let idx = next_idx.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    send_event(&tx_for_sub, &event, idx, wrap);

                    if event.r#type == "taskcast:status" {
                        if let Some(status) = event.data.get("status").and_then(|s| s.as_str()) {
                            if matches!(
                                status,
                                "completed" | "failed" | "timeout" | "cancelled"
                            ) {
                                send_done(&tx_for_sub, status);
                                if let Ok(mut guard) = done_tx_for_sub.try_lock() {
                                    if let Some(sender) = guard.take() {
                                        let _ = sender.send(());
                                    }
                                }
                            }
                        }
                    }
                }),
            )
            .await;

        // Wait for terminal event or channel close
        let _ = done_rx.await;
        drop(unsub);
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(stream))
}
