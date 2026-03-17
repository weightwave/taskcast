use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, Sse};
use axum::Extension;
use futures::stream::Stream;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;

use taskcast_core::{
    apply_filtered_index, matches_filter, Level, SSEEnvelope, SeriesFormat, SinceCursor,
    SubscribeFilter, TaskEngine, TaskEvent, TaskStatus,
};

use crate::auth::{check_scope, AuthContext};
use crate::error::AppError;

// ─── Subscriber Tracking ─────────────────────────────────────────────────────

/// Shared subscriber count state, passed via Axum Extension to avoid module-level globals.
pub type SubscriberCounts = Arc<Mutex<HashMap<String, usize>>>;

pub fn create_subscriber_counts() -> SubscriberCounts {
    Arc::new(Mutex::new(HashMap::new()))
}

pub async fn get_subscriber_count(counts: &SubscriberCounts, task_id: &str) -> usize {
    let counts = counts.lock().await;
    counts.get(task_id).copied().unwrap_or(0)
}

async fn increment_subscriber_count(counts: &SubscriberCounts, task_id: &str) {
    let mut counts = counts.lock().await;
    *counts.entry(task_id.to_string()).or_insert(0) += 1;
}

async fn decrement_subscriber_count(counts: &SubscriberCounts, task_id: &str) {
    let mut counts = counts.lock().await;
    if let Some(count) = counts.get_mut(task_id) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(task_id);
        }
    }
}

// ─── Query Parameters ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SseQuery {
    pub types: Option<String>,
    pub levels: Option<String>,
    #[serde(rename = "includeStatus")]
    pub include_status: Option<String>,
    pub wrap: Option<String>,
    #[serde(rename = "seriesFormat")]
    pub series_format: Option<String>,
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

    let series_format = query.series_format.as_ref().and_then(|s| match s.as_str() {
        "delta" => Some(SeriesFormat::Delta),
        "accumulated" => Some(SeriesFormat::Accumulated),
        _ => None,
    });

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
        series_format,
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
        series_acc_field: event.series_acc_field.clone(),
        series_snapshot: event.series_snapshot,
    }
}

// ─── Terminal Status Check ──────────────────────────────────────────────────

fn is_terminal_status(status: &TaskStatus) -> bool {
    taskcast_core::state_machine::is_terminal(status)
}

