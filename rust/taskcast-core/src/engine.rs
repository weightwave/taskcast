use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::series::process_series;
use serde::{Deserialize, Serialize};

use crate::state_machine::{can_transition, is_suspended, is_terminal};
use crate::types::{
    AssignMode, BlockedRequest, BroadcastProvider, CleanupConfig, DisconnectPolicy,
    EventQueryOptions, Level, LongTermStore, ShortTermStore, Task, TaskAuthConfig, TaskFilter,
    TaskcastHooks, TaskError, TaskEvent, TaskStatus, WebhookConfig,
};

// ─── Error ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Task already exists: {0}")]
    TaskAlreadyExists(String),

    #[error("{0}")]
    InvalidInput(String),

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
    pub tags: Option<Vec<String>>,
    pub assign_mode: Option<AssignMode>,
    pub cost: Option<u32>,
    pub disconnect_policy: Option<DisconnectPolicy>,
}

pub struct PublishEventInput {
    pub r#type: String,
    pub level: Level,
    pub data: serde_json::Value,
    pub series_id: Option<String>,
    pub series_mode: Option<crate::types::SeriesMode>,
    pub series_acc_field: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionPayload {
    pub result: Option<HashMap<String, serde_json::Value>>,
    pub error: Option<TaskError>,
    pub reason: Option<String>,
    pub resume_after_ms: Option<f64>,
    pub blocked_request: Option<BlockedRequest>,
    pub ttl: Option<u64>,
}

// ─── TaskEngineOptions ───────────────────────────────────────────────────────

pub struct TaskEngineOptions {
    pub short_term_store: Arc<dyn ShortTermStore>,
    pub broadcast: Arc<dyn BroadcastProvider>,
    pub long_term_store: Option<Arc<dyn LongTermStore>>,
    pub hooks: Option<Arc<dyn TaskcastHooks>>,
}

// ─── TaskEngine ──────────────────────────────────────────────────────────────

/// Callback signature for transition listeners.
/// Receives the task, the old status, and the new status.
pub type TransitionListener = Box<dyn Fn(&Task, &TaskStatus, &TaskStatus) + Send + Sync>;

pub struct TaskEngine {
    short_term_store: Arc<dyn ShortTermStore>,
    broadcast: Arc<dyn BroadcastProvider>,
    long_term_store: Option<Arc<dyn LongTermStore>>,
    hooks: Option<Arc<dyn TaskcastHooks>>,
    transition_listeners: Mutex<Vec<TransitionListener>>,
}

impl TaskEngine {
    pub fn new(opts: TaskEngineOptions) -> Self {
        Self {
            short_term_store: opts.short_term_store,
            broadcast: opts.broadcast,
            long_term_store: opts.long_term_store,
            hooks: opts.hooks,
            transition_listeners: Mutex::new(Vec::new()),
        }
    }

    /// Register a callback that fires whenever a task transitions status.
    /// Also fires when a task is created (with from = to = Pending).
    pub fn add_transition_listener(&self, listener: TransitionListener) {
        self.transition_listeners.lock().unwrap().push(listener);
    }

