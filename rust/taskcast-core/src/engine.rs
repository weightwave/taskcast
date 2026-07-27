use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex as TokioMutex;

use crate::archive::{
    build_task_archive_restore_data, sanitize_task_archive_event, validate_task_archive,
    ArchiveError, TASK_ARCHIVE_SCHEMA, TASK_ARCHIVE_VERSION,
};
use crate::canonical_history::{
    apply_canonical_history_query, merge_canonical_history, resolve_canonical_series_latest,
};
use crate::series::process_series;
use crate::storage_coordinator::StorageCoordinator;
use crate::ttl_coordinator::{DurableTtlSweepResult, TtlCoordinator};
use serde::{Deserialize, Serialize};

use crate::state_machine::{can_transition, is_suspended, is_terminal};
use crate::types::{
    AssignMode, BlockedRequest, BroadcastProvider, CleanupConfig, DisconnectPolicy,
    DurableSeriesState, EventQueryOptions, HotWriteToken, Level, LongTermStore,
    ReleasePreconditions, ReleaseResult, SeriesMode, ShortTermStore, SinceCursor,
    StorageFenceConflictError, StorageIntegrityError, StoragePreconditionError,
    StorageReleaseRequest, StorageReleaseUnsupportedError, StorageWriterRegistration, Task,
    TaskArchive, TaskArchiveImportOptions, TaskArchiveImportResult, TaskAuthConfig, TaskError,
    TaskEvent, TaskFilter, TaskStatus, TaskcastHooks, WebhookConfig,
};