// ─── SSE Handler ────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/tasks/{task_id}/events",
    tag = "Events",
    summary = "Subscribe to task events via SSE",
    description = "Server-Sent Events stream. Replays history then streams live events.",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID"), SseQuery),
    responses(
        (status = 200, description = "SSE event stream (text/event-stream)"),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn sse_events(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Extension(subscriber_counts): Extension<SubscriberCounts>,
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
    let sub_counts = subscriber_counts.clone();

    let series_format = filter
        .series_format
        .clone()
        .unwrap_or(SeriesFormat::Delta);

    tokio::spawn(async move {
        increment_subscriber_count(&sub_counts, &task_id_clone).await;

        let series_format_for_send = series_format.clone();

        // Helper closures
        let send_event = move |tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
                          event: &TaskEvent,
                          filtered_index: u64,
                          wrap: bool| {
            let mut event_to_send = event.clone();

            // For accumulated format, swap data with accumulated data if present
            if series_format_for_send == SeriesFormat::Accumulated {
                if let Some(ref acc_data) = event._accumulated_data {
                    event_to_send.data = acc_data.clone();
                }
            }
            // Strip transient field before sending
            event_to_send._accumulated_data = None;

            let payload: serde_json::Value = if wrap {
                serde_json::to_value(to_envelope(&event_to_send, filtered_index)).unwrap()
            } else {
                serde_json::to_value(&event_to_send).unwrap()
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
            Err(_) => {
                decrement_subscriber_count(&sub_counts, &task_id_clone).await;
                return;
            }
        };

        let has_since_cursor = filter.since.is_some();

        // Build replay events with late-join snapshot collapse
        let replay_events = if !has_since_cursor {
            let engine_ref = Arc::clone(&engine);
            taskcast_core::series::collapse_accumulate_series(
                &history,
                |tid: &str, sid: &str| {
                    let eng = Arc::clone(&engine_ref);
                    let tid = tid.to_string();
                    let sid = sid.to_string();
                    async move {
                        eng.get_series_latest(&tid, &sid).await
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }
                },
            ).await.unwrap_or(history)
        } else {
            history
        };

        let filtered = apply_filtered_index(&replay_events, &filter);
        for fe in &filtered {
            send_event(&tx, &fe.event, fe.filtered_index, wrap);
        }

        // If already terminal, send done and return
        if is_terminal_status(&task_status) {
            let status_str =
                serde_json::to_value(&task_status).unwrap_or(serde_json::Value::Null);
            send_done(&tx, status_str.as_str().unwrap_or("completed"));
            decrement_subscriber_count(&sub_counts, &task_id_clone).await;
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

        // Wait for terminal event OR client disconnect (tx.closed() resolves when rx is dropped)
        tokio::select! {
            _ = done_rx => {}
            _ = tx.closed() => {}
        }
        unsub();
        decrement_subscriber_count(&sub_counts, &task_id_clone).await;
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(stream))
}

// ─── Unit Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use taskcast_core::{Level, SeriesFormat, SeriesMode};

    // ── parse_filter: seriesFormat ──────────────────────────────────────────

    #[test]
    fn parse_filter_series_format_delta() {
        let query = SseQuery {
            types: None,
            levels: None,
            include_status: None,
            wrap: None,
            series_format: Some("delta".to_string()),
            since_id: None,
            since_index: None,
            since_timestamp: None,
        };
        let filter = parse_filter(&query);
        assert_eq!(filter.series_format, Some(SeriesFormat::Delta));
    }

    #[test]
    fn parse_filter_series_format_accumulated() {
        let query = SseQuery {
            types: None,
            levels: None,
            include_status: None,
            wrap: None,
            series_format: Some("accumulated".to_string()),
            since_id: None,
            since_index: None,
            since_timestamp: None,
        };
        let filter = parse_filter(&query);
        assert_eq!(filter.series_format, Some(SeriesFormat::Accumulated));
    }

    #[test]
    fn parse_filter_series_format_invalid_returns_none() {
        let query = SseQuery {
            types: None,
            levels: None,
            include_status: None,
            wrap: None,
            series_format: Some("bogus".to_string()),
            since_id: None,
            since_index: None,
            since_timestamp: None,
        };
        let filter = parse_filter(&query);
        assert_eq!(filter.series_format, None);
    }

    #[test]
    fn parse_filter_series_format_none() {
        let query = SseQuery {
            types: None,
            levels: None,
            include_status: None,
            wrap: None,
            series_format: None,
            since_id: None,
            since_index: None,
            since_timestamp: None,
        };
        let filter = parse_filter(&query);
        assert_eq!(filter.series_format, None);
    }

    // ── to_envelope ─────────────────────────────────────────────────────────

    #[test]
    fn to_envelope_includes_series_snapshot_true() {
        let event = TaskEvent {
            id: "e1".to_string(),
            task_id: "t1".to_string(),
            index: 5,
            timestamp: 1000.0,
            r#type: "llm.chunk".to_string(),
            level: Level::Info,
            data: json!({"text": "hello world"}),
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
            series_acc_field: Some("text".to_string()),
            series_snapshot: Some(true),
            _accumulated_data: None,
        };
        let envelope = to_envelope(&event, 3);
        assert_eq!(envelope.series_snapshot, Some(true));
        assert_eq!(envelope.filtered_index, 3);
        assert_eq!(envelope.raw_index, 5);
        assert_eq!(envelope.event_id, "e1");
        assert_eq!(envelope.task_id, "t1");
        assert_eq!(envelope.series_id, Some("s1".to_string()));
        assert_eq!(envelope.series_mode, Some(SeriesMode::Accumulate));
        assert_eq!(envelope.series_acc_field, Some("text".to_string()));
    }

    #[test]
    fn to_envelope_series_snapshot_none_when_absent() {
        let event = TaskEvent {
            id: "e2".to_string(),
            task_id: "t1".to_string(),
            index: 0,
            timestamp: 500.0,
            r#type: "log".to_string(),
            level: Level::Debug,
            data: json!("msg"),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        };
        let envelope = to_envelope(&event, 0);
        assert_eq!(envelope.series_snapshot, None);
        assert_eq!(envelope.series_id, None);
        assert_eq!(envelope.series_mode, None);
    }

    #[test]
    fn to_envelope_preserves_all_fields() {
        let event = TaskEvent {
            id: "evt_abc".to_string(),
            task_id: "task_xyz".to_string(),
            index: 42,
            timestamp: 1234567890.123,
            r#type: "progress".to_string(),
            level: Level::Warn,
            data: json!({"pct": 75}),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        };
        let envelope = to_envelope(&event, 10);
        assert_eq!(envelope.filtered_index, 10);
        assert_eq!(envelope.raw_index, 42);
        assert_eq!(envelope.event_id, "evt_abc");
        assert_eq!(envelope.task_id, "task_xyz");
        assert_eq!(envelope.r#type, "progress");
        assert_eq!(envelope.timestamp, 1234567890.123);
        assert_eq!(envelope.level, Level::Warn);
        assert_eq!(envelope.data, json!({"pct": 75}));
    }

    #[test]
    fn to_envelope_serializes_series_snapshot_in_json() {
        let event = TaskEvent {
            id: "e3".to_string(),
            task_id: "t3".to_string(),
            index: 1,
            timestamp: 100.0,
            r#type: "chunk".to_string(),
            level: Level::Info,
            data: json!("acc"),
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
            series_acc_field: None,
            series_snapshot: Some(true),
            _accumulated_data: None,
        };
        let envelope = to_envelope(&event, 0);
        let json_val = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json_val["seriesSnapshot"], true);
    }

    #[test]
    fn to_envelope_omits_series_snapshot_when_none() {
        let event = TaskEvent {
            id: "e4".to_string(),
            task_id: "t4".to_string(),
            index: 0,
            timestamp: 0.0,
            r#type: "log".to_string(),
            level: Level::Info,
            data: json!(null),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        };
        let envelope = to_envelope(&event, 0);
        let json_val = serde_json::to_value(&envelope).unwrap();
        assert!(json_val.get("seriesSnapshot").is_none());
    }
}
