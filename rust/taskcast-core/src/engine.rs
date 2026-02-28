use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::series::process_series;
use crate::state_machine::{can_transition, is_terminal};
use crate::types::{
    BroadcastProvider, CleanupConfig, EventQueryOptions, Level, LongTermStore, ShortTermStore,
    Task, TaskAuthConfig, TaskcastHooks, TaskError, TaskEvent, TaskStatus, WebhookConfig,
};

// ─── Error ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Invalid transition: {from:?} \u{2192} {to:?}")]
    InvalidTransition { from: TaskStatus, to: TaskStatus },

    #[error("Cannot publish to task in terminal status: {0:?}")]
    TaskTerminal(TaskStatus),

    #[error("{0}")]
    Store(#[from] Box<dyn std::error::Error + Send + Sync>),
}

// ─── Input types ─────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct CreateTaskInput {
    pub id: Option<String>,
    pub r#type: Option<String>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub ttl: Option<u64>,
    pub webhooks: Option<Vec<WebhookConfig>>,
    pub cleanup: Option<CleanupConfig>,
    pub auth_config: Option<TaskAuthConfig>,
}

pub struct PublishEventInput {
    pub r#type: String,
    pub level: Level,
    pub data: serde_json::Value,
    pub series_id: Option<String>,
    pub series_mode: Option<crate::types::SeriesMode>,
}

pub struct TransitionPayload {
    pub result: Option<HashMap<String, serde_json::Value>>,
    pub error: Option<TaskError>,
}

// ─── TaskEngineOptions ───────────────────────────────────────────────────────

pub struct TaskEngineOptions {
    pub short_term: Arc<dyn ShortTermStore>,
    pub broadcast: Arc<dyn BroadcastProvider>,
    pub long_term: Option<Arc<dyn LongTermStore>>,
    pub hooks: Option<Arc<dyn TaskcastHooks>>,
}

// ─── TaskEngine ──────────────────────────────────────────────────────────────

pub struct TaskEngine {
    short_term: Arc<dyn ShortTermStore>,
    broadcast: Arc<dyn BroadcastProvider>,
    long_term: Option<Arc<dyn LongTermStore>>,
    hooks: Option<Arc<dyn TaskcastHooks>>,
}

impl TaskEngine {
    pub fn new(opts: TaskEngineOptions) -> Self {
        Self {
            short_term: opts.short_term,
            broadcast: opts.broadcast,
            long_term: opts.long_term,
            hooks: opts.hooks,
        }
    }