// ─── Error ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Task already exists: {0}")]
    TaskConflict(String),

    #[error("{0}")]
    InvalidInput(String),

    #[error("Invalid transition: {from:?} \u{2192} {to:?}")]
    InvalidTransition { from: TaskStatus, to: TaskStatus },

    #[error("Cannot publish to task in terminal status: {0:?}")]
    TaskTerminal(TaskStatus),

    #[error("{0}")]
    Archive(#[from] ArchiveError),

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

/// Callback signature for creation listeners.
/// Receives the newly created task.
pub type CreationListener = Arc<dyn Fn(&Task) + Send + Sync>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StorageReleaseSweepResult {
    pub claimed: u64,
    pub released: u64,
    pub recovered: u64,
    pub stale: u64,
    pub deferred: u64,
    pub failed: u64,
}

pub struct TaskEngine {
    short_term_store: Arc<dyn ShortTermStore>,
    broadcast: Arc<dyn BroadcastProvider>,
    long_term_store: Option<Arc<dyn LongTermStore>>,
    storage_coordinator: Option<StorageCoordinator>,
    ttl_coordinator: Option<TtlCoordinator>,
    hooks: Option<Arc<dyn TaskcastHooks>>,
    transition_listeners: Arc<Mutex<Vec<TransitionListener>>>,
    creation_listeners: Mutex<Vec<CreationListener>>,
    /// Per-task mutex to serialize `emit` calls, ensuring events are stored
    /// in the same order as their atomically-assigned indices.
    emit_locks: Arc<Mutex<HashMap<String, Arc<TokioMutex<()>>>>>,
}

impl TaskEngine {
    const CREATION_CLAIM_TTL_MS: u64 = 30_000;

    pub fn new(opts: TaskEngineOptions) -> Self {
        Self::new_with_storage_lifecycle(opts, 30_000, 1_000)
    }

    pub fn new_with_storage_lifecycle(
        opts: TaskEngineOptions,
        storage_lock_ttl_ms: u64,
        rehydrate_replay_events: u64,
    ) -> Self {
        assert!(storage_lock_ttl_ms > 0, "storage lock TTL must be positive");
        assert!(
            rehydrate_replay_events > 0,
            "rehydrate replay event count must be positive"
        );
        let transition_listeners: Arc<Mutex<Vec<TransitionListener>>> =
            Arc::new(Mutex::new(Vec::new()));
        let emit_locks: Arc<Mutex<HashMap<String, Arc<TokioMutex<()>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let storage_coordinator = opts.long_term_store.as_ref().and_then(|long_term_store| {
            if opts.short_term_store.supports_hot_cold_release()
                && long_term_store.supports_hot_cold_release()
            {
                Some(
                    StorageCoordinator::new(
                        Arc::clone(&opts.short_term_store),
                        Arc::clone(long_term_store),
                    )
                    .with_storage_lock_ttl_ms(storage_lock_ttl_ms)
                    .with_rehydrate_replay_events(rehydrate_replay_events),
                )
            } else {
                None
            }
        });
        let ttl_coordinator = opts.long_term_store.as_ref().and_then(|long_term_store| {
            if opts.short_term_store.supports_hot_cold_release()
                && long_term_store.supports_durable_ttl()
            {
                let hooks = opts.hooks.clone();
                let ttl_transition_listeners = Arc::clone(&transition_listeners);
                let ttl_emit_locks = Arc::clone(&emit_locks);
                TtlCoordinator::new(
                    Arc::clone(&opts.short_term_store),
                    Arc::clone(long_term_store),
                    Arc::clone(&opts.broadcast),
                )
                .map(|coordinator| {
                    coordinator
                        .with_storage_lock_ttl_ms(storage_lock_ttl_ms)
                        .with_on_timeout_projected(Arc::new(move |task, from| {
                            ttl_emit_locks.lock().unwrap().remove(&task.id);
                            if let Some(hooks) = &hooks {
                                hooks.on_task_timeout(task);
                                hooks.on_task_transitioned(task, from, &task.status);
                            }
                            let listeners = ttl_transition_listeners.lock().unwrap();
                            for listener in listeners.iter() {
                                listener(task, from, &task.status);
                            }
                        }))
                })
                .ok()
            } else {
                None
            }
        });
        Self {
            short_term_store: opts.short_term_store,
            broadcast: opts.broadcast,
            long_term_store: opts.long_term_store,
            storage_coordinator,
            ttl_coordinator,
            hooks: opts.hooks,
            transition_listeners,
            creation_listeners: Mutex::new(Vec::new()),
            emit_locks,
        }
    }

    /// Register a callback that fires whenever a task transitions status.
    /// Also fires when a task is created (with from = to = Pending).
    pub fn add_transition_listener(&self, listener: TransitionListener) {
        self.transition_listeners.lock().unwrap().push(listener);
    }

    /// Register a callback that fires whenever a new task is created.
    /// Returns the listener Arc so it can be passed to `remove_creation_listener`.
    pub fn add_creation_listener(&self, listener: CreationListener) {
        self.creation_listeners.lock().unwrap().push(listener);
    }

    /// Remove a previously registered creation listener by Arc identity.
    pub fn remove_creation_listener(&self, listener: &CreationListener) {
        let mut listeners = self.creation_listeners.lock().unwrap();
        listeners.retain(|l| !Arc::ptr_eq(l, listener));
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
        let can_fence_creation = self
            .long_term_store
            .as_ref()
            .is_some_and(|store| store.supports_task_creation_claims());

        // Explicit IDs are durable identities, including while hot state is cold.
        if input.id.is_some() && !can_fence_creation && self.get_task(&id).await?.is_some() {
            return Err(EngineError::TaskConflict(id));
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

        let mut durable_identity_claimed = false;
        let mut creation_token = None;
        if input.id.is_some() {
            if let Some(ref long_term_store) = self.long_term_store {
                if long_term_store.supports_task_creation_claims() {
                    let token = ulid::Ulid::new().to_string();
                    durable_identity_claimed = long_term_store
                        .claim_task_creation(task.clone(), &token, Self::CREATION_CLAIM_TTL_MS)
                        .await?;
                    creation_token = Some(token);
                } else if long_term_store.supports_hot_cold_release() {
                    return Err(EngineError::Store(Box::new(
                        StorageReleaseUnsupportedError::new(
                            "Hot/cold long-term stores must support token-fenced task creation",
                        ),
                    )));
                } else {
                    durable_identity_claimed =
                        long_term_store.create_task_if_absent(task.clone()).await?;
                }
                if !durable_identity_claimed {
                    return Err(EngineError::TaskConflict(task.id.clone()));
                }
            }
        }
        if let Err(error) = self.short_term_store.save_task(task.clone()).await {
            if let (Some(long_term_store), Some(token)) =
                (self.long_term_store.as_ref(), creation_token.as_ref())
            {
                long_term_store.abort_task_creation(&task.id, token).await?;
            }
            return Err(EngineError::Store(error));
        }

        if let (Some(long_term_store), Some(token)) =
            (self.long_term_store.as_ref(), creation_token.as_ref())
        {
            let mut completed = false;
            let mut completion_error = None;
            for _ in 0..3 {
                match long_term_store
                    .complete_task_creation(&task.id, token)
                    .await
                {
                    Ok(value) => {
                        completed = value;
                        completion_error = None;
                        if completed {
                            break;
                        }
                    }
                    Err(error) => completion_error = Some(error),
                }
            }
            if let Some(error) = completion_error {
                return Err(EngineError::Store(error));
            }
            if !completed {
                return Err(EngineError::Store(Box::new(
                    StorageReleaseUnsupportedError::new(format!(
                        "Durable creation claim was lost for task {}",
                        task.id
                    )),
                )));
            }
        } else if !durable_identity_claimed {
            if let Some(ref long_term_store) = self.long_term_store {
                long_term_store.save_task(task.clone()).await?;
            }
        }

        if let Some(ttl) = task.ttl {
            if self.ttl_coordinator.is_some() {
                self.short_term_store.clear_ttl(&task.id).await?;
            } else {
                self.short_term_store.set_ttl(&task.id, ttl).await?;
            }
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

        // Fire creation listeners
        // Snapshot the Arc list, drop the lock, then invoke to prevent deadlock
        // if a listener calls add_creation_listener / remove_creation_listener.
        {
            let listeners: Vec<CreationListener> = self.creation_listeners.lock().unwrap().clone();
            for listener in &listeners {
                listener(&task);
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
        let (task, expected_revision, initial_write_token) =
            if let Some(coordinator) = &self.storage_coordinator {
                let token = coordinator.ensure_task_hot_for_write(task_id).await?;
                let snapshot = self
                    .short_term_store
                    .get_task_mutation_snapshot(task_id)
                    .await?
                    .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?;
                (snapshot.task, Some(snapshot.revision), Some(token))
            } else {
                (
                    self.get_task(task_id)
                        .await?
                        .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?,
                    None,
                    None,
                )
            };

        let from = task.status.clone();

        if !can_transition(&from, &to) {
            return Err(EngineError::InvalidTransition { from, to });
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

        // TTL override from payload
        if let Some(ref payload) = payload {
            if let Some(ttl) = payload.ttl {
                updated.ttl = Some(ttl);
            }
        }

        // PostgreSQL owns durable execution deadlines. Redis expiration remains
        // available only for stores without durable TTL support.
        if self.ttl_coordinator.is_some() {
            if updated.ttl.is_some() {
                self.short_term_store.clear_ttl(task_id).await?;
            }
        } else {
            // → paused: stop TTL clock
            if to == TaskStatus::Paused {
                self.short_term_store.clear_ttl(task_id).await?;
            }
            // → blocked/running from paused: restart the full TTL
            if from == TaskStatus::Paused
                && (to == TaskStatus::Blocked || to == TaskStatus::Running)
            {
                if let Some(ttl) = updated.ttl {
                    self.short_term_store.set_ttl(task_id, ttl).await?;
                }
            }
            // TTL override restarts the clock outside paused state
            if let Some(ttl) = payload.as_ref().and_then(|payload| payload.ttl) {
                if to != TaskStatus::Paused {
                    self.short_term_store.set_ttl(task_id, ttl).await?;
                }
            }
        }

        let mut status_data = serde_json::Map::new();
        status_data.insert("status".to_string(), serde_json::json!(to));
        if let Some(ref result) = updated.result {
            status_data.insert("result".to_string(), serde_json::json!(result));
        }
        if let Some(ref error) = updated.error {
            status_data.insert("error".to_string(), serde_json::json!(error));
        }
        let mut derived_events = vec![PublishEventInput {
            r#type: "taskcast:status".to_string(),
            level: Level::Info,
            data: serde_json::Value::Object(status_data),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
        }];

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
                derived_events.push(PublishEventInput {
                    r#type: "taskcast:blocked".to_string(),
                    level: Level::Info,
                    data: serde_json::Value::Object(data),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                });
            }
        }

        // Emit taskcast:resolved event when going from blocked → running
        if from == TaskStatus::Blocked
            && to == TaskStatus::Running
            && task.blocked_request.is_some()
        {
            let resolution = payload.as_ref().and_then(|p| p.result.clone());
            derived_events.push(PublishEventInput {
                r#type: "taskcast:resolved".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "resolution": resolution }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            });
        }

        if self.storage_coordinator.is_some() {
            let committed = self
                .commit_task_events_for_mutation(
                    updated.clone(),
                    expected_revision.as_deref().unwrap_or_default(),
                    &from,
                    derived_events,
                    initial_write_token.expect("storage coordinator write token"),
                )
                .await?;
            if let Some(ref long_term_store) = self.long_term_store {
                long_term_store.save_task(updated.clone()).await?;
            }
            for event in committed {
                self.finish_committed_event(event, None).await?;
            }
        } else {
            self.short_term_store.save_task(updated.clone()).await?;
            if let Some(ref long_term_store) = self.long_term_store {
                long_term_store.save_task(updated.clone()).await?;
            }
            for event in derived_events {
                self.emit(task_id, event, true).await?;
            }
        }

        // Clean up per-task emit lock — no more events can be published
        // to a terminal task (publish_event rejects), so the lock is unused.
        // A reopened task will lazily recreate the entry on next emit.
        if is_terminal(&to) {
            let mut locks = self.emit_locks.lock().unwrap();
            locks.remove(task_id);
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

        self.emit(task_id, input, false).await
    }

    pub async fn release_task_storage(
        &self,
        task_id: &str,
        preconditions: ReleasePreconditions,
    ) -> Result<ReleaseResult, EngineError> {
        let coordinator = self.storage_coordinator.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::default()))
        })?;
        if preconditions.expected_last_event_index < -1
            || !preconditions.inactive_since.is_finite()
            || preconditions.inactive_since < 0.0
            || preconditions.inactive_since.fract() != 0.0
            || preconditions.inactive_since > i64::MAX as f64
        {
            return Err(EngineError::Store(Box::new(StoragePreconditionError::new(
                "Storage release preconditions are invalid",
            ))));
        }
        let durable = self.long_term_store.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::default()))
        })?;
        let request = StorageReleaseRequest {
            task_id: task_id.to_string(),
            requested_at: now_millis(),
            expected_last_event_index: preconditions.expected_last_event_index,
            inactive_since: preconditions.inactive_since,
        };
        if !durable
            .persist_storage_release_request(request.clone())
            .await?
        {
            return Err(EngineError::TaskNotFound(task_id.to_string()));
        }
        match coordinator
            .release_task_storage(task_id, preconditions)
            .await
        {
            Ok(result) => {
                durable.clear_storage_release_request(&request).await?;
                Ok(result)
            }
            Err(error) => {
                if error.downcast_ref::<StoragePreconditionError>().is_some() {
                    durable.clear_storage_release_request(&request).await?;
                }
                Err(EngineError::Store(error))
            }
        }
    }

    pub async fn release_task_storage_at_current_durable_index(
        &self,
        task_id: &str,
        inactive_since: f64,
    ) -> Result<ReleaseResult, EngineError> {
        let durable = self.long_term_store.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::new(
                "Long-term store cannot read the durable event watermark",
            )))
        })?;
        let expected_last_event_index = durable.get_last_event_index(task_id).await?;
        self.release_task_storage(
            task_id,
            ReleasePreconditions {
                expected_last_event_index,
                inactive_since,
            },
        )
        .await
    }

    pub async fn retry_storage_release_requests(
        &self,
        limit: u64,
        inactive_before: f64,
    ) -> Result<StorageReleaseSweepResult, EngineError> {
        if limit == 0
            || !inactive_before.is_finite()
            || inactive_before < 0.0
            || inactive_before.fract() != 0.0
        {
            return Err(EngineError::Store(Box::new(StoragePreconditionError::new(
                "Storage release sweep bounds are invalid",
            ))));
        }
        let coordinator = self.storage_coordinator.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::new(
                "Storage release is not supported by the configured stores",
            )))
        })?;
        let durable = self.long_term_store.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::new(
                "Long-term store cannot list storage release requests",
            )))
        })?;
        let requests = durable.list_storage_release_requests(limit).await?;
        let mut result = StorageReleaseSweepResult {
            claimed: requests.len() as u64,
            ..Default::default()
        };
        for request in requests {
            if request.inactive_since > inactive_before {
                result.deferred += 1;
                continue;
            }
            let outcome = async {
                let recovery = coordinator.recover_task_storage(&request.task_id).await?;
                if recovery.storage_state == crate::types::StorageState::Cold {
                    durable.clear_storage_release_request(&request).await?;
                    return Ok::<_, Box<dyn std::error::Error + Send + Sync>>("recovered");
                }
                coordinator
                    .release_task_storage(
                        &request.task_id,
                        ReleasePreconditions {
                            expected_last_event_index: request.expected_last_event_index,
                            inactive_since: request.inactive_since,
                        },
                    )
                    .await?;
                durable.clear_storage_release_request(&request).await?;
                Ok("released")
            }
            .await;
            match outcome {
                Ok("recovered") => result.recovered += 1,
                Ok(_) => result.released += 1,
                Err(error) if error.downcast_ref::<StoragePreconditionError>().is_some() => {
                    durable.clear_storage_release_request(&request).await?;
                    result.stale += 1;
                }
                Err(_) => result.failed += 1,
            }
        }
        Ok(result)
    }

    pub async fn register_storage_writer(
        &self,
        registration: StorageWriterRegistration,
        ttl_ms: u64,
    ) -> Result<(), EngineError> {
        if !self.short_term_store.supports_hot_cold_release() {
            return Err(EngineError::Store(Box::new(
                StorageReleaseUnsupportedError::new(
                    "Short-term store cannot register storage writers",
                ),
            )));
        }
        self.short_term_store
            .register_storage_writer(registration, ttl_ms)
            .await?;
        Ok(())
    }

    pub async fn list_storage_writers(
        &self,
    ) -> Result<Vec<StorageWriterRegistration>, EngineError> {
        if !self.short_term_store.supports_hot_cold_release() {
            return Err(EngineError::Store(Box::new(
                StorageReleaseUnsupportedError::new("Short-term store cannot list storage writers"),
            )));
        }
        Ok(self.short_term_store.list_storage_writers().await?)
    }

    pub fn supports_storage_release(&self) -> bool {
        self.storage_coordinator.is_some()
    }

    pub fn supports_durable_ttl(&self) -> bool {
        self.ttl_coordinator.is_some()
    }

    pub async fn sweep_durable_ttl(
        &self,
        limit: u64,
        claim_ttl_ms: Option<u64>,
    ) -> Result<DurableTtlSweepResult, EngineError> {
        let coordinator = self.ttl_coordinator.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::new(
                "Durable execution TTL is not supported by the configured stores",
            )))
        })?;
        Ok(coordinator.sweep_overdue(limit, claim_ttl_ms).await?)
    }

    pub async fn sweep_terminal_projections(
        &self,
        limit: u64,
        claim_ttl_ms: Option<u64>,
    ) -> Result<DurableTtlSweepResult, EngineError> {
        let coordinator = self.ttl_coordinator.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::new(
                "Durable terminal projection is not supported by the configured stores",
            )))
        })?;
        Ok(coordinator
            .sweep_terminal_projections(limit, claim_ttl_ms)
            .await?)
    }

    pub async fn recover_task_storage(&self, task_id: &str) -> Result<ReleaseResult, EngineError> {
        let coordinator = self.storage_coordinator.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::default()))
        })?;
        Ok(coordinator.recover_task_storage(task_id).await?)
    }

    pub async fn export_task_archive(&self, task_id: &str) -> Result<TaskArchive, EngineError> {
        let task = self
            .get_task(task_id)
            .await?
            .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?;

        self.build_export_archive(&task).await
    }

    pub async fn import_task_archive(
        &self,
        archive: TaskArchive,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<TaskArchiveImportResult, EngineError> {
        let import_options = options.unwrap_or_default();
        let normalized = validate_task_archive(&archive)?;
        let task_id = normalized.task.id.clone();
        let existing = self.get_task(&task_id).await?;

        if existing.is_some() && !import_options.overwrite {
            return Err(EngineError::TaskConflict(task_id));
        }

        if !self.short_term_store.supports_task_archive_restore() {
            return Err(unsupported_archive_restore(
                "shortTermStore does not support restore_task_archive",
            ));
        }
        let long_term_shares_archive_restore_storage = self
            .long_term_store
            .as_ref()
            .map(|store| store.shares_task_archive_restore_storage())
            .unwrap_or(false);

        if let Some(ref long_term_store) = self.long_term_store {
            if !long_term_shares_archive_restore_storage
                && !long_term_store.supports_task_archive_restore()
            {
                return Err(unsupported_archive_restore(
                    "longTermStore does not support restore_task_archive",
                ));
            }
        }

        let event_count = normalized.events.len();
        let restore_data = build_task_archive_restore_data(&normalized)?;
        self.short_term_store
            .validate_task_archive_restore(&restore_data, Some(import_options))
            .await?;
        if let Some(ref long_term_store) = self.long_term_store {
            long_term_store
                .validate_task_archive_restore(&restore_data, Some(import_options))
                .await?;
        }

        let restore_options = Some(import_options);
        // Durable history is restored before the live short-term cache so a final
        // long-term failure cannot expose an imported task that was never persisted.
        if let Some(ref long_term_store) = self.long_term_store {
            if !long_term_shares_archive_restore_storage {
                long_term_store
                    .restore_task_archive(restore_data.clone(), restore_options)
                    .await?;
            }
        }
        self.short_term_store
            .restore_task_archive(restore_data, restore_options)
            .await?;

        self.emit_locks.lock().unwrap().remove(&task_id);

        Ok(TaskArchiveImportResult {
            task_id,
            event_count,
            overwritten: existing.is_some(),
        })
    }

    pub async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, EngineError> {
        let Some(ref long_term_store) = self.long_term_store else {
            return Ok(self.short_term_store.get_events(task_id, opts).await?);
        };
        if !long_term_store.supports_hot_cold_release() {
            let from_short = self
                .short_term_store
                .get_events(task_id, opts.clone())
                .await?;
            return if from_short.is_empty() {
                Ok(long_term_store.get_events(task_id, opts).await?)
            } else {
                Ok(from_short)
            };
        }
        let overlay_hot = self.should_overlay_hot_history(task_id).await?;
        let hot_events = if overlay_hot {
            self.short_term_store.get_events(task_id, None).await?
        } else {
            Vec::new()
        };
        let durable_series = if long_term_store.supports_hot_cold_release() {
            long_term_store.get_durable_series_state(task_id).await?
        } else {
            Vec::new()
        };
        let durable_events = self
            .load_canonical_durable_events(task_id, opts.as_ref(), &hot_events, &durable_series)
            .await?;
        let merged = merge_canonical_history(&durable_events, &hot_events, &durable_series)
            .map_err(|error| EngineError::Store(Box::new(error)))?;
        Ok(apply_canonical_history_query(&merged, opts))
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

    /// Synchronous version of `subscribe` for use in contexts where async
    /// is not available (e.g., inside creation listener callbacks).
    ///
    /// Not all broadcast providers support this — see `BroadcastProvider::subscribe_sync`.
    pub fn subscribe_sync(
        &self,
        task_id: &str,
        handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Result<Box<dyn Fn() + Send + Sync>, Box<dyn std::error::Error + Send + Sync>> {
        self.broadcast.subscribe_sync(task_id, handler)
    }

    /// Get the latest accumulated event for a series.
    pub async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, EngineError> {
        let Some(ref long_term_store) = self.long_term_store else {
            return Ok(self
                .short_term_store
                .get_series_latest(task_id, series_id)
                .await?);
        };
        if !long_term_store.supports_hot_cold_release() {
            return Ok(self
                .short_term_store
                .get_series_latest(task_id, series_id)
                .await?);
        }
        let durable = long_term_store
            .get_durable_series_state(task_id)
            .await?
            .into_iter()
            .find(|state| state.series_id == series_id);
        let Some(durable) = durable else {
            return Ok(self
                .short_term_store
                .get_series_latest(task_id, series_id)
                .await?);
        };
        if !self.should_overlay_hot_history(task_id).await? {
            return Ok(Some(durable.event));
        }
        let hot_events = self.short_term_store.get_events(task_id, None).await?;
        Ok(Some(
            resolve_canonical_series_latest(&durable, &hot_events)
                .map_err(|error| EngineError::Store(Box::new(error)))?,
        ))
    }

    // ─── Private ─────────────────────────────────────────────────────────

    async fn should_overlay_hot_history(&self, task_id: &str) -> Result<bool, EngineError> {
        let Some(ref long_term_store) = self.long_term_store else {
            return Ok(true);
        };
        if long_term_store.supports_hot_cold_release() {
            let metadata = long_term_store.get_task_storage_metadata(task_id).await?;
            if metadata
                .is_some_and(|metadata| metadata.storage_state == crate::types::StorageState::Cold)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn load_canonical_durable_events(
        &self,
        task_id: &str,
        opts: Option<&EventQueryOptions>,
        hot_events: &[TaskEvent],
        durable_series: &[DurableSeriesState],
    ) -> Result<Vec<TaskEvent>, EngineError> {
        let long_term_store = self
            .long_term_store
            .as_ref()
            .expect("canonical history requires long-term storage");
        let Some(requested_limit) = opts.and_then(|opts| opts.limit) else {
            return Ok(long_term_store
                .get_events(
                    task_id,
                    canonical_durable_query(opts, hot_events, durable_series),
                )
                .await?);
        };
        if requested_limit == 0 {
            return Ok(Vec::new());
        }

        let page_size = requested_limit.min(1_000);
        let mut paged_opts = opts.cloned().unwrap_or(EventQueryOptions {
            since: None,
            limit: None,
        });
        paged_opts.limit = Some(page_size);
        let mut query = canonical_durable_query(Some(&paged_opts), hot_events, durable_series);
        let mut loaded = Vec::new();
        let mut previous_boundary = None;
        loop {
            let page = long_term_store.get_events(task_id, query).await?;
            loaded.extend(page.iter().cloned());
            let merged = merge_canonical_history(&loaded, hot_events, durable_series)
                .map_err(|error| EngineError::Store(Box::new(error)))?;
            let assembled = apply_canonical_history_query(&merged, opts.cloned());
            if page.len() < page_size as usize {
                return Ok(loaded);
            }

            let boundary = page.last().expect("full durable history page").index;
            if previous_boundary.is_some_and(|previous| boundary <= previous) {
                return Err(EngineError::Store(Box::new(StorageIntegrityError::new(
                    "Durable history pagination did not advance",
                ))));
            }
            if assembled.len() >= requested_limit as usize
                && assembled[requested_limit as usize - 1].index <= boundary
            {
                return Ok(loaded);
            }
            previous_boundary = Some(boundary);
            query = Some(EventQueryOptions {
                since: Some(SinceCursor {
                    id: None,
                    index: Some(boundary),
                    timestamp: None,
                }),
                limit: Some(page_size),
            });
        }
    }

    async fn build_export_archive(&self, task: &Task) -> Result<TaskArchive, EngineError> {
        let short_term_events = self.short_term_store.get_events(&task.id, None).await?;
        if let Some(ref long_term_store) = self.long_term_store {
            let long_term_events = long_term_store.get_events(&task.id, None).await?;
            if !long_term_events.is_empty() {
                let merged = self.merge_export_histories(&long_term_events, &short_term_events)?;
                return self.normalize_export_archive(task, merged).await;
            }
        }

        self.normalize_export_archive(task, short_term_events).await
    }

    fn merge_export_histories(
        &self,
        long_term_events: &[TaskEvent],
        short_term_events: &[TaskEvent],
    ) -> Result<Vec<TaskEvent>, EngineError> {
        let short_term_by_index: HashMap<u64, TaskEvent> = short_term_events
            .iter()
            .cloned()
            .map(|event| (event.index, event))
            .collect();

        let long_term_indexes: HashSet<u64> =
            long_term_events.iter().map(|event| event.index).collect();
        if let Some(max_index) = long_term_indexes.iter().copied().max() {
            for index in 0..=max_index {
                if long_term_indexes.contains(&index) {
                    continue;
                }
                if let Some(short_term_event) = short_term_by_index.get(&index) {
                    if !is_compactable_series_event(short_term_event) {
                        return Err(ArchiveError::Invalid(format!(
                            "Cannot export sparse long-term history; missing durable non-series event at index {index}",
                        ))
                        .into());
                    }
                }
            }
        }

        let mut merged = long_term_events.to_vec();
        let mut merged_keys: HashSet<(String, u64)> = long_term_events
            .iter()
            .map(|event| (event.id.clone(), event.index))
            .collect();
        let prefix_len = contiguous_prefix_len(long_term_events);
        for event in short_term_events {
            let key = (event.id.clone(), event.index);
            if merged_keys.contains(&key) {
                continue;
            }
            if event.index >= prefix_len || is_compactable_series_event(event) {
                merged.push(event.clone());
                merged_keys.insert(key);
            }
        }

        Ok(merged)
    }

    async fn normalize_export_archive(
        &self,
        task: &Task,
        events: Vec<TaskEvent>,
    ) -> Result<TaskArchive, EngineError> {
        let compacted_events = self.compact_export_events(&task.id, events).await?;
        let archive = TaskArchive {
            schema: TASK_ARCHIVE_SCHEMA.to_string(),
            version: TASK_ARCHIVE_VERSION,
            exported_at: now_millis(),
            task: task.clone(),
            events: compacted_events,
        };

        Ok(validate_task_archive(&archive)?)
    }

    async fn compact_export_events(
        &self,
        task_id: &str,
        events: Vec<TaskEvent>,
    ) -> Result<Vec<TaskEvent>, EngineError> {
        #[derive(Clone)]
        struct ExportEntry {
            event: TaskEvent,
            first_index: u64,
            last_index: u64,
            order: usize,
        }

        let mut entries = Vec::<ExportEntry>::new();
        let mut series_entries = HashMap::<String, usize>::new();
        let mut sorted = events;
        sorted.sort_by_key(|event| event.index);

        for event in sorted {
            if !is_compactable_series_event(&event) {
                entries.push(ExportEntry {
                    first_index: event.index,
                    last_index: event.index,
                    order: entries.len(),
                    event,
                });
                continue;
            }

            let key = format!(
                "{}:{}",
                event.task_id,
                event.series_id.as_deref().unwrap_or_default()
            );
            if let Some(existing_index) = series_entries.get(&key).copied() {
                let existing = &mut entries[existing_index];
                if event.index >= existing.last_index {
                    existing.last_index = event.index;
                    existing.event = event;
                }
            } else {
                let entry_index = entries.len();
                series_entries.insert(key, entry_index);
                entries.push(ExportEntry {
                    first_index: event.index,
                    last_index: event.index,
                    order: entry_index,
                    event,
                });
            }
        }

        for entry_index in series_entries.values().copied() {
            let series_id = entries[entry_index].event.series_id.clone();
            if let Some(series_id) = series_id {
                let latest = self
                    .short_term_store
                    .get_series_latest(task_id, &series_id)
                    .await?;
                if let Some(latest) = latest {
                    if latest.index >= entries[entry_index].last_index {
                        entries[entry_index].last_index = latest.index;
                        entries[entry_index].event = latest;
                    }
                }
            }
        }

        entries.sort_by_key(|entry| (entry.first_index, entry.order));
        Ok(entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| {
                let mut event = sanitize_task_archive_event(entry.event);
                event.index = index as u64;
                event
            })
            .collect())
    }

    async fn emit(
        &self,
        task_id: &str,
        input: PublishEventInput,
        allow_terminal: bool,
    ) -> Result<TaskEvent, EngineError> {
        // Acquire per-task lock to serialize event storage + broadcast,
        // preventing race conditions where concurrent publishes could
        // store events in a different order than their assigned indices.
        let emit_lock = {
            let mut locks = self.emit_locks.lock().unwrap();
            locks
                .entry(task_id.to_string())
                .or_insert_with(|| Arc::new(TokioMutex::new(())))
                .clone()
        };
        let _guard = emit_lock.lock().await;
        if !allow_terminal {
            let current = self
                .get_task(task_id)
                .await?
                .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?;
            if is_terminal(&current.status) {
                return Err(EngineError::TaskTerminal(current.status));
            }
        }

        if let Some(coordinator) = &self.storage_coordinator {
            let raw = TaskEvent {
                id: ulid::Ulid::new().to_string(),
                task_id: task_id.to_string(),
                index: 0,
                timestamp: now_millis(),
                r#type: input.r#type,
                level: input.level,
                data: input.data,
                series_id: input.series_id,
                series_mode: input.series_mode,
                series_acc_field: input.series_acc_field,
                series_snapshot: None,
                _accumulated_data: None,
            };
            let mut initial_storage_epoch = None;
            for attempt in 0..3 {
                let token = if attempt == 0 {
                    coordinator.ensure_task_hot_for_write(task_id).await?
                } else {
                    coordinator
                        .ensure_task_hot_for_write_without_rehydrate(task_id)
                        .await?
                };
                match initial_storage_epoch {
                    Some(epoch) if token.storage_epoch != epoch => {
                        return Err(EngineError::Store(Box::new(
                            StorageFenceConflictError::new(
                                "Task storage epoch changed after the write mutation started",
                            ),
                        )));
                    }
                    None => initial_storage_epoch = Some(token.storage_epoch),
                    Some(_) => {}
                }
                match self
                    .short_term_store
                    .commit_event_fenced(task_id, raw.clone(), &token)
                    .await
                {
                    Ok(series_result) => {
                        let event = series_result.event;
                        self.finish_committed_event(event.clone(), series_result.accumulated_event)
                            .await?;
                        return Ok(event);
                    }
                    Err(error)
                        if error.downcast_ref::<StorageFenceConflictError>().is_some()
                            && attempt < 2 =>
                    {
                        continue;
                    }
                    Err(error) => return Err(EngineError::Store(error)),
                }
            }
            return Err(EngineError::Store(Box::new(
                StorageFenceConflictError::default(),
            )));
        }

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
            series_snapshot: None,
            _accumulated_data: None,
        };

        let series_result = process_series(raw, self.short_term_store.as_ref()).await?;
        let event = series_result.event;

        // Store delta event in short-term store (skip if process_series already stored it)
        if !series_result.stored {
            self.short_term_store
                .append_event(task_id, event.clone())
                .await?;
        }

        self.finish_committed_event(event.clone(), series_result.accumulated_event)
            .await?;

        Ok(event)
    }

    async fn commit_task_events_for_mutation(
        &self,
        task: Task,
        expected_revision: &str,
        expected_status: &TaskStatus,
        inputs: Vec<PublishEventInput>,
        initial_token: HotWriteToken,
    ) -> Result<Vec<TaskEvent>, EngineError> {
        let coordinator = self.storage_coordinator.as_ref().ok_or_else(|| {
            EngineError::Store(Box::new(StorageReleaseUnsupportedError::default()))
        })?;
        let emit_lock = {
            let mut locks = self.emit_locks.lock().unwrap();
            locks
                .entry(task.id.clone())
                .or_insert_with(|| Arc::new(TokioMutex::new(())))
                .clone()
        };
        let _guard = emit_lock.lock().await;
        let events = inputs
            .into_iter()
            .map(|input| TaskEvent {
                id: ulid::Ulid::new().to_string(),
                task_id: task.id.clone(),
                index: 0,
                timestamp: now_millis(),
                r#type: input.r#type,
                level: input.level,
                data: input.data,
                series_id: None,
                series_mode: None,
                series_acc_field: None,
                series_snapshot: None,
                _accumulated_data: None,
            })
            .collect::<Vec<_>>();
        for attempt in 0..3 {
            let token = if attempt == 0 {
                initial_token.clone()
            } else {
                coordinator
                    .ensure_task_hot_for_write_without_rehydrate(&task.id)
                    .await?
            };
            if token.storage_epoch != initial_token.storage_epoch {
                return Err(EngineError::Store(Box::new(
                    StorageFenceConflictError::new(
                        "Task storage epoch changed after the write mutation started",
                    ),
                )));
            }
            match self
                .short_term_store
                .commit_task_events_fenced(task.clone(), expected_revision, events.clone(), &token)
                .await
            {
                Ok(Some(committed)) => return Ok(committed),
                Ok(None) => {
                    let current = self.get_task(&task.id).await?;
                    return Err(EngineError::InvalidTransition {
                        from: current
                            .map(|current| current.status)
                            .unwrap_or_else(|| expected_status.clone()),
                        to: task.status.clone(),
                    });
                }
                Err(error)
                    if error.downcast_ref::<StorageFenceConflictError>().is_some()
                        && attempt < 2 =>
                {
                    continue;
                }
                Err(error) => return Err(EngineError::Store(error)),
            }
        }
        Err(EngineError::Store(Box::new(
            StorageFenceConflictError::default(),
        )))
    }

    async fn finish_committed_event(
        &self,
        event: TaskEvent,
        accumulated_event: Option<TaskEvent>,
    ) -> Result<(), EngineError> {
        let broadcast_event = if let Some(ref accumulated) = accumulated_event {
            TaskEvent {
                _accumulated_data: Some(accumulated.data.clone()),
                ..event.clone()
            }
        } else {
            event.clone()
        };
        self.broadcast
            .publish(&event.task_id, broadcast_event)
            .await?;

        if let Some(ref long_term_store) = self.long_term_store {
            let long_term_store = Arc::clone(long_term_store);
            let raw_event = event.clone();
            let store_event = accumulated_event
                .clone()
                .unwrap_or_else(|| raw_event.clone());
            let hooks = self.hooks.clone();
            tokio::spawn(async move {
                if let Err(err) =
                    persist_long_term_event(long_term_store, raw_event, accumulated_event).await
                {
                    if let Some(hooks) = hooks {
                        hooks.on_event_dropped(&store_event, &err.to_string());
                    }
                }
            });
        }
        Ok(())
    }
}

async fn persist_long_term_event(
    long_term_store: Arc<dyn LongTermStore>,
    event: TaskEvent,
    accumulated_event: Option<TaskEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if long_term_store.supports_series_compaction() {
        if let (Some(series_id), Some(series_mode)) =
            (event.series_id.clone(), event.series_mode.clone())
        {
            match series_mode {
                SeriesMode::Latest => {
                    let task_id = event.task_id.clone();
                    return long_term_store
                        .replace_last_series_event(&task_id, &series_id, event)
                        .await;
                }
                SeriesMode::Accumulate => {
                    let task_id = event.task_id.clone();
                    let field = event
                        .series_acc_field
                        .clone()
                        .unwrap_or_else(|| "delta".to_string());
                    long_term_store
                        .accumulate_series(&task_id, &series_id, event, &field)
                        .await?;
                    return Ok(());
                }
                SeriesMode::KeepAll => {}
            }
        }
    }

    long_term_store
        .save_event(accumulated_event.unwrap_or(event))
        .await
}

fn is_compactable_series_event(event: &TaskEvent) -> bool {
    event.series_id.is_some()
        && matches!(
            event.series_mode,
            Some(SeriesMode::Latest) | Some(SeriesMode::Accumulate)
        )
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_millis() as f64
}

fn contiguous_prefix_len(events: &[TaskEvent]) -> u64 {
    let indexes: HashSet<u64> = events.iter().map(|event| event.index).collect();
    let mut expected = 0;
    while indexes.contains(&expected) {
        expected += 1;
    }
    expected
}

fn canonical_durable_query(
    opts: Option<&EventQueryOptions>,
    hot_events: &[TaskEvent],
    durable_series: &[DurableSeriesState],
) -> Option<EventQueryOptions> {
    let mut query = opts.cloned()?;
    if let Some(id) = query
        .since
        .as_ref()
        .and_then(|since| since.id.as_ref())
        .cloned()
    {
        let anchor = hot_events.iter().find(|event| event.id == id).or_else(|| {
            durable_series
                .iter()
                .find(|state| state.event.id == id)
                .map(|state| &state.event)
        });
        if let Some(anchor) = anchor {
            query.since = Some(SinceCursor {
                id: None,
                index: Some(anchor.index),
                timestamp: None,
            });
        }
    }
    Some(query)
}

fn unsupported_archive_restore(message: &'static str) -> EngineError {
    EngineError::Store(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        message,
    )))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_adapters::{
        MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore,
    };
    use crate::types::{LongTermStore, SeriesMode, StorageBusyError, WorkerAuditEvent};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::RwLock as TokioRwLock;

    // ─── Mock LongTermStore ───────────────────────────────────────────

    struct MockLongTermStore {
        tasks: TokioRwLock<HashMap<String, Task>>,
        events: TokioRwLock<Vec<TaskEvent>>,
        replace_latest_calls: AtomicU64,
        accumulate_calls: AtomicU64,
        fail_save_event: bool,
    }

    impl MockLongTermStore {
        fn new() -> Self {
            Self {
                tasks: TokioRwLock::new(HashMap::new()),
                events: TokioRwLock::new(Vec::new()),
                replace_latest_calls: AtomicU64::new(0),
                accumulate_calls: AtomicU64::new(0),
                fail_save_event: false,
            }
        }

        fn failing_save_event() -> Self {
            Self {
                tasks: TokioRwLock::new(HashMap::new()),
                events: TokioRwLock::new(Vec::new()),
                replace_latest_calls: AtomicU64::new(0),
                accumulate_calls: AtomicU64::new(0),
                fail_save_event: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl LongTermStore for MockLongTermStore {
        async fn save_task(
            &self,
            task: Task,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.tasks.write().await.insert(task.id.clone(), task);
            Ok(())
        }

        async fn get_task(
            &self,
            task_id: &str,
        ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.tasks.read().await.get(task_id).cloned())
        }

        async fn save_event(
            &self,
            event: TaskEvent,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if self.fail_save_event {
                return Err("mock save_event failure".into());
            }
            self.events.write().await.push(event);
            Ok(())
        }

        async fn replace_last_series_event(
            &self,
            _task_id: &str,
            _series_id: &str,
            event: TaskEvent,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.replace_latest_calls.fetch_add(1, Ordering::SeqCst);
            let mut events = self.events.write().await;
            let existing_index = events.iter().position(|candidate| {
                candidate.task_id == event.task_id
                    && candidate.series_id == event.series_id
                    && candidate.series_mode == Some(SeriesMode::Latest)
            });
            if let Some(existing_index) = existing_index {
                let existing = events[existing_index].clone();
                events[existing_index] = TaskEvent {
                    id: existing.id,
                    index: existing.index,
                    ..event
                };
            } else {
                events.push(event);
            }
            Ok(())
        }

        async fn accumulate_series(
            &self,
            _task_id: &str,
            _series_id: &str,
            event: TaskEvent,
            field: &str,
        ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
            self.accumulate_calls.fetch_add(1, Ordering::SeqCst);
            let mut events = self.events.write().await;
            let existing_index = events.iter().position(|candidate| {
                candidate.task_id == event.task_id
                    && candidate.series_id == event.series_id
                    && candidate.series_mode == Some(SeriesMode::Accumulate)
            });

            let accumulated = if let Some(existing_index) = existing_index {
                let existing = events[existing_index].clone();
                let previous = existing
                    .data
                    .as_object()
                    .and_then(|data| data.get(field))
                    .and_then(|value| value.as_str());
                let current = event
                    .data
                    .as_object()
                    .and_then(|data| data.get(field))
                    .and_then(|value| value.as_str());
                let accumulated = match (previous, current) {
                    (Some(previous), Some(current)) => {
                        let mut data = event.data.as_object().cloned().unwrap_or_default();
                        data.insert(
                            field.to_string(),
                            serde_json::Value::String(format!("{previous}{current}")),
                        );
                        TaskEvent {
                            data: serde_json::Value::Object(data),
                            ..event
                        }
                    }
                    _ => event,
                };
                events[existing_index] = TaskEvent {
                    id: existing.id,
                    index: existing.index,
                    ..accumulated.clone()
                };
                accumulated
            } else {
                events.push(event.clone());
                event
            };

            Ok(accumulated)
        }

        async fn get_events(
            &self,
            task_id: &str,
            _opts: Option<EventQueryOptions>,
        ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
            let events = self.events.read().await;
            Ok(events
                .iter()
                .filter(|e| e.task_id == task_id)
                .cloned()
                .collect())
        }

        fn supports_series_compaction(&self) -> bool {
            true
        }

        async fn save_worker_event(
            &self,
            _event: WorkerAuditEvent,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        async fn get_worker_events(
            &self,
            _worker_id: &str,
            _opts: Option<EventQueryOptions>,
        ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(Vec::new())
        }
    }

    // ─── Mock Hooks ───────────────────────────────────────────────────

    struct MockHooks {
        dropped_count: AtomicU64,
    }

    impl MockHooks {
        fn new() -> Self {
            Self {
                dropped_count: AtomicU64::new(0),
            }
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

    #[tokio::test]
    async fn stale_write_retry_does_not_rehydrate_a_released_task() {
        let hot = Arc::new(MemoryShortTermStore::new());
        let durable = Arc::new(MemoryLongTermStore::new());
        let engine = TaskEngine::new(TaskEngineOptions {
            short_term_store: hot.clone(),
            long_term_store: Some(durable),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            hooks: None,
        });
        engine
            .create_task(CreateTaskInput {
                id: Some("task-stale-write".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("task-stale-write", TaskStatus::Running, None)
            .await
            .unwrap();
        let event = hot
            .get_events("task-stale-write", None)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let coordinator = engine.storage_coordinator.as_ref().unwrap();
        let initial = coordinator
            .ensure_task_hot_for_write("task-stale-write")
            .await
            .unwrap();

        engine
            .release_task_storage(
                "task-stale-write",
                ReleasePreconditions {
                    expected_last_event_index: event.index as i64,
                    inactive_since: event.timestamp,
                },
            )
            .await
            .unwrap();

        let stale_retry = coordinator
            .ensure_task_hot_for_write_without_rehydrate("task-stale-write")
            .await
            .unwrap_err();
        assert!(stale_retry.downcast_ref::<StorageBusyError>().is_some());
        let fresh = coordinator
            .ensure_task_hot_for_write("task-stale-write")
            .await
            .unwrap();
        assert_eq!(fresh.storage_epoch, initial.storage_epoch + 1);
    }

    // ─── create_task ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_task_generates_id_and_sets_status_pending() {
        let engine = make_engine();
        let task = engine
            .create_task(CreateTaskInput::default())
            .await
            .unwrap();

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
            matches!(err, EngineError::TaskConflict(_)),
            "Expected TaskConflict error, got: {err}"
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
        assert_eq!(events[0].data, serde_json::json!({"status": "running"}));
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

        let ids: std::collections::HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
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
        let task = engine
            .create_task(CreateTaskInput::default())
            .await
            .unwrap();
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
        let task = engine
            .create_task(CreateTaskInput::default())
            .await
            .unwrap();
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
                engine
                    .create_task(CreateTaskInput::default())
                    .await
                    .unwrap()
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
        let task = engine
            .create_task(CreateTaskInput::default())
            .await
            .unwrap();
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
            assert_eq!(
                *ids, published_ids,
                "subscriber {i} received events in wrong order"
            );
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
        let engine =
            make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        let task = engine
            .create_task(CreateTaskInput {
                id: Some("lt-1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

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
            r#type: None,
            params: None,
            result: None,
            error: None,
            metadata: None,
            completed_at: None,
            ttl: None,
            auth_config: None,
            webhooks: None,
            cleanup: None,
            tags: None,
            assign_mode: None,
            cost: None,
            assigned_worker: None,
            disconnect_policy: None,
            reason: None,
            resume_at: None,
            blocked_request: None,
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
        let engine =
            make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        engine
            .create_task(CreateTaskInput {
                id: Some("lt-2".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        engine
            .transition_task("lt-2", TaskStatus::Running, None)
            .await
            .unwrap();

        let retrieved = long_term_store.get_task("lt-2").await.unwrap().unwrap();
        assert_eq!(retrieved.status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn emit_saves_event_to_long_term_async() {
        let long_term_store = Arc::new(MockLongTermStore::new());
        let engine =
            make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        engine
            .create_task(CreateTaskInput {
                id: Some("lt-3".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("lt-3", TaskStatus::Running, None)
            .await
            .unwrap();

        engine
            .publish_event(
                "lt-3",
                PublishEventInput {
                    r#type: "test".to_string(),
                    level: Level::Info,
                    data: serde_json::json!(null),
                    series_id: None,
                    series_mode: None,

                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

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

        engine
            .create_task(CreateTaskInput {
                id: Some("lt-fail".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task("lt-fail", TaskStatus::Running, None)
            .await
            .unwrap();

        engine
            .publish_event(
                "lt-fail",
                PublishEventInput {
                    r#type: "test".to_string(),
                    level: Level::Info,
                    data: serde_json::json!(null),
                    series_id: None,
                    series_mode: None,

                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        // Give async spawn time to execute
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(hooks.dropped_count.load(Ordering::SeqCst) >= 1);
    }

    // ─── get_series_latest ──────────────────────────────────────────────

    #[tokio::test]
    async fn get_series_latest_returns_none_when_no_series() {
        let engine = make_engine();
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = engine.get_series_latest("t1", "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_series_latest_returns_accumulated_after_publish() {
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
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "Hello "}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "world"}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();

        let latest = engine.get_series_latest("t1", "s1").await.unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.data["delta"], "Hello world");
    }

    #[tokio::test]
    async fn emit_accumulate_broadcasts_with_accumulated_data() {
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

        // Subscribe to collect broadcast events
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);
        let _unsub = engine
            .subscribe(
                "t1",
                Box::new(move |event| {
                    received_clone.lock().unwrap().push(event);
                }),
            )
            .await;

        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "Hello "}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "world"}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();

        let events = received.lock().unwrap();
        let chunks: Vec<_> = events.iter().filter(|e| e.r#type == "llm.chunk").collect();

        assert_eq!(chunks.len(), 2);
        // First broadcast: delta="Hello ", accumulated_data="Hello "
        assert_eq!(chunks[0].data["delta"], "Hello ");
        assert!(chunks[0]._accumulated_data.is_some());
        assert_eq!(
            chunks[0]._accumulated_data.as_ref().unwrap()["delta"],
            "Hello "
        );

        // Second broadcast: delta="world", accumulated_data="Hello world"
        assert_eq!(chunks[1].data["delta"], "world");
        assert!(chunks[1]._accumulated_data.is_some());
        assert_eq!(
            chunks[1]._accumulated_data.as_ref().unwrap()["delta"],
            "Hello world"
        );
    }

    #[tokio::test]
    async fn emit_accumulate_stores_delta_in_short_term() {
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
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "Hello "}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "world"}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();

        // ShortTermStore events should contain deltas (not accumulated)
        let events = engine.get_events("t1", None).await.unwrap();
        let chunks: Vec<_> = events.iter().filter(|e| e.r#type == "llm.chunk").collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].data["delta"], "Hello ");
        assert_eq!(chunks[1].data["delta"], "world");
    }

    #[tokio::test]
    async fn emit_accumulate_compacts_accumulated_in_long_term() {
        let long_term = Arc::new(MockLongTermStore::new());
        let engine = TaskEngine::new(TaskEngineOptions {
            short_term_store: Arc::new(MemoryShortTermStore::new()),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            long_term_store: Some(Arc::clone(&long_term) as Arc<dyn LongTermStore>),
            hooks: None,
        });
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
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "Hello "}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "llm.chunk".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"delta": "world"}),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();

        // Give async spawn time to execute
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // LongTermStore should have one compacted accumulated event.
        let lt_events = long_term.events.read().await;
        let chunks: Vec<_> = lt_events
            .iter()
            .filter(|e| e.r#type == "llm.chunk")
            .collect();
        assert_eq!(long_term.accumulate_calls.load(Ordering::SeqCst), 2);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].data["delta"], "Hello world");
    }

    #[tokio::test]
    async fn emit_latest_compacts_in_long_term() {
        let long_term = Arc::new(MockLongTermStore::new());
        let engine = TaskEngine::new(TaskEngineOptions {
            short_term_store: Arc::new(MemoryShortTermStore::new()),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            long_term_store: Some(Arc::clone(&long_term) as Arc<dyn LongTermStore>),
            hooks: None,
        });
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

        for status in ["starting", "ready"] {
            engine
                .publish_event(
                    "t1",
                    PublishEventInput {
                        r#type: "task.status".to_string(),
                        level: Level::Info,
                        data: serde_json::json!({"status": status}),
                        series_id: Some("status".to_string()),
                        series_mode: Some(SeriesMode::Latest),
                        series_acc_field: None,
                    },
                )
                .await
                .unwrap();
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let lt_events = long_term.events.read().await;
        let statuses: Vec<_> = lt_events
            .iter()
            .filter(|event| event.r#type == "task.status")
            .collect();
        assert_eq!(long_term.replace_latest_calls.load(Ordering::SeqCst), 2);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].data["status"], "ready");
    }

    #[tokio::test]
    async fn get_events_falls_back_to_long_term_store() {
        let long_term_store = Arc::new(MockLongTermStore::new());
        let event = TaskEvent {
            id: "lt-evt-1".to_string(),
            task_id: "cold-task".to_string(),
            index: 0,
            timestamp: 1000.0,
            r#type: "test".to_string(),
            level: Level::Info,
            data: serde_json::json!({"text": "from long term"}),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        };
        long_term_store.events.write().await.push(event.clone());

        let engine =
            make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        let events = engine.get_events("cold-task", None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "lt-evt-1");
    }

    #[tokio::test]
    async fn get_events_prefers_short_term_store() {
        let long_term_store = Arc::new(MockLongTermStore::new());
        let engine =
            make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

        let task = engine
            .create_task(CreateTaskInput {
                r#type: Some("test".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        engine
            .transition_task(&task.id, TaskStatus::Running, None)
            .await
            .unwrap();
        engine
            .publish_event(
                &task.id,
                PublishEventInput {
                    r#type: "test".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({}),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        let events = engine.get_events(&task.id, None).await.unwrap();
        assert!(!events.is_empty());
    }

    #[tokio::test]
    async fn emit_non_series_has_no_accumulated_data() {
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

        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_clone = Arc::clone(&received);
        let _unsub = engine
            .subscribe(
                "t1",
                Box::new(move |event| {
                    received_clone.lock().unwrap().push(event);
                }),
            )
            .await;

        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: serde_json::json!({"pct": 50}),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        let events = received.lock().unwrap();
        let progress: Vec<_> = events.iter().filter(|e| e.r#type == "progress").collect();
        assert_eq!(progress.len(), 1);
        assert!(progress[0]._accumulated_data.is_none());
    }
}
