use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;

use crate::types::{
    ArchiveBatch, ArchiveBatchReceipt, ArchiveGeneration, ArchiveGenerationStatus,
    ArchiveSourcePage, BroadcastProvider, ClosedWriteFence, DurableSeriesState, EventQueryOptions,
    HotWriteToken, LongTermStore, RehydrateSnapshot, SeriesMode, SeriesResult, ShortTermStore,
    StorageFenceConflictError, StorageIntegrityError, StorageLease, StorageState,
    StorageWriterRegistration, Task, TaskArchiveImportOptions, TaskArchiveRestoreData, TaskEvent,
    TaskFilter, TaskMutationSnapshot, TaskStatus, TaskStorageMetadata, TaskStorageMetadataCas,
    TaskStoragePresence, TaskWriteFence, Worker, WorkerAssignment, WorkerAuditEvent, WorkerFilter,
};
use crate::{
    compute_archive_batch_digest, compute_archive_source_digest,
    compute_archive_source_page_digest, compute_series_state_digest,
};

// ─── MemoryBroadcastProvider ────────────────────────────────────────────────

type Handler = Arc<dyn Fn(TaskEvent) + Send + Sync>;

pub struct MemoryBroadcastProvider {
    listeners: Arc<RwLock<HashMap<String, Vec<Handler>>>>,
}

impl MemoryBroadcastProvider {
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for MemoryBroadcastProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BroadcastProvider for MemoryBroadcastProvider {
    async fn publish(
        &self,
        channel: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let handlers = {
            let listeners = self.listeners.read().unwrap();
            listeners.get(channel).cloned()
        };
        if let Some(handlers) = handlers {
            for handler in &handlers {
                handler(event.clone());
            }
        }
        Ok(())
    }

    async fn subscribe(
        &self,
        channel: &str,
        handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        self.subscribe_sync(channel, handler)
            .expect("MemoryBroadcastProvider::subscribe_sync should never fail")
    }

    fn subscribe_sync(
        &self,
        channel: &str,
        handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Result<Box<dyn Fn() + Send + Sync>, Box<dyn std::error::Error + Send + Sync>> {
        let handler: Handler = Arc::from(handler);
        {
            let mut listeners = self.listeners.write().unwrap();
            listeners
                .entry(channel.to_string())
                .or_default()
                .push(Arc::clone(&handler));
        }

        let listeners = Arc::clone(&self.listeners);
        let channel = channel.to_string();
        // Store the pointer address as usize for Send + Sync compatibility.
        // This is only used for identity comparison, never dereferenced.
        let handler_addr = Arc::as_ptr(&handler) as *const () as usize;

        Ok(Box::new(move || {
            let mut listeners = listeners.write().unwrap();
            if let Some(handlers) = listeners.get_mut(&channel) {
                handlers.retain(|h| (Arc::as_ptr(h) as *const () as usize) != handler_addr);
            }
        }))
    }
}

// ─── MemoryShortTermStore ───────────────────────────────────────────────────

pub struct MemoryShortTermStore {
    task_event_guard: Mutex<()>,
    tasks: RwLock<HashMap<String, Task>>,
    task_revisions: RwLock<HashMap<String, u64>>,
    events: RwLock<HashMap<String, Vec<TaskEvent>>>,
    series_latest: RwLock<HashMap<String, TaskEvent>>,
    index_counters: RwLock<HashMap<String, Arc<AtomicU64>>>,
    workers: RwLock<HashMap<String, Worker>>,
    assignments: RwLock<Vec<WorkerAssignment>>,
    storage_locks: RwLock<HashMap<String, MemoryStorageLock>>,
    write_fences: RwLock<HashMap<String, TaskWriteFence>>,
    storage_writers: RwLock<HashMap<String, StorageWriterRegistration>>,
}

#[derive(Clone)]
struct MemoryStorageLock {
    lock_token: String,
    generation: String,
    storage_epoch: u64,
    expires_at: u128,
}

#[derive(Clone)]
struct MemoryCreationClaim {
    token: String,
    expires_at: Option<u128>,
    completed_at: Option<u128>,
}

impl MemoryShortTermStore {
    pub fn new() -> Self {
        Self {
            task_event_guard: Mutex::new(()),
            tasks: RwLock::new(HashMap::new()),
            task_revisions: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            series_latest: RwLock::new(HashMap::new()),
            index_counters: RwLock::new(HashMap::new()),
            workers: RwLock::new(HashMap::new()),
            assignments: RwLock::new(Vec::new()),
            storage_locks: RwLock::new(HashMap::new()),
            write_fences: RwLock::new(HashMap::new()),
            storage_writers: RwLock::new(HashMap::new()),
        }
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }

    fn owns_storage_lock(&self, lease: &StorageLease) -> bool {
        let now = Self::now_ms();
        let locks = self.storage_locks.read().unwrap();
        matches!(
            locks.get(&lease.task_id),
            Some(current)
                if current.expires_at > now
                    && current.lock_token == lease.lock_token
                    && current.generation == lease.generation
                    && current.storage_epoch == lease.storage_epoch
        )
    }

    fn assert_owned_storage_lock(
        &self,
        lease: &StorageLease,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.owns_storage_lock(lease) {
            Ok(())
        } else {
            Err(Box::new(StorageFenceConflictError::new(
                "Storage lease is stale",
            )))
        }
    }
}

impl Default for MemoryShortTermStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─── MemoryLongTermStore ───────────────────────────────────────────────────

pub struct MemoryLongTermStore {
    lifecycle_guard: Mutex<()>,
    tasks: RwLock<HashMap<String, Task>>,
    events: RwLock<HashMap<String, Vec<TaskEvent>>>,
    metadata: RwLock<HashMap<String, TaskStorageMetadata>>,
    generations: RwLock<HashMap<(String, String), ArchiveGeneration>>,
    batches: RwLock<HashMap<(String, String), BTreeMap<u64, ArchiveBatch>>>,
    series: RwLock<HashMap<String, Vec<DurableSeriesState>>>,
    worker_events: RwLock<HashMap<String, Vec<WorkerAuditEvent>>>,
    creation_claims: RwLock<HashMap<String, MemoryCreationClaim>>,
}

impl MemoryLongTermStore {
    pub fn new() -> Self {
        Self {
            lifecycle_guard: Mutex::new(()),
            tasks: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            metadata: RwLock::new(HashMap::new()),
            generations: RwLock::new(HashMap::new()),
            batches: RwLock::new(HashMap::new()),
            series: RwLock::new(HashMap::new()),
            worker_events: RwLock::new(HashMap::new()),
            creation_claims: RwLock::new(HashMap::new()),
        }
    }

    fn upsert_event(&self, event: TaskEvent) {
        let mut all_events = self.events.write().unwrap();
        let events = all_events.entry(event.task_id.clone()).or_default();
        if let Some(existing) = events
            .iter_mut()
            .find(|candidate| candidate.index == event.index)
        {
            *existing = event;
        } else {
            events.push(event);
        }
        events.sort_by_key(|candidate| candidate.index);
    }

    fn upsert_series_state(&self, state: DurableSeriesState) {
        let mut all_series = self.series.write().unwrap();
        let states = all_series.entry(state.task_id.clone()).or_default();
        if let Some(existing) = states
            .iter_mut()
            .find(|candidate| candidate.series_id == state.series_id)
        {
            *existing = state;
        } else {
            states.push(state);
        }
        states.sort_by(|left, right| left.series_id.cmp(&right.series_id));
    }

    fn is_pristine_creation_claim(&self, task_id: &str) -> bool {
        let pristine_task = self.tasks.read().unwrap().get(task_id).is_some_and(|task| {
            task.status == TaskStatus::Pending
                && task.updated_at == task.created_at
                && task.result.is_none()
                && task.error.is_none()
                && task.completed_at.is_none()
        });
        pristine_task
            && self
                .metadata
                .read()
                .unwrap()
                .get(task_id)
                .is_some_and(|metadata| {
                    metadata.storage_state == StorageState::Hot
                        && metadata.storage_epoch == 1
                        && metadata.active_release_generation.is_none()
                        && metadata.archive_watermark == -1
                        && metadata.last_event_at.is_none()
                        && metadata.cold_at.is_none()
                        && metadata.task_version == 0
                })
            && self
                .events
                .read()
                .unwrap()
                .get(task_id)
                .is_none_or(Vec::is_empty)
    }
}

impl Default for MemoryLongTermStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LongTermStore for MemoryLongTermStore {
    fn supports_hot_cold_release(&self) -> bool {
        true
    }

    fn supports_task_creation_claims(&self) -> bool {
        true
    }

    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let task_id = task.id.clone();
        let deadline = task.ttl.map(|ttl| task.created_at + ttl as f64 * 1_000.0);
        self.tasks.write().unwrap().insert(task_id.clone(), task);
        self.metadata
            .write()
            .unwrap()
            .entry(task_id.clone())
            .or_insert(TaskStorageMetadata {
                task_id,
                storage_state: StorageState::Hot,
                storage_epoch: 1,
                active_release_generation: None,
                archive_watermark: -1,
                last_event_at: None,
                cold_at: None,
                execution_deadline_at: deadline,
                task_version: 0,
            });
        Ok(())
    }

    async fn create_task_if_absent(
        &self,
        task: Task,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let task_id = task.id.clone();
        let deadline = task.ttl.map(|ttl| task.created_at + ttl as f64 * 1_000.0);
        {
            let mut tasks = self.tasks.write().unwrap();
            if tasks.contains_key(&task_id) {
                return Ok(false);
            }
            tasks.insert(task_id.clone(), task);
        }
        self.metadata
            .write()
            .unwrap()
            .entry(task_id.clone())
            .or_insert(TaskStorageMetadata {
                task_id,
                storage_state: StorageState::Hot,
                storage_epoch: 1,
                active_release_generation: None,
                archive_watermark: -1,
                last_event_at: None,
                cold_at: None,
                execution_deadline_at: deadline,
                task_version: 0,
            });
        Ok(true)
    }

    async fn claim_task_creation(
        &self,
        task: Task,
        creation_token: &str,
        claim_ttl_ms: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if claim_ttl_ms == 0 {
            return Err(Box::new(StorageIntegrityError::new(
                "Creation claim TTL must be positive",
            )));
        }
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let task_id = task.id.clone();
        let now = MemoryShortTermStore::now_ms();
        let deadline = task.ttl.map(|ttl| task.created_at + ttl as f64 * 1_000.0);
        if self.tasks.read().unwrap().contains_key(&task_id) {
            let claims = self.creation_claims.read().unwrap();
            let can_take_over = claims.get(&task_id).is_some_and(|claim| {
                claim.completed_at.is_none()
                    && claim.expires_at.is_none_or(|expires_at| expires_at <= now)
            });
            drop(claims);
            if !can_take_over || !self.is_pristine_creation_claim(&task_id) {
                return Ok(false);
            }
        }
        self.tasks.write().unwrap().insert(task_id.clone(), task);
        self.metadata.write().unwrap().insert(
            task_id.clone(),
            TaskStorageMetadata {
                task_id: task_id.clone(),
                storage_state: StorageState::Hot,
                storage_epoch: 1,
                active_release_generation: None,
                archive_watermark: -1,
                last_event_at: None,
                cold_at: None,
                execution_deadline_at: deadline,
                task_version: 0,
            },
        );
        self.creation_claims.write().unwrap().insert(
            task_id,
            MemoryCreationClaim {
                token: creation_token.to_string(),
                expires_at: Some(now + claim_ttl_ms as u128),
                completed_at: None,
            },
        );
        Ok(true)
    }

    async fn complete_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let mut claims = self.creation_claims.write().unwrap();
        let Some(claim) = claims.get_mut(task_id) else {
            return Ok(false);
        };
        if claim.token != creation_token {
            return Ok(false);
        }
        claim
            .completed_at
            .get_or_insert_with(MemoryShortTermStore::now_ms);
        claim.expires_at = None;
        Ok(true)
    }