    pub async fn create_task(&self, input: CreateTaskInput) -> Result<Task, EngineError> {
        let now = now_millis();
        let task = Task {
            id: input.id.unwrap_or_else(|| ulid::Ulid::new().to_string()),
            status: TaskStatus::Pending,
            created_at: now,
            updated_at: now,
            r#type: input.r#type,
            params: input.params,
            metadata: input.metadata,
            ttl: input.ttl,
            webhooks: input.webhooks,
            cleanup: input.cleanup,
            auth_config: input.auth_config,
            result: None,
            error: None,
            completed_at: None,
        };

        self.short_term.save_task(task.clone()).await?;

        if let Some(ref long_term) = self.long_term {
            long_term.save_task(task.clone()).await?;
        }

        if let Some(ttl) = task.ttl {
            self.short_term.set_ttl(&task.id, ttl).await?;
        }

        Ok(task)
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Option<Task>, EngineError> {
        let from_short = self.short_term.get_task(task_id).await?;
        if from_short.is_some() {
            return Ok(from_short);
        }
        if let Some(ref long_term) = self.long_term {
            return Ok(long_term.get_task(task_id).await?);
        }
        Ok(None)
    }

    pub async fn transition_task(
        &self,
        task_id: &str,
        to: TaskStatus,
        payload: Option<TransitionPayload>,
    ) -> Result<Task, EngineError> {
        let task = self
            .get_task(task_id)
            .await?
            .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?;

        if !can_transition(&task.status, &to) {
            return Err(EngineError::InvalidTransition {
                from: task.status.clone(),
                to,
            });
        }

        let now = now_millis();
        let new_result = payload
            .as_ref()
            .and_then(|p| p.result.clone())
            .or(task.result);
        let new_error = payload.as_ref().and_then(|p| p.error.clone()).or(task.error);
        let new_completed_at = if is_terminal(&to) {
            Some(now)
        } else {
            task.completed_at
        };

        let updated = Task {
            status: to.clone(),
            updated_at: now,
            completed_at: new_completed_at,
            result: new_result,
            error: new_error,
            ..task
        };

        self.short_term.save_task(updated.clone()).await?;

        if let Some(ref long_term) = self.long_term {
            long_term.save_task(updated.clone()).await?;
        }

        self.emit(
            task_id,
            PublishEventInput {
                r#type: "taskcast:status".to_string(),
                level: Level::Info,
                data: serde_json::json!({
                    "status": to,
                    "result": updated.result,
                    "error": updated.error,
                }),
                series_id: None,
                series_mode: None,
            },
        )
        .await?;

        Ok(updated)
    }

    pub async fn publish_event(
        &self,
        task_id: &str,
        input: PublishEventInput,
    ) -> Result<TaskEvent, EngineError> {
        let task = self
            .get_task(task_id)
            .await?
            .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?;

        if is_terminal(&task.status) {
            return Err(EngineError::TaskTerminal(task.status));
        }

        self.emit(task_id, input).await
    }

    pub async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, EngineError> {
        Ok(self.short_term.get_events(task_id, opts).await?)
    }

    pub async fn subscribe(
        &self,
        task_id: &str,
        handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        self.broadcast.subscribe(task_id, handler).await
    }

    // ─── Private ─────────────────────────────────────────────────────────

    async fn emit(
        &self,
        task_id: &str,
        input: PublishEventInput,
    ) -> Result<TaskEvent, EngineError> {
        let index = self.short_term.next_index(task_id).await?;
        let raw = TaskEvent {
            id: ulid::Ulid::new().to_string(),
            task_id: task_id.to_string(),
            index,
            timestamp: now_millis(),
            r#type: input.r#type,
            level: input.level,
            data: input.data,
            series_id: input.series_id,
            series_mode: input.series_mode,
        };

        let event = process_series(raw, self.short_term.as_ref()).await?;

        self.short_term
            .append_event(task_id, event.clone())
            .await?;
        self.broadcast.publish(task_id, event.clone()).await?;

        if let Some(ref long_term) = self.long_term {
            let long_term = Arc::clone(long_term);
            let event_clone = event.clone();
            let hooks = self.hooks.clone();
            tokio::spawn(async move {
                if let Err(err) = long_term.save_event(event_clone.clone()).await {
                    if let Some(hooks) = hooks {
                        hooks.on_event_dropped(&event_clone, &err.to_string());
                    }
                }
            });
        }

        Ok(event)
    }

}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_millis() as f64
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_adapters::{MemoryBroadcastProvider, MemoryShortTermStore};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn make_engine() -> TaskEngine {
        TaskEngine::new(TaskEngineOptions {
            short_term: Arc::new(MemoryShortTermStore::new()),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            long_term: None,
            hooks: None,
        })
    }

    fn make_engine_with_broadcast(broadcast: Arc<MemoryBroadcastProvider>) -> TaskEngine {
        TaskEngine::new(TaskEngineOptions {
            short_term: Arc::new(MemoryShortTermStore::new()),
            broadcast,
            long_term: None,
            hooks: None,
        })
    }

    // ─── create_task ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_task_generates_id_and_sets_status_pending() {
        let engine = make_engine();
        let task = engine.create_task(CreateTaskInput::default()).await.unwrap();