    pub async fn create_task(&self, input: CreateTaskInput) -> Result<Task, EngineError> {
        if let Some(ttl) = input.ttl {
            if ttl == 0 {
                return Err(EngineError::InvalidInput(
                    "Invalid TTL: 0. TTL must be a positive number.".to_string(),
                ));
            }
        }

        let id = input
            .id
            .clone()
            .unwrap_or_else(|| ulid::Ulid::new().to_string());

        // Check for duplicate user-supplied IDs
        if input.id.is_some() {
            let existing = self.short_term_store.get_task(&id).await?;
            if existing.is_some() {
                return Err(EngineError::TaskAlreadyExists(id));
            }
        }

        let now = now_millis();
        let task = Task {
            id,
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
            tags: input.tags,
            assign_mode: input.assign_mode,
            cost: input.cost,
            assigned_worker: None,
            disconnect_policy: input.disconnect_policy,
            reason: None,
            resume_at: None,
            blocked_request: None,
        };

        self.short_term_store.save_task(task.clone()).await?;

        if let Some(ref long_term_store) = self.long_term_store {
            long_term_store.save_task(task.clone()).await?;
        }

        if let Some(ttl) = task.ttl {
            self.short_term_store.set_ttl(&task.id, ttl).await?;
        }

        if let Some(ref hooks) = self.hooks {
            hooks.on_task_created(&task);
        }

        // Fire transition listeners for task creation (pending → pending)
        {
            let listeners = self.transition_listeners.lock().unwrap();
            for listener in listeners.iter() {
                listener(&task, &TaskStatus::Pending, &TaskStatus::Pending);
            }
        }

        Ok(task)
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Option<Task>, EngineError> {
        let from_short = self.short_term_store.get_task(task_id).await?;
        if from_short.is_some() {
            return Ok(from_short);
        }
        if let Some(ref long_term_store) = self.long_term_store {
            return Ok(long_term_store.get_task(task_id).await?);
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

        let from = task.status.clone();

        if !can_transition(&from, &to) {
            return Err(EngineError::InvalidTransition {
                from,
                to,
            });
        }

        let now = now_millis();
        let new_result = payload
            .as_ref()
            .and_then(|p| p.result.clone())
            .or_else(|| task.result.clone());
        let new_error = payload
            .as_ref()
            .and_then(|p| p.error.clone())
            .or_else(|| task.error.clone());
        let new_completed_at = if is_terminal(&to) {
            Some(now)
        } else {
            task.completed_at
        };

        let mut updated = Task {
            status: to.clone(),
            updated_at: now,
            completed_at: new_completed_at,
            result: new_result,
            error: new_error,
            ..task.clone()
        };

        // ─── Suspended-state field management ────────────────────────────────
        // Set reason when entering suspended state
        if is_suspended(&to) {
            if let Some(ref payload) = payload {
                if payload.reason.is_some() {
                    updated.reason = payload.reason.clone();
                }
            }
        } else {
            // Clear suspended fields when leaving suspended state
            updated.reason = None;
            updated.blocked_request = None;
            updated.resume_at = None;
        }

        // Blocked-specific: set blockedRequest and resumeAt
        if to == TaskStatus::Blocked {
            if let Some(ref payload) = payload {
                if payload.blocked_request.is_some() {
                    updated.blocked_request = payload.blocked_request.clone();
                }
                if let Some(resume_after_ms) = payload.resume_after_ms {
                    updated.resume_at = Some(now + resume_after_ms);
                }
            }
        }

        // ─── TTL manipulation for suspended states ───────────────────────────
        // → paused: stop TTL clock
        if to == TaskStatus::Paused {
            self.short_term_store.clear_ttl(task_id).await?;
        }
        // paused → blocked: restart TTL
        if from == TaskStatus::Paused && to == TaskStatus::Blocked {
            if let Some(ttl) = updated.ttl {
                self.short_term_store.set_ttl(task_id, ttl).await?;
            }
        }
        // paused → running: reset full TTL
        if from == TaskStatus::Paused && to == TaskStatus::Running {
            if let Some(ttl) = updated.ttl {
                self.short_term_store.set_ttl(task_id, ttl).await?;
            }
        }
        // blocked → paused: stop TTL clock
        if from == TaskStatus::Blocked && to == TaskStatus::Paused {
            self.short_term_store.clear_ttl(task_id).await?;
        }

        // TTL override from payload
        if let Some(ref payload) = payload {
            if let Some(ttl) = payload.ttl {
                updated.ttl = Some(ttl);
                if to != TaskStatus::Paused {
                    self.short_term_store.set_ttl(task_id, ttl).await?;
                }
            }
        }

        self.short_term_store.save_task(updated.clone()).await?;

        if let Some(ref long_term_store) = self.long_term_store {
            long_term_store.save_task(updated.clone()).await?;
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
                series_acc_field: None,
            },
        )
        .await?;

        // Emit taskcast:blocked event when entering blocked with blockedRequest
        if to == TaskStatus::Blocked {
            if let Some(ref blocked_request) = updated.blocked_request {
                let mut data = serde_json::Map::new();
                if let Some(ref reason) = updated.reason {
                    data.insert(
                        "reason".to_string(),
                        serde_json::Value::String(reason.clone()),
                    );
                }
                data.insert(
                    "request".to_string(),
                    serde_json::to_value(blocked_request).unwrap(),
                );
                self.emit(
                    task_id,
                    PublishEventInput {
                        r#type: "taskcast:blocked".to_string(),
                        level: Level::Info,
                        data: serde_json::Value::Object(data),
                        series_id: None,
                        series_mode: None,
                        series_acc_field: None,
                    },
                )
                .await?;
            }
        }

        // Emit taskcast:resolved event when going from blocked → running
        if from == TaskStatus::Blocked && to == TaskStatus::Running && task.blocked_request.is_some()
        {
            let resolution = payload.as_ref().and_then(|p| p.result.clone());
            self.emit(
                task_id,
                PublishEventInput {
                    r#type: "taskcast:resolved".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({ "resolution": resolution }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await?;
        }

        if let Some(ref hooks) = self.hooks {
            hooks.on_task_transitioned(&updated, &from, &updated.status);
        }

        // Fire transition listeners
        {
            let listeners = self.transition_listeners.lock().unwrap();
            for listener in listeners.iter() {
                listener(&updated, &from, &updated.status);
            }
        }

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
        Ok(self.short_term_store.get_events(task_id, opts).await?)
    }

    pub async fn list_tasks(&self, filter: TaskFilter) -> Result<Vec<Task>, EngineError> {
        Ok(self.short_term_store.list_tasks(filter).await?)
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
        let index = self.short_term_store.next_index(task_id).await?;
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
            series_acc_field: input.series_acc_field,
        };

        let event = process_series(raw, self.short_term_store.as_ref()).await?;

        self.short_term_store
            .append_event(task_id, event.clone())
            .await?;
        self.broadcast.publish(task_id, event.clone()).await?;

        if let Some(ref long_term_store) = self.long_term_store {
            let long_term_store = Arc::clone(long_term_store);
            let event_clone = event.clone();
            let hooks = self.hooks.clone();
            tokio::spawn(async move {
                if let Err(err) = long_term_store.save_event(event_clone.clone()).await {
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
    use crate::types::{LongTermStore, WorkerAuditEvent};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::RwLock as TokioRwLock;

    // ─── Mock LongTermStore ───────────────────────────────────────────

    struct MockLongTermStore {
        tasks: TokioRwLock<HashMap<String, Task>>,
        events: TokioRwLock<Vec<TaskEvent>>,
        fail_save_event: bool,
    }

    impl MockLongTermStore {
        fn new() -> Self {
            Self {
                tasks: TokioRwLock::new(HashMap::new()),
                events: TokioRwLock::new(Vec::new()),
                fail_save_event: false,
            }
        }

        fn failing_save_event() -> Self {
            Self {
                tasks: TokioRwLock::new(HashMap::new()),
                events: TokioRwLock::new(Vec::new()),
                fail_save_event: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl LongTermStore for MockLongTermStore {
        async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.tasks.write().await.insert(task.id.clone(), task);
            Ok(())
        }

        async fn get_task(&self, task_id: &str) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.tasks.read().await.get(task_id).cloned())
        }

        async fn save_event(&self, event: TaskEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if self.fail_save_event {
                return Err("mock save_event failure".into());
            }
            self.events.write().await.push(event);
            Ok(())
        }

        async fn get_events(&self, _task_id: &str, _opts: Option<EventQueryOptions>) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.events.read().await.clone())
        }

        async fn save_worker_event(&self, _event: WorkerAuditEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        async fn get_worker_events(&self, _worker_id: &str, _opts: Option<EventQueryOptions>) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(Vec::new())
        }
    }

    // ─── Mock Hooks ───────────────────────────────────────────────────

    struct MockHooks {
        dropped_count: AtomicU64,
    }

    impl MockHooks {
        fn new() -> Self {
            Self { dropped_count: AtomicU64::new(0) }
        }
    }

    impl TaskcastHooks for MockHooks {
        fn on_event_dropped(&self, _event: &TaskEvent, _reason: &str) {
            self.dropped_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn make_engine() -> TaskEngine {
        TaskEngine::new(TaskEngineOptions {
            short_term_store: Arc::new(MemoryShortTermStore::new()),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            long_term_store: None,
            hooks: None,
        })
    }

    fn make_engine_with_broadcast(broadcast: Arc<MemoryBroadcastProvider>) -> TaskEngine {
        TaskEngine::new(TaskEngineOptions {
            short_term_store: Arc::new(MemoryShortTermStore::new()),
            broadcast,
            long_term_store: None,
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
                tags: Some(vec!["gpu".to_string()]),
                assign_mode: Some(AssignMode::Pull),
                cost: Some(2),
                disconnect_policy: Some(DisconnectPolicy::Reassign),
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
        assert_eq!(task.tags, Some(vec!["gpu".to_string()]));
        assert_eq!(task.assign_mode, Some(AssignMode::Pull));
        assert_eq!(task.cost, Some(2));
        assert_eq!(task.assigned_worker, None);
        assert_eq!(task.disconnect_policy, Some(DisconnectPolicy::Reassign));
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn create_task_rejects_ttl_zero() {
        let engine = make_engine();
        let result = engine
            .create_task(CreateTaskInput {
                ttl: Some(0),
                ..Default::default()
            })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidInput(_)),
            "Expected InvalidInput error, got: {err}"
        );
        assert!(err.to_string().contains("TTL"));
    }

    #[tokio::test]
    async fn create_task_rejects_duplicate_user_supplied_id() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("dup-id".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = engine
            .create_task(CreateTaskInput {
                id: Some("dup-id".to_string()),
                ..Default::default()
            })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EngineError::TaskAlreadyExists(_)),
            "Expected TaskAlreadyExists error, got: {err}"
        );
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
                    ..Default::default()
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
                    ..Default::default()
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

                    series_acc_field: None,
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

                    series_acc_field: None,
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

                    series_acc_field: None,
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

                    series_acc_field: None,
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

                    series_acc_field: None,
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

                    series_acc_field: None,
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

                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        let events = engine.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 2); // 1 status + 1 progress
        assert_eq!(events[0].r#type, "taskcast:status");
        assert_eq!(events[1].r#type, "progress");
    }

    // ─── list_tasks ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_tasks_returns_all_tasks() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .create_task(CreateTaskInput {
                id: Some("t2".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .create_task(CreateTaskInput {
                id: Some("t3".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let tasks = engine.list_tasks(TaskFilter::default()).await.unwrap();
        assert_eq!(tasks.len(), 3);

        let ids: std::collections::HashSet<String> =
            tasks.iter().map(|t| t.id.clone()).collect();
        assert!(ids.contains("t1"));
        assert!(ids.contains("t2"));
        assert!(ids.contains("t3"));
    }

    #[tokio::test]
    async fn list_tasks_returns_empty_when_no_tasks() {
        let engine = make_engine();
        let tasks = engine.list_tasks(TaskFilter::default()).await.unwrap();
        assert!(tasks.is_empty());
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

                    series_acc_field: None,
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

                            series_acc_field: None,
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

                        series_acc_field: None,
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

    // ─── long_term_store integration ────────────────────────────────────────

    fn make_engine_with_long_term(long_term_store: Arc<dyn LongTermStore>) -> TaskEngine {
        TaskEngine::new(TaskEngineOptions {
            short_term_store: Arc::new(MemoryShortTermStore::new()),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            long_term_store: Some(long_term_store),
            hooks: None,
        })
    }

    #[tokio::test]
    async fn create_task_saves_to_long_term() {
        let long_term_store = Arc::new(MockLongTermStore::new());
        let engine = make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        let task = engine.create_task(CreateTaskInput {
            id: Some("lt-1".to_string()),
            ..Default::default()
        }).await.unwrap();

        let retrieved = long_term_store.get_task(&task.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "lt-1");
    }

    #[tokio::test]
    async fn get_task_falls_back_to_long_term() {
        let long_term_store = Arc::new(MockLongTermStore::new());
        // Save directly to long_term_store, bypassing short_term_store
        let task = Task {
            id: "lt-only".to_string(),
            status: TaskStatus::Completed,
            created_at: 1000.0,
            updated_at: 1000.0,
            r#type: None, params: None, result: None, error: None,
            metadata: None, completed_at: None, ttl: None,
            auth_config: None, webhooks: None, cleanup: None,
            tags: None, assign_mode: None, cost: None,
            assigned_worker: None, disconnect_policy: None,
            reason: None, resume_at: None, blocked_request: None,
        };
        long_term_store.save_task(task).await.unwrap();

        let engine = make_engine_with_long_term(long_term_store);
        let retrieved = engine.get_task("lt-only").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "lt-only");
    }

    #[tokio::test]
    async fn transition_task_saves_to_long_term() {
        let long_term_store = Arc::new(MockLongTermStore::new());
        let engine = make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        engine.create_task(CreateTaskInput {
            id: Some("lt-2".to_string()),
            ..Default::default()
        }).await.unwrap();

        engine.transition_task("lt-2", TaskStatus::Running, None).await.unwrap();

        let retrieved = long_term_store.get_task("lt-2").await.unwrap().unwrap();
        assert_eq!(retrieved.status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn emit_saves_event_to_long_term_async() {
        let long_term_store = Arc::new(MockLongTermStore::new());
        let engine = make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        engine.create_task(CreateTaskInput {
            id: Some("lt-3".to_string()),
            ..Default::default()
        }).await.unwrap();
        engine.transition_task("lt-3", TaskStatus::Running, None).await.unwrap();

        engine.publish_event("lt-3", PublishEventInput {
            r#type: "test".to_string(),
            level: Level::Info,
            data: serde_json::json!(null),
            series_id: None,
            series_mode: None,

            series_acc_field: None,
        }).await.unwrap();

        // The long_term_store save is async (tokio::spawn), give it a moment
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = long_term_store.get_events("lt-3", None).await.unwrap();
        // transition emits a status event + our event = at least 2
        assert!(events.len() >= 2);
    }

    #[tokio::test]
    async fn emit_calls_on_event_dropped_when_long_term_fails() {
        let long_term_store = Arc::new(MockLongTermStore::failing_save_event());
        let hooks = Arc::new(MockHooks::new());

        let engine = TaskEngine::new(TaskEngineOptions {
            short_term_store: Arc::new(MemoryShortTermStore::new()),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            long_term_store: Some(long_term_store),
            hooks: Some(Arc::clone(&hooks) as Arc<dyn TaskcastHooks>),
        });

        engine.create_task(CreateTaskInput {
            id: Some("lt-fail".to_string()),
            ..Default::default()
        }).await.unwrap();
        engine.transition_task("lt-fail", TaskStatus::Running, None).await.unwrap();

        engine.publish_event("lt-fail", PublishEventInput {
            r#type: "test".to_string(),
            level: Level::Info,
            data: serde_json::json!(null),
            series_id: None,
            series_mode: None,

            series_acc_field: None,
        }).await.unwrap();

        // Give async spawn time to execute
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(hooks.dropped_count.load(Ordering::SeqCst) >= 1);
    }
}