    async fn abort_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let claims = self.creation_claims.read().unwrap();
        let owns_incomplete_claim = claims
            .get(task_id)
            .is_some_and(|claim| claim.token == creation_token && claim.completed_at.is_none());
        drop(claims);
        if !owns_incomplete_claim {
            return Ok(false);
        }
        if !self.is_pristine_creation_claim(task_id) {
            return Ok(false);
        }
        self.creation_claims.write().unwrap().remove(task_id);
        self.tasks.write().unwrap().remove(task_id);
        self.metadata.write().unwrap().remove(task_id);
        Ok(true)
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.tasks.read().unwrap().get(task_id).cloned())
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let task_id = event.task_id.clone();
        let timestamp = event.timestamp;
        self.upsert_event(event);
        if let Some(metadata) = self.metadata.write().unwrap().get_mut(&task_id) {
            metadata.last_event_at = Some(timestamp);
        }
        Ok(())
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let previous = self.series.read().unwrap().get(task_id).and_then(|states| {
            states
                .iter()
                .find(|state| state.series_id == series_id)
                .cloned()
        });
        let archive_watermark = self
            .metadata
            .read()
            .unwrap()
            .get(task_id)
            .map(|metadata| metadata.archive_watermark)
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new(format!(
                    "Series task does not exist: {task_id}"
                ))) as Box<dyn std::error::Error + Send + Sync>
            })?;
        if archive_watermark >= event.index as i64
            || previous
                .as_ref()
                .is_some_and(|state| state.through_index >= event.index)
        {
            if archive_watermark >= event.index as i64 && previous.is_none() {
                return Err(Box::new(StorageIntegrityError::new(format!(
                    "Archived latest series state is missing for {task_id}:{series_id}"
                ))));
            }
            return Ok(());
        }
        if previous.as_ref().is_some_and(|state| {
            state.mode != SeriesMode::Latest
                || state.event.series_acc_field != event.series_acc_field
        }) {
            return Err(Box::new(StorageIntegrityError::new(format!(
                "Durable series semantics conflict for {task_id}:{series_id}"
            ))));
        }
        if let Some(previous) = previous {
            if let Some(events) = self.events.write().unwrap().get_mut(task_id) {
                events.retain(|candidate| candidate.id != previous.event.id);
            }
        }
        self.upsert_event(event.clone());
        self.upsert_series_state(DurableSeriesState {
            task_id: task_id.to_string(),
            series_id: series_id.to_string(),
            mode: SeriesMode::Latest,
            through_index: event.index,
            event,
        });
        Ok(())
    }

    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let previous = self.series.read().unwrap().get(task_id).and_then(|states| {
            states
                .iter()
                .find(|state| state.series_id == series_id)
                .cloned()
        });
        let archive_watermark = self
            .metadata
            .read()
            .unwrap()
            .get(task_id)
            .map(|metadata| metadata.archive_watermark)
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new(format!(
                    "Series task does not exist: {task_id}"
                ))) as Box<dyn std::error::Error + Send + Sync>
            })?;
        if archive_watermark >= event.index as i64
            || previous
                .as_ref()
                .is_some_and(|state| state.through_index >= event.index)
        {
            let Some(previous) = previous else {
                return Err(Box::new(StorageIntegrityError::new(format!(
                    "Archived accumulate series state is missing for {task_id}:{series_id}"
                ))));
            };
            return Ok(previous.event);
        }
        if previous.as_ref().is_some_and(|state| {
            state.mode != SeriesMode::Accumulate
                || state.event.series_acc_field.as_deref().unwrap_or("delta") != field
                || event.series_acc_field.as_deref().unwrap_or("delta") != field
        }) {
            return Err(Box::new(StorageIntegrityError::new(format!(
                "Durable series semantics conflict for {task_id}:{series_id}"
            ))));
        }
        let mut accumulated = event.clone();
        if let Some(previous_state) = &previous {
            if let (Some(previous_value), Some(next_value)) = (
                previous_state
                    .event
                    .data
                    .as_object()
                    .and_then(|data| data.get(field))
                    .and_then(|value| value.as_str()),
                event
                    .data
                    .as_object()
                    .and_then(|data| data.get(field))
                    .and_then(|value| value.as_str()),
            ) {
                if let Some(data) = accumulated.data.as_object_mut() {
                    data.insert(
                        field.to_string(),
                        serde_json::Value::String(format!("{previous_value}{next_value}")),
                    );
                }
            }
            if let Some(events) = self.events.write().unwrap().get_mut(task_id) {
                events.retain(|candidate| candidate.id != previous_state.event.id);
            }
        }
        self.upsert_event(accumulated.clone());
        self.upsert_series_state(DurableSeriesState {
            task_id: task_id.to_string(),
            series_id: series_id.to_string(),
            mode: SeriesMode::Accumulate,
            through_index: event.index,
            event: accumulated.clone(),
        });
        Ok(accumulated)
    }

    fn supports_series_compaction(&self) -> bool {
        true
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let mut events = self
            .events
            .read()
            .unwrap()
            .get(task_id)
            .cloned()
            .unwrap_or_default();
        if let Some(options) = opts {
            if let Some(since) = options.since {
                if let Some(id) = since.id {
                    if let Some(position) = events.iter().position(|event| event.id == id) {
                        events = events.into_iter().skip(position + 1).collect();
                    }
                } else if let Some(index) = since.index {
                    events.retain(|event| event.index > index);
                } else if let Some(timestamp) = since.timestamp {
                    events.retain(|event| event.timestamp > timestamp);
                }
            }
            if let Some(limit) = options.limit {
                events.truncate(limit as usize);
            }
        }
        Ok(events)
    }

    async fn get_task_storage_metadata(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskStorageMetadata>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.metadata.read().unwrap().get(task_id).cloned())
    }

    async fn compare_and_set_task_storage_metadata(
        &self,
        update: TaskStorageMetadataCas,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let mut metadata = self.metadata.write().unwrap();
        let Some(current) = metadata.get(&update.task_id) else {
            return Ok(false);
        };
        if current.storage_state != update.expected_storage_state
            || current.storage_epoch != update.expected_storage_epoch
            || current.active_release_generation != update.expected_release_generation
        {
            return Ok(false);
        }
        metadata.insert(update.task_id, update.next);
        Ok(true)
    }

    async fn begin_archive(
        &self,
        generation: ArchiveGeneration,
    ) -> Result<ArchiveGeneration, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let metadata = self.metadata.read().unwrap();
        let current = metadata.get(&generation.task_id).ok_or_else(|| {
            Box::new(StorageIntegrityError::new("Archive task does not exist"))
                as Box<dyn std::error::Error + Send + Sync>
        })?;
        if current.storage_state != StorageState::Releasing
            || current.storage_epoch != generation.storage_epoch
            || current.active_release_generation.as_deref() != Some(generation.generation.as_str())
            || current.archive_watermark != generation.manifest.prior_watermark
        {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive generation lost its durable release fence",
            )));
        }
        drop(metadata);
        let key = (generation.task_id.clone(), generation.generation.clone());
        let mut generations = self.generations.write().unwrap();
        if let Some(existing) = generations.get(&key) {
            if existing != &generation {
                return Err(Box::new(StorageIntegrityError::new(
                    "Archive generation replay conflicts",
                )));
            }
            return Ok(existing.clone());
        }
        generations.insert(key, generation.clone());
        Ok(generation)
    }

    async fn archive_batch(
        &self,
        task_id: &str,
        generation: &str,
        batch: ArchiveBatch,
    ) -> Result<ArchiveBatchReceipt, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let expected_digest = compute_archive_batch_digest(
            batch.receipt.previous_batch_digest.as_deref(),
            &batch.events,
            &batch.series_latest,
        )?;
        if expected_digest != batch.receipt.batch_digest {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive batch digest mismatch",
            )));
        }
        let key = (task_id.to_string(), generation.to_string());
        let archive = self
            .generations
            .read()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new(
                    "Archive generation does not exist",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
        if archive.status != ArchiveGenerationStatus::Open {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive generation is not open",
            )));
        }
        let metadata = self.metadata.read().unwrap();
        let current = metadata.get(task_id).ok_or_else(|| {
            Box::new(StorageIntegrityError::new("Archive task does not exist"))
                as Box<dyn std::error::Error + Send + Sync>
        })?;
        if current.storage_state != StorageState::Releasing
            || current.storage_epoch != archive.storage_epoch
            || current.active_release_generation.as_deref() != Some(generation)
        {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive batch lost its durable release fence",
            )));
        }
        drop(metadata);
        let mut all_batches = self.batches.write().unwrap();
        let batches = all_batches.entry(key).or_default();
        if let Some(existing) = batches.get(&batch.receipt.ordinal) {
            if existing.receipt != batch.receipt || existing.events != batch.events {
                return Err(Box::new(StorageIntegrityError::new(
                    "Archive batch replay conflicts",
                )));
            }
            return Ok(existing.receipt.clone());
        }
        let expected_ordinal = archive
            .manifest
            .expected_batch_ordinals
            .get(batches.len())
            .copied();
        if expected_ordinal != Some(batch.receipt.ordinal) {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive batch ordinal is out of order",
            )));
        }
        for event in &batch.events {
            self.upsert_event(event.clone());
        }
        let receipt = batch.receipt.clone();
        batches.insert(receipt.ordinal, batch);
        Ok(receipt)
    }

    async fn finalize_archive(
        &self,
        task_id: &str,
        generation: &str,
        task: Task,
        series_latest: Vec<DurableSeriesState>,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let _lifecycle = self.lifecycle_guard.lock().unwrap();
        let key = (task_id.to_string(), generation.to_string());
        let archive = self
            .generations
            .read()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new(
                    "Archive generation does not exist",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
        let batches = self
            .batches
            .read()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let ordinals = batches.keys().copied().collect::<Vec<_>>();
        if ordinals != archive.manifest.expected_batch_ordinals {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive generation is missing batches",
            )));
        }
        let source_entry_count = batches
            .values()
            .map(|batch| batch.events.len() as u64)
            .sum::<u64>();
        let page_digests = batches
            .values()
            .map(|batch| compute_archive_source_page_digest(&batch.events))
            .collect::<Result<Vec<_>, _>>()?;
        if source_entry_count != archive.manifest.source_entry_count
            || compute_archive_source_digest(&page_digests) != archive.manifest.source_digest
            || compute_series_state_digest(&series_latest)? != archive.manifest.series_state_digest
        {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive generation manifest verification failed",
            )));
        }
        let mut metadata = self.metadata.write().unwrap();
        let current = metadata.get_mut(task_id).ok_or_else(|| {
            Box::new(StorageIntegrityError::new("Archive task does not exist"))
                as Box<dyn std::error::Error + Send + Sync>
        })?;
        if current.storage_state != StorageState::Releasing
            || current.storage_epoch != archive.storage_epoch
            || current.active_release_generation.as_deref() != Some(generation)
        {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive finalization lost its durable release fence",
            )));
        }
        current.archive_watermark = archive.target_watermark;
        drop(metadata);
        self.tasks
            .write()
            .unwrap()
            .insert(task_id.to_string(), task);
        let compact_series_ids = series_latest
            .iter()
            .map(|state| state.series_id.clone())
            .collect::<std::collections::HashSet<_>>();
        if let Some(events) = self.events.write().unwrap().get_mut(task_id) {
            events.retain(|event| {
                event.series_id.as_ref().is_none_or(|series_id| {
                    !compact_series_ids.contains(series_id)
                        || !matches!(
                            event.series_mode,
                            Some(SeriesMode::Latest) | Some(SeriesMode::Accumulate)
                        )
                })
            });
            events.extend(series_latest.iter().map(|state| state.event.clone()));
            events.sort_by_key(|event| event.index);
        }
        self.series
            .write()
            .unwrap()
            .insert(task_id.to_string(), series_latest);
        if let Some(generation) = self.generations.write().unwrap().get_mut(&key) {
            generation.status = ArchiveGenerationStatus::Finalized;
        }
        Ok(archive.target_watermark)
    }

    async fn get_archive_watermark(
        &self,
        task_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        self.metadata
            .read()
            .unwrap()
            .get(task_id)
            .map(|metadata| metadata.archive_watermark)
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new("Task does not exist"))
                    as Box<dyn std::error::Error + Send + Sync>
            })
    }

    async fn get_last_event_index(
        &self,
        task_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .events
            .read()
            .unwrap()
            .get(task_id)
            .and_then(|events| events.last())
            .map(|event| event.index as i64)
            .unwrap_or(-1))
    }

    async fn get_recent_events(
        &self,
        task_id: &str,
        limit: u64,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let events = self
            .events
            .read()
            .unwrap()
            .get(task_id)
            .cloned()
            .unwrap_or_default();
        Ok(events
            .into_iter()
            .rev()
            .take(limit as usize)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect())
    }

    async fn get_durable_series_state(
        &self,
        task_id: &str,
    ) -> Result<Vec<DurableSeriesState>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .series
            .read()
            .unwrap()
            .get(task_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.worker_events
            .write()
            .unwrap()
            .entry(event.worker_id.clone())
            .or_default()
            .push(event);
        Ok(())
    }

    async fn get_worker_events(
        &self,
        worker_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .worker_events
            .read()
            .unwrap()
            .get(worker_id)
            .cloned()
            .unwrap_or_default())
    }
}