        assert!(!task.id.is_empty());
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.created_at > 0.0);
        assert!(task.updated_at > 0.0);

        // Verify it was saved to the store
        let retrieved = engine.get_task(&task.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, task.id);
    }

    #[tokio::test]
    async fn create_task_with_custom_id() {
        let engine = make_engine();
        let task = engine
            .create_task(CreateTaskInput {
                id: Some("my-custom-id".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(task.id, "my-custom-id");
    }

    #[tokio::test]
    async fn create_task_with_all_optional_fields() {
        let engine = make_engine();
        let mut params = HashMap::new();
        params.insert("url".to_string(), serde_json::json!("https://example.com"));
        let mut metadata = HashMap::new();
        metadata.insert("source".to_string(), serde_json::json!("test"));

        let task = engine
            .create_task(CreateTaskInput {
                id: Some("full-task".to_string()),
                r#type: Some("crawl".to_string()),
                params: Some(params.clone()),
                metadata: Some(metadata.clone()),
                ttl: Some(3600),
                webhooks: Some(vec![WebhookConfig {
                    url: "https://hook.example.com".to_string(),
                    filter: None,
                    secret: None,
                    wrap: None,
                    retry: None,
                }]),
                cleanup: Some(CleanupConfig { rules: vec![] }),
                auth_config: Some(TaskAuthConfig { rules: vec![] }),
            })
            .await
            .unwrap();

        assert_eq!(task.id, "full-task");
        assert_eq!(task.r#type, Some("crawl".to_string()));
        assert_eq!(task.params, Some(params));
        assert_eq!(task.metadata, Some(metadata));
        assert_eq!(task.ttl, Some(3600));
        assert!(task.webhooks.is_some());
        assert!(task.cleanup.is_some());
        assert!(task.auth_config.is_some());
        assert_eq!(task.status, TaskStatus::Pending);
    }

    // ─── get_task ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_task_returns_created_task() {
        let engine = make_engine();
        let task = engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let retrieved = engine.get_task("t1").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, task.id);
    }

    #[tokio::test]
    async fn get_task_returns_none_for_nonexistent() {
        let engine = make_engine();
        let result = engine.get_task("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    // ─── transition_task ─────────────────────────────────────────────────

    #[tokio::test]
    async fn transition_task_pending_to_running() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let updated = engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        assert_eq!(updated.status, TaskStatus::Running);
        assert!(updated.completed_at.is_none()); // Running is not terminal
    }

    #[tokio::test]
    async fn transition_task_running_to_completed() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        let updated = engine
            .transition_task("t1", TaskStatus::Completed, None)
            .await
            .unwrap();

        assert_eq!(updated.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn transition_task_invalid_transition_returns_error() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = engine
            .transition_task("t1", TaskStatus::Completed, None)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidTransition { .. }),
            "Expected InvalidTransition error, got: {err}"
        );
    }

    #[tokio::test]
    async fn transition_task_nonexistent_returns_error() {
        let engine = make_engine();
        let result = engine
            .transition_task("nonexistent", TaskStatus::Running, None)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EngineError::TaskNotFound(_)),
            "Expected TaskNotFound error, got: {err}"
        );
    }

    #[tokio::test]
    async fn transition_task_sets_completed_at_for_terminal() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        let updated = engine
            .transition_task("t1", TaskStatus::Completed, None)
            .await
            .unwrap();

        assert!(updated.completed_at.is_some());
        assert!(updated.completed_at.unwrap() > 0.0);
    }

    #[tokio::test]
    async fn transition_task_preserves_result_and_error_from_payload() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        let mut result_map = HashMap::new();
        result_map.insert("output".to_string(), serde_json::json!("done"));

        let updated = engine
            .transition_task(
                "t1",
                TaskStatus::Completed,
                Some(TransitionPayload {
                    result: Some(result_map.clone()),
                    error: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(updated.result, Some(result_map));
    }

    #[tokio::test]
    async fn transition_task_preserves_error_from_payload() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        let err = TaskError {
            code: Some("ERR_001".to_string()),
            message: "something broke".to_string(),
            details: None,
        };

        let updated = engine
            .transition_task(
                "t1",
                TaskStatus::Failed,
                Some(TransitionPayload {
                    result: None,
                    error: Some(err.clone()),
                }),
            )
            .await
            .unwrap();

        assert_eq!(updated.error, Some(err));
    }

    #[tokio::test]
    async fn transition_task_emits_status_event() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        let events = engine.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].r#type, "taskcast:status");
        assert_eq!(events[0].level, Level::Info);

        let data = &events[0].data;
        assert_eq!(data["status"], "running");
    }

    // ─── publish_event ───────────────────────────────────────────────────

    #[tokio::test]
    async fn publish_event_appends_to_store_and_broadcasts() {
        let broadcast = Arc::new(MemoryBroadcastProvider::new());
        let engine = make_engine_with_broadcast(Arc::clone(&broadcast));

        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        let broadcast_count = Arc::new(AtomicU64::new(0));
        let count_clone = Arc::clone(&broadcast_count);
        let _unsub = broadcast
            .subscribe(
                "t1",
                Box::new(move |_| {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        let event = engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({ "percent": 50 }),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(event.r#type, "progress");
        assert_eq!(event.task_id, "t1");

        // Event should be in the store (transition event + our event)
        let events = engine.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 2); // 1 from transition + 1 from publish
        assert_eq!(events[1].r#type, "progress");

        // Broadcast should have been called
        assert_eq!(broadcast_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn publish_event_rejects_when_task_is_terminal() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Completed, None)
            .await
            .unwrap();

        let result = engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: serde_json::json!(null),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EngineError::TaskTerminal(_)),
            "Expected TaskTerminal error, got: {err}"
        );
    }

    #[tokio::test]
    async fn publish_event_rejects_when_task_does_not_exist() {
        let engine = make_engine();
        let result = engine
            .publish_event(
                "nonexistent",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: serde_json::json!(null),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EngineError::TaskNotFound(_)),
            "Expected TaskNotFound error, got: {err}"
        );
    }

    #[tokio::test]
    async fn publish_event_monotonic_index_increments() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        // The transition already emitted index 0, so publish events start at 1
        let e1 = engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "a".to_string(),
                    level: Level::Info,
                    data: serde_json::json!(null),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await
            .unwrap();

        let e2 = engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "b".to_string(),
                    level: Level::Info,
                    data: serde_json::json!(null),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await
            .unwrap();

        let e3 = engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "c".to_string(),
                    level: Level::Info,
                    data: serde_json::json!(null),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await
            .unwrap();

        // Index 0 was used by the transition_task status event
        assert_eq!(e1.index, 1);
        assert_eq!(e2.index, 2);
        assert_eq!(e3.index, 3);
    }

    // ─── get_events ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_events_returns_events_for_task() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({ "percent": 50 }),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await
            .unwrap();

        let events = engine.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 2); // 1 status + 1 progress
        assert_eq!(events[0].r#type, "taskcast:status");
        assert_eq!(events[1].r#type, "progress");
    }

    // ─── subscribe ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn subscribe_receives_events_via_broadcast() {
        let broadcast = Arc::new(MemoryBroadcastProvider::new());
        let engine = make_engine_with_broadcast(Arc::clone(&broadcast));

        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let received_types = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let types_clone = Arc::clone(&received_types);

        let _unsub = engine
            .subscribe(
                "t1",
                Box::new(move |event| {
                    types_clone.lock().unwrap().push(event.r#type.clone());
                }),
            )
            .await;

        engine
            .transition_task("t1", TaskStatus::Running, None)
            .await
            .unwrap();

        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({ "percent": 75 }),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await
            .unwrap();

        let types = received_types.lock().unwrap();
        assert_eq!(types.len(), 2);
        assert_eq!(types[0], "taskcast:status");
        assert_eq!(types[1], "progress");
    }

    // ─── Concurrency ────────────────────────────────────────────────────

    fn make_shared_engine() -> Arc<TaskEngine> {
        Arc::new(make_engine())
    }

    #[tokio::test]
    async fn concurrent_publish_event_maintains_unique_monotonic_indices() {
        let engine = make_shared_engine();
        let task = engine.create_task(CreateTaskInput::default()).await.unwrap();
        engine
            .transition_task(&task.id, TaskStatus::Running, None)
            .await
            .unwrap();

        let count = 50;
        let mut handles = Vec::new();
        for i in 0..count {
            let engine = Arc::clone(&engine);
            let task_id = task.id.clone();
            handles.push(tokio::spawn(async move {
                engine
                    .publish_event(
                        &task_id,
                        PublishEventInput {
                            r#type: "parallel".to_string(),
                            level: Level::Info,
                            data: serde_json::json!({ "i": i }),
                            series_id: None,
                            series_mode: None,
                        },
                    )
                    .await
                    .unwrap()
            }));
        }

        let events: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let mut indices: Vec<u64> = events.iter().map(|e| e.index).collect();
        indices.sort();

        // All indices must be unique
        assert_eq!(
            std::collections::HashSet::<u64>::from_iter(indices.iter().copied()).len(),
            count,
            "all indices must be unique"
        );
        // Must span exactly `count` consecutive values (transition takes index 0)
        let min = *indices.first().unwrap();
        let max = *indices.last().unwrap();
        assert_eq!(max - min, (count - 1) as u64, "indices must be consecutive");
    }

    #[tokio::test]
    async fn concurrent_status_transitions_final_state_is_consistent() {
        let engine = make_shared_engine();
        let task = engine.create_task(CreateTaskInput::default()).await.unwrap();
        engine
            .transition_task(&task.id, TaskStatus::Running, None)
            .await
            .unwrap();

        // 20 concurrent attempts to complete the same task
        let mut handles = Vec::new();
        for _ in 0..20 {
            let engine = Arc::clone(&engine);
            let task_id = task.id.clone();
            handles.push(tokio::spawn(async move {
                engine
                    .transition_task(&task_id, TaskStatus::Completed, None)
                    .await
            }));
        }

        let results: Vec<_> = futures::future::join_all(handles).await;
        let succeeded = results
            .iter()
            .filter(|r| r.as_ref().map(|r| r.is_ok()).unwrap_or(false))
            .count();

        // At least one must succeed
        assert!(succeeded >= 1, "at least one transition must succeed");

        // Final state must be terminal
        let final_task = engine.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(final_task.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn concurrent_create_task_all_get_unique_ids() {
        let engine = make_shared_engine();
        let count = 100;

        let mut handles = Vec::new();
        for _ in 0..count {
            let engine = Arc::clone(&engine);
            handles.push(tokio::spawn(async move {
                engine.create_task(CreateTaskInput::default()).await.unwrap()
            }));
        }

        let tasks: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let ids: std::collections::HashSet<_> = tasks.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids.len(), count, "all task IDs must be unique");
    }

    #[tokio::test]
    async fn concurrent_subscribers_all_receive_all_events_in_order() {
        let broadcast = Arc::new(MemoryBroadcastProvider::new());
        let engine = Arc::new(make_engine_with_broadcast(Arc::clone(&broadcast)));
        let task = engine.create_task(CreateTaskInput::default()).await.unwrap();
        engine
            .transition_task(&task.id, TaskStatus::Running, None)
            .await
            .unwrap();

        let subscriber_count = 20;
        let event_count = 100;

        // Set up subscribers
        let received: Vec<Arc<std::sync::Mutex<Vec<String>>>> = (0..subscriber_count)
            .map(|_| Arc::new(std::sync::Mutex::new(Vec::new())))
            .collect();

        let mut unsubs = Vec::new();
        for arr in &received {
            let arr = Arc::clone(arr);
            let unsub = broadcast
                .subscribe(
                    &task.id,
                    Box::new(move |event| {
                        if event.r#type != "taskcast:status" {
                            arr.lock().unwrap().push(event.id.clone());
                        }
                    }),
                )
                .await;
            unsubs.push(unsub);
        }

        // Publish events sequentially (engine guarantees ordering)
        let mut published_ids = Vec::new();
        for i in 0..event_count {
            let event = engine
                .publish_event(
                    &task.id,
                    PublishEventInput {
                        r#type: "load.test".to_string(),
                        level: Level::Info,
                        data: serde_json::json!({ "seq": i }),
                        series_id: None,
                        series_mode: None,
                    },
                )
                .await
                .unwrap();
            published_ids.push(event.id);
        }

        // All subscribers should have received all events in correct order
        for (i, arr) in received.iter().enumerate() {
            let ids = arr.lock().unwrap();
            assert_eq!(
                ids.len(),
                event_count,
                "subscriber {i} received {} events, expected {event_count}",
                ids.len()
            );
            assert_eq!(*ids, published_ids, "subscriber {i} received events in wrong order");
        }

        for unsub in unsubs {
            unsub();
        }
    }
}