#[async_trait]
impl ShortTermStore for MemoryShortTermStore {
    fn supports_hot_cold_release(&self) -> bool {
        true
    }

    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _mutation = self.task_event_guard.lock().unwrap();
        let mut tasks = self.tasks.write().unwrap();
        let task_id = task.id.clone();
        tasks.insert(task_id.clone(), task);
        drop(tasks);
        let mut revisions = self.task_revisions.write().unwrap();
        *revisions.entry(task_id.clone()).or_insert(0) += 1;
        drop(revisions);
        self.write_fences
            .write()
            .unwrap()
            .entry(task_id.clone())
            .or_insert(TaskWriteFence {
                task_id,
                accepting_writes: true,
                storage_epoch: 1,
                active_release_generation: None,
            });
        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let tasks = self.tasks.read().unwrap();
        Ok(tasks.get(task_id).cloned())
    }

    async fn get_task_mutation_snapshot(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskMutationSnapshot>, Box<dyn std::error::Error + Send + Sync>> {
        let _mutation = self.task_event_guard.lock().unwrap();
        let tasks = self.tasks.read().unwrap();
        let Some(task) = tasks.get(task_id).cloned() else {
            return Ok(None);
        };
        let revision = self
            .task_revisions
            .read()
            .unwrap()
            .get(task_id)
            .copied()
            .unwrap_or(0);
        Ok(Some(TaskMutationSnapshot {
            task,
            revision: revision.to_string(),
        }))
    }

    async fn append_event(
        &self,
        task_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut events = self.events.write().unwrap();
        events.entry(task_id.to_string()).or_default().push(event);
        Ok(())
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let events = self.events.read().unwrap();
        let all = match events.get(task_id) {
            Some(v) => v.clone(),
            None => return Ok(vec![]),
        };

        let mut result = all;

        if let Some(ref opts) = opts {
            if let Some(ref since) = opts.since {
                if let Some(ref id) = since.id {
                    // since.id takes priority
                    let idx = result.iter().position(|e| &e.id == id);
                    result = match idx {
                        Some(i) => result[i + 1..].to_vec(),
                        None => result,
                    };
                } else if let Some(index) = since.index {
                    // since.index is second priority
                    result.retain(|e| e.index > index);
                } else if let Some(timestamp) = since.timestamp {
                    // since.timestamp is third priority
                    result.retain(|e| e.timestamp > timestamp);
                }
            }

            if let Some(limit) = opts.limit {
                result.truncate(limit as usize);
            }
        }

        Ok(result)
    }

    async fn set_ttl(
        &self,
        _task_id: &str,
        _ttl_seconds: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // no-op in memory adapter
        Ok(())
    }

    async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let series = self.series_latest.read().unwrap();
        let key = format!("{task_id}:{series_id}");
        Ok(series.get(&key).cloned())
    }

    async fn set_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut series = self.series_latest.write().unwrap();
        let key = format!("{task_id}:{series_id}");
        series.insert(key, event);
        Ok(())
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let key = format!("{task_id}:{series_id}");

        let prev = {
            let series = self.series_latest.read().unwrap();
            series.get(&key).cloned()
        };

        if let Some(prev) = prev {
            let mut events = self.events.write().unwrap();
            if let Some(task_events) = events.get_mut(task_id) {
                if let Some(idx) = task_events.iter().rposition(|e| e.id == prev.id) {
                    task_events[idx] = event.clone();
                }
            }
        } else {
            self.append_event(task_id, event.clone()).await?;
        }

        let mut series = self.series_latest.write().unwrap();
        series.insert(key, event);
        Ok(())
    }

    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        // Atomic read-modify-write under a single write lock
        let key = format!("{task_id}:{series_id}");
        let mut series = self.series_latest.write().unwrap();
        let prev = series.get(&key).cloned();

        let accumulated = if let Some(prev) = prev {
            let should_concat = prev
                .data
                .as_object()
                .and_then(|po| po.get(field)?.as_str().map(|s| s.to_string()))
                .zip(
                    event
                        .data
                        .as_object()
                        .and_then(|no| no.get(field)?.as_str().map(|s| s.to_string())),
                );

            if let Some((prev_val, new_val)) = should_concat {
                let mut new_data = event.data.as_object().cloned().unwrap_or_default();
                new_data.insert(
                    field.to_string(),
                    serde_json::Value::String(prev_val + &new_val),
                );
                TaskEvent {
                    data: serde_json::Value::Object(new_data),
                    ..event
                }
            } else {
                event
            }
        } else {
            event
        };
        series.insert(key, accumulated.clone());
        Ok(accumulated)
    }

    async fn next_index(
        &self,
        task_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let counter = {
            let mut counters = self.index_counters.write().unwrap();
            counters
                .entry(task_id.to_string())
                .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                .clone()
        };
        Ok(counter.fetch_add(1, Ordering::SeqCst))
    }

    fn supports_task_archive_restore(&self) -> bool {
        true
    }

    async fn validate_task_archive_restore(
        &self,
        data: &TaskArchiveRestoreData,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let task_id = &data.task.id;
        let tasks = self.tasks.read().unwrap();
        if tasks.contains_key(task_id) && !options.unwrap_or_default().overwrite {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Task already exists: {task_id}"),
            )));
        }
        Ok(())
    }

    async fn restore_task_archive(
        &self,
        data: TaskArchiveRestoreData,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.validate_task_archive_restore(&data, options).await?;
        let _mutation = self.task_event_guard.lock().unwrap();

        let task_id = data.task.id.clone();
        let overwritten = {
            let tasks = self.tasks.read().unwrap();
            tasks.contains_key(&task_id)
        };

        {
            let mut series = self.series_latest.write().unwrap();
            let prefix = format!("{task_id}:");
            series.retain(|key, _| !key.starts_with(&prefix));
            for entry in &data.series_latest {
                series.insert(
                    format!("{}:{}", entry.task_id, entry.series_id),
                    entry.event.clone(),
                );
            }
        }

        self.tasks
            .write()
            .unwrap()
            .insert(task_id.clone(), data.task);
        let mut revisions = self.task_revisions.write().unwrap();
        *revisions.entry(task_id.clone()).or_insert(0) += 1;
        drop(revisions);
        self.events
            .write()
            .unwrap()
            .insert(task_id.clone(), data.events);
        self.index_counters
            .write()
            .unwrap()
            .insert(task_id, Arc::new(AtomicU64::new(data.next_index)));

        Ok(overwritten)
    }

    async fn acquire_storage_lock(
        &self,
        task_id: &str,
        lock_token: &str,
        generation: &str,
        ttl_ms: u64,
    ) -> Result<Option<StorageLease>, Box<dyn std::error::Error + Send + Sync>> {
        let now = Self::now_ms();
        let mut locks = self.storage_locks.write().unwrap();
        if let Some(current) = locks.get_mut(task_id) {
            if current.expires_at > now {
                if current.lock_token != lock_token || current.generation != generation {
                    return Ok(None);
                }
                current.expires_at = now + ttl_ms as u128;
                return Ok(Some(StorageLease {
                    task_id: task_id.to_string(),
                    lock_token: lock_token.to_string(),
                    generation: generation.to_string(),
                    storage_epoch: current.storage_epoch,
                }));
            }
        }

        let storage_epoch = self
            .write_fences
            .read()
            .unwrap()
            .get(task_id)
            .map(|fence| fence.storage_epoch)
            .unwrap_or(1);
        locks.insert(
            task_id.to_string(),
            MemoryStorageLock {
                lock_token: lock_token.to_string(),
                generation: generation.to_string(),
                storage_epoch,
                expires_at: now + ttl_ms as u128,
            },
        );
        Ok(Some(StorageLease {
            task_id: task_id.to_string(),
            lock_token: lock_token.to_string(),
            generation: generation.to_string(),
            storage_epoch,
        }))
    }

    async fn renew_storage_lock(
        &self,
        lease: &StorageLease,
        ttl_ms: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let now = Self::now_ms();
        let mut locks = self.storage_locks.write().unwrap();
        let Some(current) = locks.get_mut(&lease.task_id) else {
            return Ok(false);
        };
        if current.expires_at <= now
            || current.lock_token != lease.lock_token
            || current.generation != lease.generation
            || current.storage_epoch != lease.storage_epoch
        {
            return Ok(false);
        }
        current.expires_at = now + ttl_ms as u128;
        Ok(true)
    }

    async fn release_storage_lock(
        &self,
        lease: &StorageLease,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !self.owns_storage_lock(lease) {
            return Ok(false);
        }
        self.storage_locks.write().unwrap().remove(&lease.task_id);
        Ok(true)
    }

    async fn get_write_fence(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskWriteFence>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.write_fences.read().unwrap().get(task_id).cloned())
    }

    async fn close_write_fence(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<ClosedWriteFence, Box<dyn std::error::Error + Send + Sync>> {
        self.assert_owned_storage_lock(lease)?;
        let mut fences = self.write_fences.write().unwrap();
        let Some(fence) = fences.get_mut(&lease.task_id) else {
            return Err(Box::new(StorageFenceConflictError::default()));
        };
        if fence.storage_epoch != expected_epoch {
            return Err(Box::new(StorageFenceConflictError::default()));
        }

        let counter_watermark = self
            .index_counters
            .read()
            .unwrap()
            .get(&lease.task_id)
            .map(|counter| counter.load(Ordering::SeqCst) as i64 - 1)
            .unwrap_or(-1);
        let event_watermark = self
            .events
            .read()
            .unwrap()
            .get(&lease.task_id)
            .and_then(|events| events.iter().map(|event| event.index as i64).max())
            .unwrap_or(-1);
        let high_watermark = counter_watermark.max(event_watermark);
        fence.accepting_writes = false;
        fence.active_release_generation = Some(lease.generation.clone());
        Ok(ClosedWriteFence {
            task_id: lease.task_id.clone(),
            accepting_writes: false,
            storage_epoch: fence.storage_epoch,
            active_release_generation: fence.active_release_generation.clone(),
            high_watermark,
        })
    }

    async fn reopen_write_fence(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<HotWriteToken, Box<dyn std::error::Error + Send + Sync>> {
        self.assert_owned_storage_lock(lease)?;
        let mut fences = self.write_fences.write().unwrap();
        let Some(fence) = fences.get_mut(&lease.task_id) else {
            return Err(Box::new(StorageFenceConflictError::default()));
        };
        if fence.accepting_writes
            || fence.storage_epoch != expected_epoch
            || fence.active_release_generation.as_deref() != Some(lease.generation.as_str())
        {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        fence.accepting_writes = true;
        fence.storage_epoch = expected_epoch + 1;
        fence.active_release_generation = None;
        Ok(HotWriteToken {
            task_id: lease.task_id.clone(),
            storage_epoch: fence.storage_epoch,
        })
    }

    async fn commit_event_fenced(
        &self,
        task_id: &str,
        mut event: TaskEvent,
        token: &HotWriteToken,
    ) -> Result<SeriesResult, Box<dyn std::error::Error + Send + Sync>> {
        let fences = self.write_fences.read().unwrap();
        let Some(fence) = fences.get(task_id) else {
            return Err(Box::new(StorageFenceConflictError::default()));
        };
        if token.task_id != task_id
            || !fence.accepting_writes
            || fence.storage_epoch != token.storage_epoch
        {
            return Err(Box::new(StorageFenceConflictError::default()));
        }

        let counter = {
            let mut counters = self.index_counters.write().unwrap();
            counters
                .entry(task_id.to_string())
                .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                .clone()
        };
        event.task_id = task_id.to_string();
        event.index = counter.fetch_add(1, Ordering::SeqCst);

        if let (Some(series_id), Some(SeriesMode::Latest)) =
            (event.series_id.as_deref(), event.series_mode.as_ref())
        {
            let key = format!("{task_id}:{series_id}");
            let mut series = self.series_latest.write().unwrap();
            let previous = series.get(&key).cloned();
            let mut events = self.events.write().unwrap();
            let task_events = events.entry(task_id.to_string()).or_default();
            if let Some(previous) = previous {
                if let Some(position) = task_events
                    .iter()
                    .rposition(|candidate| candidate.id == previous.id)
                {
                    task_events[position] = event.clone();
                } else {
                    task_events.push(event.clone());
                }
            } else {
                task_events.push(event.clone());
            }
            series.insert(key, event.clone());
            return Ok(SeriesResult {
                event,
                accumulated_event: None,
                stored: true,
            });
        }

        self.events
            .write()
            .unwrap()
            .entry(task_id.to_string())
            .or_default()
            .push(event.clone());

        let accumulated_event = if let (Some(series_id), Some(SeriesMode::Accumulate)) =
            (event.series_id.as_deref(), event.series_mode.as_ref())
        {
            let key = format!("{task_id}:{series_id}");
            let field = event.series_acc_field.as_deref().unwrap_or("delta");
            let mut series = self.series_latest.write().unwrap();
            let accumulated = if let Some(previous) = series.get(&key) {
                let previous_value = previous.data.get(field).and_then(|value| value.as_str());
                let delta = event.data.get(field).and_then(|value| value.as_str());
                if let (Some(previous_value), Some(delta)) = (previous_value, delta) {
                    let mut accumulated = event.clone();
                    let mut data = event.data.as_object().cloned().unwrap_or_default();
                    data.insert(
                        field.to_string(),
                        serde_json::Value::String(format!("{previous_value}{delta}")),
                    );
                    accumulated.data = serde_json::Value::Object(data);
                    accumulated
                } else {
                    event.clone()
                }
            } else {
                event.clone()
            };
            series.insert(key, accumulated.clone());
            Some(accumulated)
        } else {
            None
        };

        Ok(SeriesResult {
            event,
            accumulated_event,
            stored: true,
        })
    }

    async fn commit_task_events_fenced(
        &self,
        task: Task,
        expected_revision: &str,
        mut events: Vec<TaskEvent>,
        token: &HotWriteToken,
    ) -> Result<Option<Vec<TaskEvent>>, Box<dyn std::error::Error + Send + Sync>> {
        let _mutation = self.task_event_guard.lock().unwrap();
        let fences = self.write_fences.read().unwrap();
        let Some(fence) = fences.get(&task.id) else {
            return Err(Box::new(StorageFenceConflictError::default()));
        };
        if token.task_id != task.id
            || !fence.accepting_writes
            || fence.storage_epoch != token.storage_epoch
            || events.iter().any(|event| {
                event.task_id != task.id || event.series_id.is_some() || event.series_mode.is_some()
            })
        {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        if self
            .task_revisions
            .read()
            .unwrap()
            .get(&task.id)
            .copied()
            .unwrap_or(0)
            .to_string()
            != expected_revision
        {
            return Ok(None);
        }
        let counter = {
            let mut counters = self.index_counters.write().unwrap();
            counters
                .entry(task.id.clone())
                .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                .clone()
        };
        for event in &mut events {
            event.index = counter.fetch_add(1, Ordering::SeqCst);
        }
        self.tasks.write().unwrap().insert(task.id.clone(), task);
        let mut revisions = self.task_revisions.write().unwrap();
        *revisions.entry(token.task_id.clone()).or_insert(0) += 1;
        drop(revisions);
        self.events
            .write()
            .unwrap()
            .entry(token.task_id.clone())
            .or_default()
            .extend(events.iter().cloned());
        Ok(Some(events))
    }

    async fn save_task_fenced(
        &self,
        task: Task,
        token: &HotWriteToken,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _mutation = self.task_event_guard.lock().unwrap();
        let fences = self.write_fences.read().unwrap();
        let Some(fence) = fences.get(&task.id) else {
            return Err(Box::new(StorageFenceConflictError::default()));
        };
        if token.task_id != task.id
            || !fence.accepting_writes
            || fence.storage_epoch != token.storage_epoch
        {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        self.tasks.write().unwrap().insert(task.id.clone(), task);
        let mut revisions = self.task_revisions.write().unwrap();
        *revisions.entry(token.task_id.clone()).or_insert(0) += 1;
        Ok(())
    }

    async fn read_archive_source_page(
        &self,
        task_id: &str,
        watermark: i64,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<ArchiveSourcePage, Box<dyn std::error::Error + Send + Sync>> {
        let offset = cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let mut source = self
            .events
            .read()
            .unwrap()
            .get(task_id)
            .cloned()
            .unwrap_or_default();
        source.retain(|event| event.index as i64 <= watermark);
        source.sort_by_key(|event| event.index);
        let events = source
            .iter()
            .skip(offset)
            .take(limit.max(1) as usize)
            .cloned()
            .collect::<Vec<_>>();
        let next_offset = offset + events.len();
        let done = next_offset >= source.len();
        Ok(ArchiveSourcePage {
            task_id: task_id.to_string(),
            watermark,
            cursor: cursor.map(str::to_string),
            next_cursor: (!done).then(|| next_offset.to_string()),
            events,
            done,
        })
    }

    async fn delete_task_storage_fenced(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.assert_owned_storage_lock(lease)?;
        {
            let fences = self.write_fences.read().unwrap();
            let Some(fence) = fences.get(&lease.task_id) else {
                return Err(Box::new(StorageFenceConflictError::default()));
            };
            if fence.accepting_writes
                || fence.storage_epoch != expected_epoch
                || fence.active_release_generation.as_deref() != Some(lease.generation.as_str())
            {
                return Err(Box::new(StorageFenceConflictError::default()));
            }
        }

        self.tasks.write().unwrap().remove(&lease.task_id);
        self.task_revisions.write().unwrap().remove(&lease.task_id);
        self.events.write().unwrap().remove(&lease.task_id);
        self.index_counters.write().unwrap().remove(&lease.task_id);
        self.write_fences.write().unwrap().remove(&lease.task_id);
        let prefix = format!("{}:", lease.task_id);
        self.series_latest
            .write()
            .unwrap()
            .retain(|key, _| !key.starts_with(&prefix));
        Ok(())
    }

    async fn restore_hot_task_fenced(
        &self,
        snapshot: RehydrateSnapshot,
        lease: &StorageLease,
        next_epoch: u64,
    ) -> Result<HotWriteToken, Box<dyn std::error::Error + Send + Sync>> {
        self.assert_owned_storage_lock(lease)?;
        if snapshot.task.id != lease.task_id || next_epoch <= snapshot.storage_epoch {
            return Err(Box::new(StorageFenceConflictError::default()));
        }

        let task_id = snapshot.task.id.clone();
        self.tasks
            .write()
            .unwrap()
            .insert(task_id.clone(), snapshot.task);
        self.task_revisions
            .write()
            .unwrap()
            .insert(task_id.clone(), 1);
        self.events
            .write()
            .unwrap()
            .insert(task_id.clone(), snapshot.replay_events);
        self.index_counters.write().unwrap().insert(
            task_id.clone(),
            Arc::new(AtomicU64::new((snapshot.max_event_index + 1).max(0) as u64)),
        );
        let prefix = format!("{task_id}:");
        let mut series = self.series_latest.write().unwrap();
        series.retain(|key, _| !key.starts_with(&prefix));
        for entry in snapshot.series_latest {
            series.insert(format!("{task_id}:{}", entry.series_id), entry.event);
        }
        drop(series);
        self.write_fences.write().unwrap().insert(
            task_id.clone(),
            TaskWriteFence {
                task_id: task_id.clone(),
                accepting_writes: true,
                storage_epoch: next_epoch,
                active_release_generation: None,
            },
        );
        Ok(HotWriteToken {
            task_id,
            storage_epoch: next_epoch,
        })
    }

    async fn get_task_storage_presence(
        &self,
        task_id: &str,
    ) -> Result<TaskStoragePresence, Box<dyn std::error::Error + Send + Sync>> {
        let prefix = format!("{task_id}:");
        Ok(TaskStoragePresence {
            task: self.tasks.read().unwrap().contains_key(task_id),
            event_count: self
                .events
                .read()
                .unwrap()
                .get(task_id)
                .map(|events| events.len() as u64)
                .unwrap_or(0),
            next_index: self.index_counters.read().unwrap().contains_key(task_id),
            series_state_count: self
                .series_latest
                .read()
                .unwrap()
                .keys()
                .filter(|key| key.starts_with(&prefix))
                .count() as u64,
            write_fence: self.write_fences.read().unwrap().contains_key(task_id),
        })
    }

    async fn register_storage_writer(
        &self,
        mut registration: StorageWriterRegistration,
        ttl_ms: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        registration.expires_at = (Self::now_ms() + ttl_ms as u128) as f64;
        self.storage_writers
            .write()
            .unwrap()
            .insert(registration.instance_id.clone(), registration);
        Ok(())
    }

    async fn list_storage_writers(
        &self,
    ) -> Result<Vec<StorageWriterRegistration>, Box<dyn std::error::Error + Send + Sync>> {
        let now = Self::now_ms() as f64;
        let mut writers = self.storage_writers.write().unwrap();
        writers.retain(|_, registration| registration.expires_at > now);
        Ok(writers.values().cloned().collect())
    }

    async fn list_tasks(
        &self,
        filter: TaskFilter,
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let tasks = self.tasks.read().unwrap();
        Ok(tasks
            .values()
            .filter(|t| {
                if let Some(ref statuses) = filter.status {
                    if !statuses.contains(&t.status) {
                        return false;
                    }
                }
                if let Some(ref types) = filter.types {
                    if let Some(ref task_type) = t.r#type {
                        if !types.iter().any(|ty| ty == task_type) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                if let Some(ref modes) = filter.assign_mode {
                    if let Some(ref am) = t.assign_mode {
                        if !modes.contains(am) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                if let Some(ref tag_matcher) = filter.tags {
                    if !crate::worker_matching::matches_tag(t.tags.as_deref(), tag_matcher) {
                        return false;
                    }
                }
                if let Some(ref exclude) = filter.exclude_task_ids {
                    if exclude.contains(&t.id) {
                        return false;
                    }
                }
                true
            })
            .take(filter.limit.unwrap_or(u64::MAX) as usize)
            .cloned()
            .collect())
    }

    async fn save_worker(
        &self,
        worker: Worker,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut workers = self.workers.write().unwrap();
        workers.insert(worker.id.clone(), worker);
        Ok(())
    }

    async fn get_worker(
        &self,
        worker_id: &str,
    ) -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        let workers = self.workers.read().unwrap();
        Ok(workers.get(worker_id).cloned())
    }

    async fn list_workers(
        &self,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        let workers = self.workers.read().unwrap();
        Ok(workers
            .values()
            .filter(|w| {
                if let Some(ref f) = filter {
                    if let Some(ref statuses) = f.status {
                        if !statuses.contains(&w.status) {
                            return false;
                        }
                    }
                    if let Some(ref modes) = f.connection_mode {
                        if !modes.contains(&w.connection_mode) {
                            return false;
                        }
                    }
                }
                true
            })
            .cloned()
            .collect())
    }

    async fn delete_worker(
        &self,
        worker_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut workers = self.workers.write().unwrap();
        workers.remove(worker_id);
        Ok(())
    }

    async fn claim_task(
        &self,
        task_id: &str,
        worker_id: &str,
        cost: u32,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _mutation = self.task_event_guard.lock().unwrap();
        // Phase 1: Check and update worker capacity (write lock, then release)
        {
            let mut workers = self.workers.write().unwrap();
            match workers.get_mut(worker_id) {
                Some(w) if w.used_slots + cost <= w.capacity => {
                    w.used_slots += cost;
                }
                _ => return Ok(false),
            }
        }

        // Phase 2: Update task (write lock only)
        let mut tasks = self.tasks.write().unwrap();
        let task = match tasks.get_mut(task_id) {
            Some(t) if t.status == TaskStatus::Pending || t.status == TaskStatus::Assigned => t,
            _ => {
                // Rollback worker used_slots
                let mut workers = self.workers.write().unwrap();
                if let Some(w) = workers.get_mut(worker_id) {
                    w.used_slots = w.used_slots.saturating_sub(cost);
                }
                return Ok(false);
            }
        };
        task.status = TaskStatus::Assigned;
        task.assigned_worker = Some(worker_id.to_string());
        task.cost = Some(cost);
        task.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64;
        let mut revisions = self.task_revisions.write().unwrap();
        *revisions.entry(task_id.to_string()).or_insert(0) += 1;

        Ok(true)
    }

    async fn add_assignment(
        &self,
        assignment: WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut assignments = self.assignments.write().unwrap();
        assignments.push(assignment);
        Ok(())
    }

    async fn remove_assignment(
        &self,
        task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut assignments = self.assignments.write().unwrap();
        assignments.retain(|a| a.task_id != task_id);
        Ok(())
    }

    async fn get_worker_assignments(
        &self,
        worker_id: &str,
    ) -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        let assignments = self.assignments.read().unwrap();
        Ok(assignments
            .iter()
            .filter(|a| a.worker_id == worker_id)
            .cloned()
            .collect())
    }

    async fn get_task_assignment(
        &self,
        task_id: &str,
    ) -> Result<Option<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        let assignments = self.assignments.read().unwrap();
        Ok(assignments.iter().find(|a| a.task_id == task_id).cloned())
    }

    async fn clear_ttl(
        &self,
        _task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // no-op in memory adapter (no TTL tracking)
        Ok(())
    }

    async fn list_by_status(
        &self,
        statuses: &[TaskStatus],
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let tasks = self.tasks.read().unwrap();
        Ok(tasks
            .values()
            .filter(|t| statuses.contains(&t.status))
            .cloned()
            .collect())
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AssignMode, ConnectionMode, Level, TagMatcher, TaskStatus, Worker, WorkerAssignment,
        WorkerAssignmentStatus, WorkerFilter, WorkerMatchRule, WorkerStatus,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn make_task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            r#type: Some("test".to_string()),
            status: TaskStatus::Running,
            params: None,
            result: None,
            error: None,
            metadata: None,
            created_at: 1000.0,
            updated_at: 1000.0,
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
        }
    }

    fn make_event(id: &str, task_id: &str, index: u64, timestamp: f64) -> TaskEvent {
        TaskEvent {
            id: id.to_string(),
            task_id: task_id.to_string(),
            index,
            timestamp,
            r#type: "progress".to_string(),
            level: Level::Info,
            data: json!({ "index": index }),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        }
    }

    // ─── MemoryShortTermStore: save/get task ────────────────────────────

    #[tokio::test]
    async fn short_term_store_save_and_get_task() {
        let store = MemoryShortTermStore::new();
        let task = make_task("t1");
        store.save_task(task.clone()).await.unwrap();

        let retrieved = store.get_task("t1").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "t1");
        assert_eq!(retrieved.status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn short_term_store_get_nonexistent_task_returns_none() {
        let store = MemoryShortTermStore::new();
        let result = store.get_task("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn short_term_store_save_task_overwrites() {
        let store = MemoryShortTermStore::new();
        let task1 = make_task("t1");
        store.save_task(task1).await.unwrap();

        let mut task2 = make_task("t1");
        task2.status = TaskStatus::Completed;
        store.save_task(task2).await.unwrap();

        let retrieved = store.get_task("t1").await.unwrap().unwrap();
        assert_eq!(retrieved.status, TaskStatus::Completed);
    }

    // ─── MemoryShortTermStore: append/get events ────────────────────────

    #[tokio::test]
    async fn short_term_store_append_and_get_events() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e3", "t1", 2, 3000.0))
            .await
            .unwrap();

        let events = store.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].id, "e1");
        assert_eq!(events[1].id, "e2");
        assert_eq!(events[2].id, "e3");
    }

    #[tokio::test]
    async fn short_term_store_get_events_empty_task() {
        let store = MemoryShortTermStore::new();
        let events = store.get_events("nonexistent", None).await.unwrap();
        assert!(events.is_empty());
    }

    // ─── MemoryShortTermStore: since.id cursor ──────────────────────────

    #[tokio::test]
    async fn short_term_store_get_events_since_id() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e3", "t1", 2, 3000.0))
            .await
            .unwrap();

        let opts = EventQueryOptions {
            since: Some(crate::types::SinceCursor {
                id: Some("e1".to_string()),
                index: None,
                timestamp: None,
            }),
            limit: None,
        };
        let events = store.get_events("t1", Some(opts)).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e2");
        assert_eq!(events[1].id, "e3");
    }

    #[tokio::test]
    async fn short_term_store_get_events_since_id_not_found_returns_all() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();

        let opts = EventQueryOptions {
            since: Some(crate::types::SinceCursor {
                id: Some("nonexistent".to_string()),
                index: None,
                timestamp: None,
            }),
            limit: None,
        };
        let events = store.get_events("t1", Some(opts)).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    // ─── MemoryShortTermStore: since.index cursor ───────────────────────

    #[tokio::test]
    async fn short_term_store_get_events_since_index() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e3", "t1", 2, 3000.0))
            .await
            .unwrap();

        let opts = EventQueryOptions {
            since: Some(crate::types::SinceCursor {
                id: None,
                index: Some(0),
                timestamp: None,
            }),
            limit: None,
        };
        let events = store.get_events("t1", Some(opts)).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e2");
        assert_eq!(events[1].id, "e3");
    }

    // ─── MemoryShortTermStore: since.timestamp cursor ───────────────────

    #[tokio::test]
    async fn short_term_store_get_events_since_timestamp() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e3", "t1", 2, 3000.0))
            .await
            .unwrap();

        let opts = EventQueryOptions {
            since: Some(crate::types::SinceCursor {
                id: None,
                index: None,
                timestamp: Some(1000.0),
            }),
            limit: None,
        };
        let events = store.get_events("t1", Some(opts)).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e2");
        assert_eq!(events[1].id, "e3");
    }

    // ─── MemoryShortTermStore: since.id takes priority over index ───────

    #[tokio::test]
    async fn short_term_store_since_id_takes_priority_over_index() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e3", "t1", 2, 3000.0))
            .await
            .unwrap();

        // since.id = "e2" (should skip e1 and e2), even though index = 0 would keep e2 and e3
        let opts = EventQueryOptions {
            since: Some(crate::types::SinceCursor {
                id: Some("e2".to_string()),
                index: Some(0),
                timestamp: None,
            }),
            limit: None,
        };
        let events = store.get_events("t1", Some(opts)).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "e3");
    }

    // ─── MemoryShortTermStore: limit ────────────────────────────────────

    #[tokio::test]
    async fn short_term_store_get_events_with_limit() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e3", "t1", 2, 3000.0))
            .await
            .unwrap();

        let opts = EventQueryOptions {
            since: None,
            limit: Some(2),
        };
        let events = store.get_events("t1", Some(opts)).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e1");
        assert_eq!(events[1].id, "e2");
    }

    #[tokio::test]
    async fn short_term_store_get_events_since_and_limit() {
        let store = MemoryShortTermStore::new();
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e3", "t1", 2, 3000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e4", "t1", 3, 4000.0))
            .await
            .unwrap();

        let opts = EventQueryOptions {
            since: Some(crate::types::SinceCursor {
                id: Some("e1".to_string()),
                index: None,
                timestamp: None,
            }),
            limit: Some(2),
        };
        let events = store.get_events("t1", Some(opts)).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e2");
        assert_eq!(events[1].id, "e3");
    }

    // ─── MemoryShortTermStore: setTTL no-op ─────────────────────────────

    #[tokio::test]
    async fn short_term_store_set_ttl_is_noop() {
        let store = MemoryShortTermStore::new();
        let result = store.set_ttl("t1", 3600).await;
        assert!(result.is_ok());
    }

    // ─── MemoryShortTermStore: series operations ────────────────────────

    #[tokio::test]
    async fn short_term_store_get_series_latest_returns_none_initially() {
        let store = MemoryShortTermStore::new();
        let result = store.get_series_latest("t1", "s1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn short_term_store_set_and_get_series_latest() {
        let store = MemoryShortTermStore::new();
        let event = make_event("e1", "t1", 0, 1000.0);
        store
            .set_series_latest("t1", "s1", event.clone())
            .await
            .unwrap();

        let result = store.get_series_latest("t1", "s1").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "e1");
    }

    #[tokio::test]
    async fn short_term_store_set_series_latest_overwrites() {
        let store = MemoryShortTermStore::new();
        store
            .set_series_latest("t1", "s1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .set_series_latest("t1", "s1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();

        let result = store.get_series_latest("t1", "s1").await.unwrap();
        assert_eq!(result.unwrap().id, "e2");
    }

    #[tokio::test]
    async fn short_term_store_replace_last_series_event_replaces_in_events() {
        let store = MemoryShortTermStore::new();

        // Append some events
        store
            .append_event("t1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        store
            .append_event("t1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();

        // Set e2 as series latest
        store
            .set_series_latest("t1", "s1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();

        // Replace with e3
        let replacement = make_event("e3", "t1", 1, 2500.0);
        store
            .replace_last_series_event("t1", "s1", replacement)
            .await
            .unwrap();

        // The events list should have e3 in place of e2
        let events = store.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "e1");
        assert_eq!(events[1].id, "e3");

        // The series latest should be e3
        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.id, "e3");
    }

    #[tokio::test]
    async fn short_term_store_replace_last_series_event_appends_when_no_previous() {
        let store = MemoryShortTermStore::new();

        // No prior series latest, should append
        let event = make_event("e1", "t1", 0, 1000.0);
        store
            .replace_last_series_event("t1", "s1", event)
            .await
            .unwrap();

        let events = store.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "e1");

        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.id, "e1");
    }

    #[tokio::test]
    async fn short_term_store_replace_last_series_event_finds_from_end() {
        let store = MemoryShortTermStore::new();

        // Append events with duplicate IDs at different positions
        // to verify rposition (search from end) behavior
        let mut e1 = make_event("e1", "t1", 0, 1000.0);
        e1.data = json!("first");
        store.append_event("t1", e1).await.unwrap();

        let e2 = make_event("e2", "t1", 1, 2000.0);
        store.append_event("t1", e2.clone()).await.unwrap();

        let e3 = make_event("e3", "t1", 2, 3000.0);
        store.append_event("t1", e3).await.unwrap();

        // Set e2 as latest for series s1
        store.set_series_latest("t1", "s1", e2).await.unwrap();

        // Replace
        let replacement = make_event("e2_replaced", "t1", 1, 2500.0);
        store
            .replace_last_series_event("t1", "s1", replacement)
            .await
            .unwrap();

        let events = store.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].id, "e1");
        assert_eq!(events[1].id, "e2_replaced");
        assert_eq!(events[2].id, "e3");
    }

    // ─── MemoryBroadcastProvider: publish with no subscribers ────────────

    #[tokio::test]
    async fn broadcast_publish_with_no_subscribers() {
        let provider = MemoryBroadcastProvider::new();
        let event = make_event("e1", "t1", 0, 1000.0);
        let result = provider.publish("channel1", event).await;
        assert!(result.is_ok());
    }

    // ─── MemoryBroadcastProvider: publish with subscriber ───────────────

    #[tokio::test]
    async fn broadcast_publish_with_subscriber() {
        let provider = MemoryBroadcastProvider::new();
        let received = Arc::new(AtomicU64::new(0));
        let received_clone = Arc::clone(&received);

        let _unsub = provider
            .subscribe(
                "channel1",
                Box::new(move |_event| {
                    received_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        let event = make_event("e1", "t1", 0, 1000.0);
        provider.publish("channel1", event).await.unwrap();

        assert_eq!(received.load(Ordering::SeqCst), 1);
    }

    // ─── MemoryBroadcastProvider: unsubscribe works ─────────────────────

    #[tokio::test]
    async fn broadcast_unsubscribe_stops_delivery() {
        let provider = MemoryBroadcastProvider::new();
        let received = Arc::new(AtomicU64::new(0));
        let received_clone = Arc::clone(&received);

        let unsub = provider
            .subscribe(
                "channel1",
                Box::new(move |_event| {
                    received_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        // Publish once, should be received
        provider
            .publish("channel1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();
        assert_eq!(received.load(Ordering::SeqCst), 1);

        // Unsubscribe
        unsub();

        // Publish again, should NOT be received
        provider
            .publish("channel1", make_event("e2", "t1", 1, 2000.0))
            .await
            .unwrap();
        assert_eq!(received.load(Ordering::SeqCst), 1);
    }

    // ─── MemoryBroadcastProvider: multiple subscribers ───────────────────

    #[tokio::test]
    async fn broadcast_multiple_subscribers_same_channel() {
        let provider = MemoryBroadcastProvider::new();
        let count1 = Arc::new(AtomicU64::new(0));
        let count2 = Arc::new(AtomicU64::new(0));
        let count1_clone = Arc::clone(&count1);
        let count2_clone = Arc::clone(&count2);

        let _unsub1 = provider
            .subscribe(
                "channel1",
                Box::new(move |_event| {
                    count1_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        let _unsub2 = provider
            .subscribe(
                "channel1",
                Box::new(move |_event| {
                    count2_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        provider
            .publish("channel1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();

        assert_eq!(count1.load(Ordering::SeqCst), 1);
        assert_eq!(count2.load(Ordering::SeqCst), 1);
    }

    // ─── MemoryBroadcastProvider: channels are independent ──────────────

    #[tokio::test]
    async fn broadcast_channels_are_independent() {
        let provider = MemoryBroadcastProvider::new();
        let count = Arc::new(AtomicU64::new(0));
        let count_clone = Arc::clone(&count);

        let _unsub = provider
            .subscribe(
                "channel1",
                Box::new(move |_event| {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        // Publish to different channel
        provider
            .publish("channel2", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    // ─── MemoryBroadcastProvider: unsubscribe only removes target ───────

    #[tokio::test]
    async fn broadcast_unsubscribe_only_removes_target_handler() {
        let provider = MemoryBroadcastProvider::new();
        let count1 = Arc::new(AtomicU64::new(0));
        let count2 = Arc::new(AtomicU64::new(0));
        let count1_clone = Arc::clone(&count1);
        let count2_clone = Arc::clone(&count2);

        let unsub1 = provider
            .subscribe(
                "channel1",
                Box::new(move |_event| {
                    count1_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        let _unsub2 = provider
            .subscribe(
                "channel1",
                Box::new(move |_event| {
                    count2_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .await;

        // Unsubscribe first handler only
        unsub1();

        provider
            .publish("channel1", make_event("e1", "t1", 0, 1000.0))
            .await
            .unwrap();

        assert_eq!(count1.load(Ordering::SeqCst), 0);
        assert_eq!(count2.load(Ordering::SeqCst), 1);
    }

    // ─── Default impls ───────────────────────────────────────────────

    #[test]
    fn memory_broadcast_provider_default_works() {
        let _provider: MemoryBroadcastProvider = Default::default();
    }

    #[test]
    fn memory_short_term_store_default_works() {
        let _store: MemoryShortTermStore = Default::default();
    }

    // ─── Helper: make_worker ────────────────────────────────────────────

    fn make_worker(id: &str) -> Worker {
        Worker {
            id: id.to_string(),
            status: WorkerStatus::Idle,
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            used_slots: 0,
            weight: 1,
            connection_mode: ConnectionMode::Pull,
            connected_at: 1000.0,
            last_heartbeat_at: 1000.0,
            metadata: None,
        }
    }

    fn make_assignment(task_id: &str, worker_id: &str) -> WorkerAssignment {
        WorkerAssignment {
            task_id: task_id.to_string(),
            worker_id: worker_id.to_string(),
            cost: 1,
            assigned_at: 1000.0,
            status: WorkerAssignmentStatus::Assigned,
        }
    }

    // ─── Worker CRUD ────────────────────────────────────────────────────

    #[tokio::test]
    async fn worker_save_and_get() {
        let store = MemoryShortTermStore::new();
        let worker = make_worker("w1");
        store.save_worker(worker.clone()).await.unwrap();

        let retrieved = store.get_worker("w1").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "w1");
        assert_eq!(retrieved.status, WorkerStatus::Idle);
        assert_eq!(retrieved.capacity, 5);
    }

    #[tokio::test]
    async fn worker_get_nonexistent_returns_none() {
        let store = MemoryShortTermStore::new();
        let result = store.get_worker("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn worker_save_overwrites_existing() {
        let store = MemoryShortTermStore::new();
        let worker = make_worker("w1");
        store.save_worker(worker).await.unwrap();

        let mut updated = make_worker("w1");
        updated.status = WorkerStatus::Busy;
        updated.used_slots = 3;
        store.save_worker(updated).await.unwrap();

        let retrieved = store.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(retrieved.status, WorkerStatus::Busy);
        assert_eq!(retrieved.used_slots, 3);
    }

    #[tokio::test]
    async fn worker_delete_removes_worker() {
        let store = MemoryShortTermStore::new();
        store.save_worker(make_worker("w1")).await.unwrap();
        store.delete_worker("w1").await.unwrap();

        let result = store.get_worker("w1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn worker_delete_nonexistent_is_noop() {
        let store = MemoryShortTermStore::new();
        let result = store.delete_worker("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn worker_list_returns_all() {
        let store = MemoryShortTermStore::new();
        store.save_worker(make_worker("w1")).await.unwrap();
        store.save_worker(make_worker("w2")).await.unwrap();
        store.save_worker(make_worker("w3")).await.unwrap();

        let workers = store.list_workers(None).await.unwrap();
        assert_eq!(workers.len(), 3);

        let mut ids: Vec<String> = workers.iter().map(|w| w.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["w1", "w2", "w3"]);
    }

    #[tokio::test]
    async fn worker_list_with_status_filter() {
        let store = MemoryShortTermStore::new();

        let mut w1 = make_worker("w1");
        w1.status = WorkerStatus::Idle;
        store.save_worker(w1).await.unwrap();

        let mut w2 = make_worker("w2");
        w2.status = WorkerStatus::Busy;
        store.save_worker(w2).await.unwrap();

        let mut w3 = make_worker("w3");
        w3.status = WorkerStatus::Draining;
        store.save_worker(w3).await.unwrap();

        // Filter for Idle only
        let filter = WorkerFilter {
            status: Some(vec![WorkerStatus::Idle]),
            connection_mode: None,
        };
        let workers = store.list_workers(Some(filter)).await.unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].id, "w1");

        // Filter for Idle and Busy
        let filter = WorkerFilter {
            status: Some(vec![WorkerStatus::Idle, WorkerStatus::Busy]),
            connection_mode: None,
        };
        let workers = store.list_workers(Some(filter)).await.unwrap();
        assert_eq!(workers.len(), 2);
    }

    #[tokio::test]
    async fn worker_list_with_connection_mode_filter() {
        let store = MemoryShortTermStore::new();

        let mut w1 = make_worker("w1");
        w1.connection_mode = ConnectionMode::Pull;
        store.save_worker(w1).await.unwrap();

        let mut w2 = make_worker("w2");
        w2.connection_mode = ConnectionMode::Websocket;
        store.save_worker(w2).await.unwrap();

        // Filter for Pull only
        let filter = WorkerFilter {
            status: None,
            connection_mode: Some(vec![ConnectionMode::Pull]),
        };
        let workers = store.list_workers(Some(filter)).await.unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].id, "w1");

        // Filter for Websocket only
        let filter = WorkerFilter {
            status: None,
            connection_mode: Some(vec![ConnectionMode::Websocket]),
        };
        let workers = store.list_workers(Some(filter)).await.unwrap();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].id, "w2");
    }

    // ─── claim_task ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn claim_task_succeeds_for_pending_task() {
        let store = MemoryShortTermStore::new();

        let mut task = make_task("t1");
        task.status = TaskStatus::Pending;
        store.save_task(task).await.unwrap();

        store.save_worker(make_worker("w1")).await.unwrap();

        let result = store.claim_task("t1", "w1", 1).await.unwrap();
        assert!(result);

        // Verify task is now Assigned
        let task = store.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Assigned);
        assert_eq!(task.assigned_worker, Some("w1".to_string()));
        assert_eq!(task.cost, Some(1));

        // Verify worker used_slots incremented
        let worker = store.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 1);
    }

    #[tokio::test]
    async fn claim_task_fails_when_worker_has_no_capacity() {
        let store = MemoryShortTermStore::new();

        let mut task = make_task("t1");
        task.status = TaskStatus::Pending;
        store.save_task(task).await.unwrap();

        let mut worker = make_worker("w1");
        worker.capacity = 2;
        worker.used_slots = 2; // already at capacity
        store.save_worker(worker).await.unwrap();

        let result = store.claim_task("t1", "w1", 1).await.unwrap();
        assert!(!result);

        // Task should remain Pending
        let task = store.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn claim_task_fails_for_non_pending_non_assigned_task() {
        let store = MemoryShortTermStore::new();

        // Task is Running, not Pending or Assigned
        let mut task = make_task("t1");
        task.status = TaskStatus::Running;
        store.save_task(task).await.unwrap();

        store.save_worker(make_worker("w1")).await.unwrap();

        let result = store.claim_task("t1", "w1", 1).await.unwrap();
        assert!(!result);

        // Worker used_slots should be rolled back to 0
        let worker = store.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 0);
    }

    #[tokio::test]
    async fn claim_task_rollback_restores_worker_slots() {
        let store = MemoryShortTermStore::new();

        // Task is Completed (invalid for claiming)
        let mut task = make_task("t1");
        task.status = TaskStatus::Completed;
        store.save_task(task).await.unwrap();

        let mut worker = make_worker("w1");
        worker.used_slots = 2;
        worker.capacity = 5;
        store.save_worker(worker).await.unwrap();

        let result = store.claim_task("t1", "w1", 1).await.unwrap();
        assert!(!result);

        // Worker used_slots should be rolled back to original value
        let worker = store.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 2);
    }

    #[tokio::test]
    async fn claim_task_fails_for_nonexistent_worker() {
        let store = MemoryShortTermStore::new();

        let mut task = make_task("t1");
        task.status = TaskStatus::Pending;
        store.save_task(task).await.unwrap();

        let result = store.claim_task("t1", "nonexistent", 1).await.unwrap();
        assert!(!result);

        // Task should remain Pending
        let task = store.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn claim_task_fails_for_nonexistent_task() {
        let store = MemoryShortTermStore::new();

        store.save_worker(make_worker("w1")).await.unwrap();

        let result = store.claim_task("nonexistent", "w1", 1).await.unwrap();
        assert!(!result);

        // Worker used_slots should be rolled back
        let worker = store.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 0);
    }

    // ─── Assignments ────────────────────────────────────────────────────

    #[tokio::test]
    async fn add_assignment_and_get_worker_assignments() {
        let store = MemoryShortTermStore::new();

        let a1 = make_assignment("t1", "w1");
        let a2 = make_assignment("t2", "w1");
        store.add_assignment(a1).await.unwrap();
        store.add_assignment(a2).await.unwrap();

        let assignments = store.get_worker_assignments("w1").await.unwrap();
        assert_eq!(assignments.len(), 2);

        let mut task_ids: Vec<String> = assignments.iter().map(|a| a.task_id.clone()).collect();
        task_ids.sort();
        assert_eq!(task_ids, vec!["t1", "t2"]);
    }

    #[tokio::test]
    async fn get_worker_assignments_returns_empty_for_unknown_worker() {
        let store = MemoryShortTermStore::new();
        let assignments = store.get_worker_assignments("unknown").await.unwrap();
        assert!(assignments.is_empty());
    }

    #[tokio::test]
    async fn remove_assignment_removes_by_task_id() {
        let store = MemoryShortTermStore::new();

        store
            .add_assignment(make_assignment("t1", "w1"))
            .await
            .unwrap();
        store
            .add_assignment(make_assignment("t2", "w1"))
            .await
            .unwrap();
        store
            .add_assignment(make_assignment("t3", "w2"))
            .await
            .unwrap();

        store.remove_assignment("t2").await.unwrap();

        // w1 should only have t1 left
        let w1_assignments = store.get_worker_assignments("w1").await.unwrap();
        assert_eq!(w1_assignments.len(), 1);
        assert_eq!(w1_assignments[0].task_id, "t1");

        // w2 should still have t3
        let w2_assignments = store.get_worker_assignments("w2").await.unwrap();
        assert_eq!(w2_assignments.len(), 1);
        assert_eq!(w2_assignments[0].task_id, "t3");
    }

    #[tokio::test]
    async fn remove_assignment_nonexistent_is_noop() {
        let store = MemoryShortTermStore::new();
        let result = store.remove_assignment("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_task_assignment_returns_assignment() {
        let store = MemoryShortTermStore::new();

        store
            .add_assignment(make_assignment("t1", "w1"))
            .await
            .unwrap();
        store
            .add_assignment(make_assignment("t2", "w2"))
            .await
            .unwrap();

        let assignment = store.get_task_assignment("t1").await.unwrap();
        assert!(assignment.is_some());
        let assignment = assignment.unwrap();
        assert_eq!(assignment.task_id, "t1");
        assert_eq!(assignment.worker_id, "w1");
    }

    #[tokio::test]
    async fn get_task_assignment_returns_none_for_unknown() {
        let store = MemoryShortTermStore::new();
        let result = store.get_task_assignment("unknown").await.unwrap();
        assert!(result.is_none());
    }

    // ─── list_tasks with filters ────────────────────────────────────────

    #[tokio::test]
    async fn list_tasks_filter_by_status() {
        let store = MemoryShortTermStore::new();

        let mut t1 = make_task("t1");
        t1.status = TaskStatus::Pending;
        store.save_task(t1).await.unwrap();

        let mut t2 = make_task("t2");
        t2.status = TaskStatus::Running;
        store.save_task(t2).await.unwrap();

        let mut t3 = make_task("t3");
        t3.status = TaskStatus::Completed;
        store.save_task(t3).await.unwrap();

        let filter = TaskFilter {
            status: Some(vec![TaskStatus::Pending, TaskStatus::Running]),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert_eq!(tasks.len(), 2);

        let mut ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["t1", "t2"]);
    }

    #[tokio::test]
    async fn list_tasks_filter_by_types() {
        let store = MemoryShortTermStore::new();

        let mut t1 = make_task("t1");
        t1.r#type = Some("llm".to_string());
        store.save_task(t1).await.unwrap();

        let mut t2 = make_task("t2");
        t2.r#type = Some("image".to_string());
        store.save_task(t2).await.unwrap();

        let mut t3 = make_task("t3");
        t3.r#type = None; // no type
        store.save_task(t3).await.unwrap();

        let filter = TaskFilter {
            types: Some(vec!["llm".to_string()]),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "t1");
    }

    #[tokio::test]
    async fn list_tasks_filter_by_types_excludes_tasks_with_no_type() {
        let store = MemoryShortTermStore::new();

        let mut t1 = make_task("t1");
        t1.r#type = None;
        store.save_task(t1).await.unwrap();

        let filter = TaskFilter {
            types: Some(vec!["llm".to_string()]),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn list_tasks_filter_by_assign_mode() {
        let store = MemoryShortTermStore::new();

        let mut t1 = make_task("t1");
        t1.assign_mode = Some(AssignMode::Pull);
        store.save_task(t1).await.unwrap();

        let mut t2 = make_task("t2");
        t2.assign_mode = Some(AssignMode::External);
        store.save_task(t2).await.unwrap();

        let mut t3 = make_task("t3");
        t3.assign_mode = None; // no assign_mode
        store.save_task(t3).await.unwrap();

        let filter = TaskFilter {
            assign_mode: Some(vec![AssignMode::Pull]),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "t1");
    }

    #[tokio::test]
    async fn list_tasks_filter_by_assign_mode_excludes_none() {
        let store = MemoryShortTermStore::new();

        let mut t1 = make_task("t1");
        t1.assign_mode = None;
        store.save_task(t1).await.unwrap();

        let filter = TaskFilter {
            assign_mode: Some(vec![AssignMode::Pull]),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn list_tasks_filter_by_tags() {
        let store = MemoryShortTermStore::new();

        let mut t1 = make_task("t1");
        t1.tags = Some(vec!["gpu".to_string(), "fast".to_string()]);
        store.save_task(t1).await.unwrap();

        let mut t2 = make_task("t2");
        t2.tags = Some(vec!["cpu".to_string()]);
        store.save_task(t2).await.unwrap();

        let mut t3 = make_task("t3");
        t3.tags = None;
        store.save_task(t3).await.unwrap();

        // Filter: must have "gpu" tag
        let filter = TaskFilter {
            tags: Some(TagMatcher {
                all: Some(vec!["gpu".to_string()]),
                any: None,
                none: None,
            }),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "t1");
    }

    #[tokio::test]
    async fn list_tasks_filter_by_exclude_task_ids() {
        let store = MemoryShortTermStore::new();

        store.save_task(make_task("t1")).await.unwrap();
        store.save_task(make_task("t2")).await.unwrap();
        store.save_task(make_task("t3")).await.unwrap();

        let filter = TaskFilter {
            exclude_task_ids: Some(vec!["t1".to_string(), "t3".to_string()]),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "t2");
    }

    #[tokio::test]
    async fn list_tasks_with_limit() {
        let store = MemoryShortTermStore::new();

        store.save_task(make_task("t1")).await.unwrap();
        store.save_task(make_task("t2")).await.unwrap();
        store.save_task(make_task("t3")).await.unwrap();

        let filter = TaskFilter {
            limit: Some(2),
            ..Default::default()
        };
        let tasks = store.list_tasks(filter).await.unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[tokio::test]
    async fn list_tasks_no_filter_returns_all() {
        let store = MemoryShortTermStore::new();

        store.save_task(make_task("t1")).await.unwrap();
        store.save_task(make_task("t2")).await.unwrap();

        let filter = TaskFilter::default();
        let tasks = store.list_tasks(filter).await.unwrap();
        assert_eq!(tasks.len(), 2);
    }
}
