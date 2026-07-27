use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::archive::{
    compute_archive_batch_digest, compute_archive_source_digest,
    compute_archive_source_page_digest, compute_series_state_digest,
};
use crate::types::{
    ArchiveBatch, ArchiveBatchReceipt, ArchiveGeneration, ArchiveGenerationStatus,
    ArchiveSourceManifest, DurableSeriesState, HotWriteToken, LongTermStore, RehydrateSnapshot,
    ReleasePreconditions, ReleaseResult, SeriesMode, ShortTermStore, StorageBusyError,
    StorageFenceConflictError, StorageIntegrityError, StorageLease, StoragePreconditionError,
    StorageReleaseUnsupportedError, StorageState, StorageUnavailableError, TaskEvent,
    TaskStorageMetadata, TaskStorageMetadataCas,
};

type StorageResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub type StorageLifecycleObserver = Arc<dyn Fn(&serde_json::Value) + Send + Sync>;

pub struct StorageCoordinator {
    short_term_store: Arc<dyn ShortTermStore>,
    long_term_store: Arc<dyn LongTermStore>,
    archive_batch_size: u64,
    storage_lock_ttl_ms: u64,
    rehydrate_replay_events: u64,
    required_storage_protocol_version: u64,
    id_generator: Arc<dyn Fn() -> String + Send + Sync>,
    observer: Option<StorageLifecycleObserver>,
}

struct SourceDescription {
    manifest: ArchiveSourceManifest,
    series_latest: Vec<DurableSeriesState>,
    max_event_timestamp: Option<f64>,
}

#[derive(Default)]
struct ReleaseProgress {
    fence_closed: AtomicBool,
    hot_deleted: AtomicBool,
    source_event_count: AtomicU64,
    source_bytes: AtomicU64,
}

struct RehydrateProgress {
    replay_event_count: AtomicU64,
    archive_watermark: AtomicI64,
    max_event_index: AtomicI64,
    storage_epoch: AtomicU64,
}

struct ArchiveUpload<'a> {
    target_watermark: i64,
    prior_watermark: i64,
    manifest: &'a ArchiveSourceManifest,
    progress: &'a ReleaseProgress,
}

impl RehydrateProgress {
    fn new(initial: &TaskStorageMetadata) -> Self {
        Self {
            replay_event_count: AtomicU64::new(0),
            archive_watermark: AtomicI64::new(initial.archive_watermark),
            max_event_index: AtomicI64::new(-1),
            storage_epoch: AtomicU64::new(initial.storage_epoch),
        }
    }
}

impl StorageCoordinator {
    pub fn new(
        short_term_store: Arc<dyn ShortTermStore>,
        long_term_store: Arc<dyn LongTermStore>,
    ) -> Self {
        Self {
            short_term_store,
            long_term_store,
            archive_batch_size: 1_000,
            storage_lock_ttl_ms: 30_000,
            rehydrate_replay_events: 1_000,
            required_storage_protocol_version: 2,
            id_generator: Arc::new(|| ulid::Ulid::new().to_string()),
            observer: None,
        }
    }

    pub fn with_archive_batch_size(mut self, archive_batch_size: u64) -> Self {
        assert!(
            archive_batch_size > 0,
            "archive batch size must be positive"
        );
        self.archive_batch_size = archive_batch_size;
        self
    }

    pub fn with_storage_lock_ttl_ms(mut self, storage_lock_ttl_ms: u64) -> Self {
        assert!(storage_lock_ttl_ms > 0, "storage lock TTL must be positive");
        self.storage_lock_ttl_ms = storage_lock_ttl_ms;
        self
    }

    pub fn with_rehydrate_replay_events(mut self, rehydrate_replay_events: u64) -> Self {
        self.rehydrate_replay_events = rehydrate_replay_events;
        self
    }

    pub fn with_required_storage_protocol_version(
        mut self,
        required_storage_protocol_version: u64,
    ) -> Self {
        self.required_storage_protocol_version = required_storage_protocol_version;
        self
    }

    pub fn with_id_generator<F>(mut self, id_generator: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        self.id_generator = Arc::new(id_generator);
        self
    }

    pub fn with_observer(mut self, observer: StorageLifecycleObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    pub async fn ensure_task_hot_for_write(&self, task_id: &str) -> StorageResult<HotWriteToken> {
        self.ensure_task_hot_for_write_mode(task_id, true).await
    }

    pub(crate) async fn ensure_task_hot_for_write_without_rehydrate(
        &self,
        task_id: &str,
    ) -> StorageResult<HotWriteToken> {
        self.ensure_task_hot_for_write_mode(task_id, false).await
    }

    async fn ensure_task_hot_for_write_mode(
        &self,
        task_id: &str,
        rehydrate_cold: bool,
    ) -> StorageResult<HotWriteToken> {
        self.require_capabilities()?;
        for _attempt in 0..3 {
            let metadata = self
                .long_term_store
                .get_task_storage_metadata(task_id)
                .await?
                .ok_or_else(|| {
                    boxed(StorageIntegrityError::new(format!(
                        "Task storage metadata does not exist: {task_id}"
                    )))
                })?;
            match metadata.storage_state {
                StorageState::Releasing => {
                    return Err(boxed(StorageBusyError::new(
                        "Task storage lifecycle operation is in progress",
                    )))
                }
                StorageState::Cold => {
                    if !rehydrate_cold {
                        return Err(boxed(StorageBusyError::new(
                            "Task became cold after the write mutation started",
                        )));
                    }
                    return self.rehydrate_cold_task(task_id, metadata).await;
                }
                StorageState::Hot => {}
            }
            let fence = self
                .short_term_store
                .get_write_fence(task_id)
                .await?
                .ok_or_else(|| boxed(StorageFenceConflictError::default()))?;
            if fence.accepting_writes
                && fence.active_release_generation.is_none()
                && fence.storage_epoch == metadata.storage_epoch
            {
                return Ok(HotWriteToken {
                    task_id: task_id.to_string(),
                    storage_epoch: fence.storage_epoch,
                });
            }
            if fence.accepting_writes
                && fence.active_release_generation.is_none()
                && metadata.active_release_generation.is_none()
                && fence.storage_epoch > metadata.storage_epoch
            {
                let repaired = self
                    .long_term_store
                    .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                        task_id: task_id.to_string(),
                        expected_storage_state: StorageState::Hot,
                        expected_storage_epoch: metadata.storage_epoch,
                        expected_release_generation: None,
                        next: TaskStorageMetadata {
                            storage_epoch: fence.storage_epoch,
                            ..metadata
                        },
                    })
                    .await?;
                if repaired {
                    return Ok(HotWriteToken {
                        task_id: task_id.to_string(),
                        storage_epoch: fence.storage_epoch,
                    });
                }
                continue;
            }
            return Err(boxed(StorageFenceConflictError::new(
                "Hot task write fence does not match durable metadata",
            )));
        }
        Err(boxed(StorageFenceConflictError::new(
            "Hot task write fence repair lost its metadata race",
        )))
    }

    async fn rehydrate_cold_task(
        &self,
        task_id: &str,
        initial: TaskStorageMetadata,
    ) -> StorageResult<HotWriteToken> {
        let started_at = now_millis();
        let progress = RehydrateProgress::new(&initial);
        let lease = self
            .short_term_store
            .acquire_storage_lock(
                task_id,
                &(self.id_generator)(),
                &(self.id_generator)(),
                self.storage_lock_ttl_ms,
            )
            .await?
            .ok_or_else(|| {
                boxed(StorageBusyError::new(
                    "Task storage rehydration is already in progress",
                ))
            })?;
        let lease_lost = AtomicBool::new(false);
        let result = self
            .rehydrate_cold_task_owned(task_id, initial, &lease, &lease_lost, &progress)
            .await;
        if !lease_lost.load(Ordering::SeqCst) {
            let _ = self.short_term_store.release_storage_lock(&lease).await;
        }
        match &result {
            Ok(token) => self.observe(serde_json::json!({
                "event": "storage_rehydrate",
                "taskId": task_id,
                "outcome": "rehydrated",
                "durationMs": (now_millis() - started_at).max(0.0),
                "replayEventCount": progress.replay_event_count.load(Ordering::SeqCst),
                "archiveWatermark": progress.archive_watermark.load(Ordering::SeqCst),
                "maxEventIndex": progress.max_event_index.load(Ordering::SeqCst),
                "storageEpoch": token.storage_epoch,
                "storageStateBefore": "cold",
                "storageStateAfter": "hot",
            })),
            Err(error) => self.observe(serde_json::json!({
                "event": "storage_rehydrate",
                "taskId": task_id,
                "outcome": "failed",
                "durationMs": (now_millis() - started_at).max(0.0),
                "replayEventCount": progress.replay_event_count.load(Ordering::SeqCst),
                "archiveWatermark": progress.archive_watermark.load(Ordering::SeqCst),
                "maxEventIndex": progress.max_event_index.load(Ordering::SeqCst),
                "storageEpoch": progress.storage_epoch.load(Ordering::SeqCst),
                "storageStateBefore": "cold",
                "storageStateAfter": "cold",
                "errorCode": Self::error_code(error.as_ref()),
                "error": error.to_string(),
            })),
        }
        result
    }

    async fn rehydrate_cold_task_owned(
        &self,
        task_id: &str,
        initial: TaskStorageMetadata,
        lease: &StorageLease,
        lease_lost: &AtomicBool,
        progress: &RehydrateProgress,
    ) -> StorageResult<HotWriteToken> {
        self.renew(lease, lease_lost).await?;
        let metadata = self
            .long_term_store
            .get_task_storage_metadata(task_id)
            .await?
            .ok_or_else(|| {
                boxed(StorageIntegrityError::new(format!(
                    "Task storage metadata does not exist: {task_id}"
                )))
            })?;
        match metadata.storage_state {
            StorageState::Releasing => {
                return Err(boxed(StorageBusyError::new(
                    "Task storage lifecycle operation is in progress",
                )))
            }
            StorageState::Hot => {
                let fence = self.short_term_store.get_write_fence(task_id).await?;
                if fence.as_ref().is_some_and(|fence| {
                    fence.accepting_writes
                        && fence.active_release_generation.is_none()
                        && fence.storage_epoch == metadata.storage_epoch
                }) {
                    return Ok(HotWriteToken {
                        task_id: task_id.to_string(),
                        storage_epoch: metadata.storage_epoch,
                    });
                }
                return Err(boxed(StorageFenceConflictError::new(
                    "Hot task write fence does not match durable metadata",
                )));
            }
            StorageState::Cold => {}
        }
        if metadata.storage_epoch != initial.storage_epoch
            || metadata.active_release_generation.is_some()
        {
            return Err(boxed(StorageFenceConflictError::new(
                "Cold task metadata changed before rehydration",
            )));
        }

        let (presence, existing_fence) = tokio::try_join!(
            self.short_term_store.get_task_storage_presence(task_id),
            self.short_term_store.get_write_fence(task_id),
        )?;
        if presence.task
            && existing_fence.as_ref().is_some_and(|fence| {
                fence.accepting_writes
                    && fence.active_release_generation.is_none()
                    && fence.storage_epoch > metadata.storage_epoch
            })
        {
            let storage_epoch = existing_fence.unwrap().storage_epoch;
            let adopted = self
                .long_term_store
                .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                    task_id: task_id.to_string(),
                    expected_storage_state: StorageState::Cold,
                    expected_storage_epoch: metadata.storage_epoch,
                    expected_release_generation: None,
                    next: TaskStorageMetadata {
                        storage_state: StorageState::Hot,
                        storage_epoch,
                        cold_at: None,
                        ..metadata
                    },
                })
                .await?;
            if !adopted {
                return Err(boxed(StorageFenceConflictError::new(
                    "Restored hot epoch lost its metadata recovery race",
                )));
            }
            return Ok(HotWriteToken {
                task_id: task_id.to_string(),
                storage_epoch,
            });
        }
        if presence.task
            || presence.event_count != 0
            || presence.next_index
            || presence.series_state_count != 0
            || presence.write_fence
        {
            return Err(boxed(StorageIntegrityError::new(
                "Cold task has partial or stale hot storage",
            )));
        }

        self.renew(lease, lease_lost).await?;
        let (task, max_event_index, replay_events, series_latest) = tokio::try_join!(
            self.long_term_store.get_task(task_id),
            self.long_term_store.get_last_event_index(task_id),
            self.long_term_store
                .get_recent_events(task_id, self.rehydrate_replay_events),
            self.long_term_store.get_durable_series_state(task_id),
        )?;
        progress
            .replay_event_count
            .store(replay_events.len() as u64, Ordering::SeqCst);
        progress
            .archive_watermark
            .store(metadata.archive_watermark, Ordering::SeqCst);
        progress
            .max_event_index
            .store(max_event_index, Ordering::SeqCst);
        progress
            .storage_epoch
            .store(metadata.storage_epoch, Ordering::SeqCst);
        let task = task.ok_or_else(|| {
            boxed(StorageIntegrityError::new(format!(
                "Durable task does not exist: {task_id}"
            )))
        })?;
        if max_event_index < metadata.archive_watermark
            || replay_events.iter().any(|event| {
                event.task_id != task_id
                    || i64::try_from(event.index)
                        .map(|index| index > max_event_index)
                        .unwrap_or(true)
            })
        {
            self.observe(serde_json::json!({
                "event": "storage_watermark_mismatch",
                "operation": "rehydrate",
                "taskId": task_id,
                "expectedWatermark": metadata.archive_watermark,
                "actualWatermark": max_event_index,
                "replayEventCount": replay_events.len(),
                "storageEpoch": metadata.storage_epoch,
            }));
            return Err(boxed(StorageIntegrityError::new(
                "Durable rehydrate snapshot is inconsistent",
            )));
        }
        let next_epoch = metadata.storage_epoch.checked_add(1).ok_or_else(|| {
            boxed(StorageIntegrityError::new(
                "Task storage epoch exceeds safe bounds",
            ))
        })?;
        self.renew(lease, lease_lost).await?;
        let token = self
            .short_term_store
            .restore_hot_task_fenced(
                RehydrateSnapshot {
                    task,
                    archive_watermark: metadata.archive_watermark,
                    max_event_index,
                    replay_events,
                    series_latest,
                    storage_epoch: metadata.storage_epoch,
                },
                lease,
                next_epoch,
            )
            .await?;
        self.renew(lease, lease_lost).await?;
        let installed = self
            .long_term_store
            .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                task_id: task_id.to_string(),
                expected_storage_state: StorageState::Cold,
                expected_storage_epoch: metadata.storage_epoch,
                expected_release_generation: None,
                next: TaskStorageMetadata {
                    storage_state: StorageState::Hot,
                    storage_epoch: next_epoch,
                    active_release_generation: None,
                    cold_at: None,
                    ..metadata.clone()
                },
            })
            .await?;
        if installed {
            return Ok(token);
        }
        let current = self
            .long_term_store
            .get_task_storage_metadata(task_id)
            .await?;
        if current.as_ref().is_some_and(|current| {
            current.storage_state == StorageState::Hot
                && current.storage_epoch == next_epoch
                && current.active_release_generation.is_none()
        }) {
            return Ok(token);
        }
        self.renew(lease, lease_lost).await?;
        self.short_term_store
            .close_write_fence(lease, next_epoch)
            .await?;
        self.short_term_store
            .delete_task_storage_fenced(lease, next_epoch)
            .await?;
        Err(boxed(StorageFenceConflictError::new(
            "Restored hot epoch lost its durable metadata race",
        )))
    }

    pub async fn release_task_storage(
        &self,
        task_id: &str,
        preconditions: ReleasePreconditions,
    ) -> StorageResult<ReleaseResult> {
        let started_at = now_millis();
        let progress = ReleaseProgress::default();
        let result = self
            .release_task_storage_inner(task_id, preconditions, &progress)
            .await;
        match &result {
            Ok(release) => self.observe(serde_json::json!({
                "event": "storage_release",
                "taskId": task_id,
                "outcome": if release.released { "released" } else { "noop" },
                "durationMs": (now_millis() - started_at).max(0.0),
                "sourceEventCount": progress.source_event_count.load(Ordering::SeqCst),
                "sourceBytes": progress.source_bytes.load(Ordering::SeqCst),
                "storageStateBefore": if release.released { "hot" } else { "cold" },
                "storageStateAfter": release.storage_state,
                "archiveWatermark": release.archive_watermark,
            })),
            Err(error) => self.observe(serde_json::json!({
                "event": "storage_release",
                "taskId": task_id,
                "outcome": "failed",
                "durationMs": (now_millis() - started_at).max(0.0),
                "sourceEventCount": progress.source_event_count.load(Ordering::SeqCst),
                "sourceBytes": progress.source_bytes.load(Ordering::SeqCst),
                "storageStateBefore": "hot",
                "storageStateAfter": "hot",
                "errorCode": Self::error_code(error.as_ref()),
                "error": error.to_string(),
            })),
        }
        result
    }

    async fn release_task_storage_inner(
        &self,
        task_id: &str,
        preconditions: ReleasePreconditions,
        progress: &ReleaseProgress,
    ) -> StorageResult<ReleaseResult> {
        self.require_capabilities()?;
        let metadata = self
            .long_term_store
            .get_task_storage_metadata(task_id)
            .await?
            .ok_or_else(|| {
                boxed(StorageIntegrityError::new(format!(
                    "Task storage metadata does not exist: {task_id}"
                )))
            })?;
        if metadata.storage_state == StorageState::Cold {
            return Ok(ReleaseResult {
                task_id: task_id.to_string(),
                storage_state: StorageState::Cold,
                archive_watermark: metadata.archive_watermark,
                released: false,
            });
        }
        if metadata.storage_state != StorageState::Hot {
            return Err(boxed(StorageBusyError::new(
                "Task storage is already being released",
            )));
        }

        let generation = (self.id_generator)();
        let lock_token = (self.id_generator)();
        let lease = self
            .short_term_store
            .acquire_storage_lock(task_id, &lock_token, &generation, self.storage_lock_ttl_ms)
            .await?
            .ok_or_else(|| boxed(StorageBusyError::default()))?;
        let lease_lost = AtomicBool::new(false);
        let result = self
            .release_owned(
                task_id,
                preconditions,
                metadata,
                &lease,
                &lease_lost,
                progress,
            )
            .await;
        if result.is_err()
            && !lease_lost.load(Ordering::SeqCst)
            && progress.fence_closed.load(Ordering::SeqCst)
            && !progress.hot_deleted.load(Ordering::SeqCst)
        {
            self.reopen_after_failure(task_id, &lease).await;
        }
        if !lease_lost.load(Ordering::SeqCst) {
            let _ = self.short_term_store.release_storage_lock(&lease).await;
        }
        result
    }

    pub async fn recover_task_storage(&self, task_id: &str) -> StorageResult<ReleaseResult> {
        self.require_capabilities()?;
        let initial = self
            .long_term_store
            .get_task_storage_metadata(task_id)
            .await?
            .ok_or_else(|| {
                boxed(StorageIntegrityError::new(format!(
                    "Task storage metadata does not exist: {task_id}"
                )))
            })?;
        if initial.storage_state != StorageState::Releasing {
            return Ok(ReleaseResult {
                task_id: task_id.to_string(),
                storage_state: initial.storage_state,
                archive_watermark: initial.archive_watermark,
                released: false,
            });
        }
        let generation = (self.id_generator)();
        let lock_token = (self.id_generator)();
        let lease = self
            .short_term_store
            .acquire_storage_lock(task_id, &lock_token, &generation, self.storage_lock_ttl_ms)
            .await?
            .ok_or_else(|| boxed(StorageBusyError::default()))?;
        let lease_lost = AtomicBool::new(false);
        let result = self
            .recover_owned(task_id, initial, &lease, &lease_lost)
            .await;
        if !lease_lost.load(Ordering::SeqCst) {
            let _ = self.short_term_store.release_storage_lock(&lease).await;
        }
        result
    }

    async fn recover_owned(
        &self,
        task_id: &str,
        initial: TaskStorageMetadata,
        lease: &StorageLease,
        lease_lost: &AtomicBool,
    ) -> StorageResult<ReleaseResult> {
        self.renew(lease, lease_lost).await?;
        let presence = self
            .short_term_store
            .get_task_storage_presence(task_id)
            .await?;
        let fence = self.short_term_store.get_write_fence(task_id).await?;
        if initial.storage_state == StorageState::Releasing
            && presence.task
            && matches!(
                fence,
                Some(ref fence)
                    if fence.accepting_writes
                        && fence.active_release_generation.is_none()
                        && fence.storage_epoch > initial.storage_epoch
            )
        {
            let fence = fence.expect("matched present write fence");
            let reopened = TaskStorageMetadata {
                storage_state: StorageState::Hot,
                storage_epoch: fence.storage_epoch,
                active_release_generation: None,
                cold_at: None,
                ..initial.clone()
            };
            self.renew(lease, lease_lost).await?;
            let repaired = self
                .long_term_store
                .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                    task_id: task_id.to_string(),
                    expected_storage_state: StorageState::Releasing,
                    expected_storage_epoch: initial.storage_epoch,
                    expected_release_generation: initial.active_release_generation,
                    next: reopened.clone(),
                })
                .await?;
            if !repaired {
                return Err(boxed(StorageFenceConflictError::new(
                    "Recovered hot epoch lost its metadata race",
                )));
            }
            return Ok(ReleaseResult {
                task_id: task_id.to_string(),
                storage_state: StorageState::Hot,
                archive_watermark: reopened.archive_watermark,
                released: false,
            });
        }
        if !presence.task
            && !presence.write_fence
            && presence.event_count == 0
            && !presence.next_index
            && presence.series_state_count == 0
        {
            let watermark = self.long_term_store.get_archive_watermark(task_id).await?;
            let durable_last_index = self.long_term_store.get_last_event_index(task_id).await?;
            if watermark < durable_last_index {
                return Err(boxed(StorageIntegrityError::new(
                    "Missing hot storage is not covered by the durable watermark",
                )));
            }
            let adopted = TaskStorageMetadata {
                active_release_generation: Some(lease.generation.clone()),
                archive_watermark: watermark,
                ..initial.clone()
            };
            self.renew(lease, lease_lost).await?;
            let installed = self
                .long_term_store
                .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                    task_id: task_id.to_string(),
                    expected_storage_state: StorageState::Releasing,
                    expected_storage_epoch: initial.storage_epoch,
                    expected_release_generation: initial.active_release_generation,
                    next: adopted.clone(),
                })
                .await?;
            if !installed {
                return Err(boxed(StorageFenceConflictError::new(
                    "Storage recovery generation was not installed",
                )));
            }
            let cold = TaskStorageMetadata {
                storage_state: StorageState::Cold,
                active_release_generation: None,
                cold_at: Some(now_millis()),
                ..adopted.clone()
            };
            self.renew(lease, lease_lost).await?;
            let committed = self
                .long_term_store
                .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                    task_id: task_id.to_string(),
                    expected_storage_state: StorageState::Releasing,
                    expected_storage_epoch: adopted.storage_epoch,
                    expected_release_generation: Some(lease.generation.clone()),
                    next: cold,
                })
                .await?;
            if !committed {
                return Err(boxed(StorageFenceConflictError::new(
                    "Recovered cold transition lost its generation",
                )));
            }
            return Ok(ReleaseResult {
                task_id: task_id.to_string(),
                storage_state: StorageState::Cold,
                archive_watermark: watermark,
                released: true,
            });
        }
        if !presence.task {
            return Err(boxed(StorageIntegrityError::new(
                "Retained hot storage is missing its task record",
            )));
        }

        self.renew(lease, lease_lost).await?;
        let closed = self
            .short_term_store
            .close_write_fence(lease, initial.storage_epoch)
            .await?;
        let adopted = TaskStorageMetadata {
            active_release_generation: Some(lease.generation.clone()),
            ..initial.clone()
        };
        self.renew(lease, lease_lost).await?;
        let installed = self
            .long_term_store
            .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                task_id: task_id.to_string(),
                expected_storage_state: StorageState::Releasing,
                expected_storage_epoch: initial.storage_epoch,
                expected_release_generation: initial.active_release_generation,
                next: adopted.clone(),
            })
            .await?;
        if !installed {
            return Err(boxed(StorageFenceConflictError::new(
                "Storage recovery generation was not installed",
            )));
        }
        let watermark = self.long_term_store.get_archive_watermark(task_id).await?;
        if watermark >= closed.high_watermark {
            self.renew(lease, lease_lost).await?;
            self.short_term_store
                .delete_task_storage_fenced(lease, adopted.storage_epoch)
                .await?;
            self.renew(lease, lease_lost).await?;
            let cold = TaskStorageMetadata {
                storage_state: StorageState::Cold,
                active_release_generation: None,
                archive_watermark: watermark,
                cold_at: Some(now_millis()),
                ..adopted.clone()
            };
            let committed = self
                .long_term_store
                .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                    task_id: task_id.to_string(),
                    expected_storage_state: StorageState::Releasing,
                    expected_storage_epoch: adopted.storage_epoch,
                    expected_release_generation: Some(lease.generation.clone()),
                    next: cold,
                })
                .await?;
            if !committed {
                return Err(boxed(StorageFenceConflictError::new(
                    "Recovered cold transition lost its generation",
                )));
            }
            return Ok(ReleaseResult {
                task_id: task_id.to_string(),
                storage_state: StorageState::Cold,
                archive_watermark: watermark,
                released: true,
            });
        }

        self.renew(lease, lease_lost).await?;
        let token = self
            .short_term_store
            .reopen_write_fence(lease, adopted.storage_epoch)
            .await?;
        let reopened = TaskStorageMetadata {
            storage_state: StorageState::Hot,
            storage_epoch: token.storage_epoch,
            active_release_generation: None,
            cold_at: None,
            ..adopted.clone()
        };
        let committed = self
            .long_term_store
            .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                task_id: task_id.to_string(),
                expected_storage_state: StorageState::Releasing,
                expected_storage_epoch: adopted.storage_epoch,
                expected_release_generation: Some(lease.generation.clone()),
                next: reopened.clone(),
            })
            .await?;
        if !committed {
            return Err(boxed(StorageFenceConflictError::new(
                "Recovered hot transition lost its generation",
            )));
        }
        Ok(ReleaseResult {
            task_id: task_id.to_string(),
            storage_state: StorageState::Hot,
            archive_watermark: reopened.archive_watermark,
            released: false,
        })
    }

    async fn release_owned(
        &self,
        task_id: &str,
        preconditions: ReleasePreconditions,
        mut metadata: TaskStorageMetadata,
        lease: &StorageLease,
        lease_lost: &AtomicBool,
        progress: &ReleaseProgress,
    ) -> StorageResult<ReleaseResult> {
        let writers = self.short_term_store.list_storage_writers().await?;
        let incompatible = writers
            .iter()
            .filter(|writer| {
                writer.storage_protocol_version < self.required_storage_protocol_version
            })
            .map(|writer| writer.instance_id.as_str())
            .collect::<Vec<_>>();
        if !incompatible.is_empty() {
            return Err(boxed(StorageUnavailableError::new(format!(
                "Storage release is blocked by incompatible writers: {}",
                incompatible.join(", ")
            ))));
        }

        self.renew(lease, lease_lost).await?;
        let closed = self
            .short_term_store
            .close_write_fence(lease, metadata.storage_epoch)
            .await?;
        progress.fence_closed.store(true, Ordering::SeqCst);
        if closed.high_watermark != preconditions.expected_last_event_index {
            return Err(boxed(StoragePreconditionError::new(
                "Task event index changed before storage release",
            )));
        }
        if !preconditions.inactive_since.is_finite()
            || metadata
                .last_event_at
                .is_some_and(|last_event_at| last_event_at > preconditions.inactive_since)
        {
            return Err(boxed(StoragePreconditionError::new(
                "Task has activity newer than the release cutoff",
            )));
        }

        let releasing = TaskStorageMetadata {
            storage_state: StorageState::Releasing,
            active_release_generation: Some(lease.generation.clone()),
            cold_at: None,
            ..metadata.clone()
        };
        self.renew(lease, lease_lost).await?;
        let installed = self
            .long_term_store
            .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                task_id: task_id.to_string(),
                expected_storage_state: StorageState::Hot,
                expected_storage_epoch: metadata.storage_epoch,
                expected_release_generation: None,
                next: releasing.clone(),
            })
            .await?;
        if !installed {
            return Err(boxed(StorageBusyError::new(
                "Task storage metadata changed before release",
            )));
        }
        metadata = releasing;

        let description = self
            .describe_archive_source(
                task_id,
                closed.high_watermark,
                metadata.archive_watermark,
                lease,
                lease_lost,
            )
            .await?;
        progress
            .source_event_count
            .store(description.manifest.source_entry_count, Ordering::SeqCst);
        if description
            .max_event_timestamp
            .is_some_and(|timestamp| timestamp > preconditions.inactive_since)
        {
            return Err(boxed(StoragePreconditionError::new(
                "Task source has activity newer than the release cutoff",
            )));
        }
        let now = now_millis();
        let archive = ArchiveGeneration {
            task_id: task_id.to_string(),
            generation: lease.generation.clone(),
            storage_epoch: metadata.storage_epoch,
            target_watermark: closed.high_watermark,
            manifest: description.manifest.clone(),
            status: ArchiveGenerationStatus::Open,
            created_at: now,
            updated_at: now,
        };
        self.renew(lease, lease_lost).await?;
        self.long_term_store.begin_archive(archive).await?;
        self.upload_archive_batches(
            task_id,
            lease,
            lease_lost,
            ArchiveUpload {
                target_watermark: closed.high_watermark,
                prior_watermark: metadata.archive_watermark,
                manifest: &description.manifest,
                progress,
            },
        )
        .await?;

        let task = self
            .short_term_store
            .get_task(task_id)
            .await?
            .ok_or_else(|| {
                boxed(StorageIntegrityError::new(
                    "Hot task disappeared during release",
                ))
            })?;
        self.renew(lease, lease_lost).await?;
        self.long_term_store
            .finalize_archive(task_id, &lease.generation, task, description.series_latest)
            .await?;
        self.renew(lease, lease_lost).await?;
        let watermark = self.long_term_store.get_archive_watermark(task_id).await?;
        let current = self
            .long_term_store
            .get_task_storage_metadata(task_id)
            .await?
            .ok_or_else(|| boxed(StorageIntegrityError::default()))?;
        if watermark < closed.high_watermark
            || current.storage_state != StorageState::Releasing
            || current.storage_epoch != metadata.storage_epoch
            || current.active_release_generation.as_deref() != Some(lease.generation.as_str())
        {
            self.observe(serde_json::json!({
                "event": "storage_watermark_mismatch",
                "operation": "release",
                "taskId": task_id,
                "expectedWatermark": closed.high_watermark,
                "actualWatermark": watermark,
                "storageState": current.storage_state,
                "storageEpoch": current.storage_epoch,
            }));
            return Err(boxed(StorageIntegrityError::new(
                "Durable archive read-back did not prove release",
            )));
        }

        self.renew(lease, lease_lost).await?;
        self.short_term_store
            .delete_task_storage_fenced(lease, metadata.storage_epoch)
            .await?;
        progress.hot_deleted.store(true, Ordering::SeqCst);
        self.renew(lease, lease_lost).await?;
        let cold = TaskStorageMetadata {
            storage_state: StorageState::Cold,
            active_release_generation: None,
            archive_watermark: watermark,
            cold_at: Some(now_millis()),
            ..current.clone()
        };
        let committed = self
            .long_term_store
            .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                task_id: task_id.to_string(),
                expected_storage_state: StorageState::Releasing,
                expected_storage_epoch: current.storage_epoch,
                expected_release_generation: Some(lease.generation.clone()),
                next: cold,
            })
            .await?;
        if !committed {
            return Err(boxed(StorageFenceConflictError::new(
                "Task storage cold transition lost its fence",
            )));
        }
        Ok(ReleaseResult {
            task_id: task_id.to_string(),
            storage_state: StorageState::Cold,
            archive_watermark: watermark,
            released: true,
        })
    }

    async fn describe_archive_source(
        &self,
        task_id: &str,
        target_watermark: i64,
        prior_watermark: i64,
        lease: &StorageLease,
        lease_lost: &AtomicBool,
    ) -> StorageResult<SourceDescription> {
        let mut cursor: Option<String> = None;
        let mut previous_index: Option<u64> = None;
        let mut source_entry_count = 0_u64;
        let mut page_digests = Vec::new();
        let mut batch = Vec::with_capacity(self.archive_batch_size as usize);
        let mut series_modes = BTreeMap::<String, SeriesMode>::new();
        let mut max_event_timestamp: Option<f64> = None;

        loop {
            let page = self
                .short_term_store
                .read_archive_source_page(
                    task_id,
                    target_watermark,
                    cursor.as_deref(),
                    self.archive_batch_size,
                )
                .await?;
            for event in page.events {
                if event.task_id != task_id
                    || previous_index.is_some_and(|previous| event.index <= previous)
                {
                    return Err(boxed(StorageIntegrityError::new(
                        "Archive source is not strictly ordered",
                    )));
                }
                if !event.timestamp.is_finite() {
                    return Err(boxed(StorageIntegrityError::new(
                        "Archive source contains an invalid event timestamp",
                    )));
                }
                max_event_timestamp = Some(
                    max_event_timestamp
                        .map(|current| current.max(event.timestamp))
                        .unwrap_or(event.timestamp),
                );
                previous_index = Some(event.index);
                if let (Some(series_id), Some(mode)) =
                    (event.series_id.as_ref(), event.series_mode.as_ref())
                {
                    if matches!(mode, SeriesMode::Latest | SeriesMode::Accumulate) {
                        if let Some(existing) = series_modes.get(series_id) {
                            if existing != mode {
                                return Err(boxed(StorageIntegrityError::new(format!(
                                    "Series mode changed for {series_id}"
                                ))));
                            }
                        }
                        series_modes.insert(series_id.clone(), mode.clone());
                    }
                }
                if event.index as i64 <= prior_watermark {
                    continue;
                }
                if event.index as i64 > target_watermark {
                    return Err(boxed(StorageIntegrityError::new(
                        "Archive source exceeds its closed watermark",
                    )));
                }
                batch.push(event);
                if batch.len() == self.archive_batch_size as usize {
                    self.renew(lease, lease_lost).await?;
                    page_digests.push(compute_archive_source_page_digest(&batch)?);
                    source_entry_count += batch.len() as u64;
                    batch.clear();
                }
            }
            if page.done {
                break;
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Err(boxed(StorageIntegrityError::new(
                    "Archive source page omitted its next cursor",
                )));
            }
        }
        if !batch.is_empty() {
            self.renew(lease, lease_lost).await?;
            page_digests.push(compute_archive_source_page_digest(&batch)?);
            source_entry_count += batch.len() as u64;
        }

        let mut series_latest = Vec::with_capacity(series_modes.len());
        for (series_id, mode) in series_modes {
            let event = self
                .short_term_store
                .get_series_latest(task_id, &series_id)
                .await?
                .ok_or_else(|| {
                    boxed(StorageIntegrityError::new(format!(
                        "Series state is missing: {series_id}"
                    )))
                })?;
            if event.index as i64 > target_watermark {
                return Err(boxed(StorageIntegrityError::new(format!(
                    "Series state exceeds release watermark: {series_id}"
                ))));
            }
            series_latest.push(DurableSeriesState {
                task_id: task_id.to_string(),
                series_id,
                mode,
                through_index: event.index,
                event,
            });
        }
        let expected_batch_ordinals = (0..page_digests.len() as u64).collect();
        Ok(SourceDescription {
            manifest: ArchiveSourceManifest {
                prior_watermark,
                target_watermark,
                source_entry_count,
                source_digest: compute_archive_source_digest(&page_digests),
                series_state_digest: compute_series_state_digest(&series_latest)?,
                expected_batch_ordinals,
            },
            series_latest,
            max_event_timestamp,
        })
    }

    async fn upload_archive_batches(
        &self,
        task_id: &str,
        lease: &StorageLease,
        lease_lost: &AtomicBool,
        upload: ArchiveUpload<'_>,
    ) -> StorageResult<()> {
        let mut cursor: Option<String> = None;
        let mut previous_index: Option<u64> = None;
        let mut previous_batch_digest: Option<String> = None;
        let mut ordinal = 0_u64;
        let mut batch = Vec::with_capacity(self.archive_batch_size as usize);
        loop {
            let page = self
                .short_term_store
                .read_archive_source_page(
                    task_id,
                    upload.target_watermark,
                    cursor.as_deref(),
                    self.archive_batch_size,
                )
                .await?;
            for event in page.events {
                if event.task_id != task_id
                    || previous_index.is_some_and(|previous| event.index <= previous)
                {
                    return Err(boxed(StorageIntegrityError::new(
                        "Archive source changed between sealing passes",
                    )));
                }
                previous_index = Some(event.index);
                if event.index as i64 <= upload.prior_watermark {
                    continue;
                }
                batch.push(event);
                if batch.len() == self.archive_batch_size as usize {
                    upload
                        .progress
                        .source_bytes
                        .fetch_add(serde_json::to_vec(&batch)?.len() as u64, Ordering::SeqCst);
                    self.upload_batch(
                        task_id,
                        lease,
                        lease_lost,
                        ordinal,
                        &mut previous_batch_digest,
                        std::mem::take(&mut batch),
                    )
                    .await?;
                    ordinal += 1;
                    batch = Vec::with_capacity(self.archive_batch_size as usize);
                }
            }
            if page.done {
                break;
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Err(boxed(StorageIntegrityError::new(
                    "Archive source page omitted its next cursor",
                )));
            }
        }
        if !batch.is_empty() {
            upload
                .progress
                .source_bytes
                .fetch_add(serde_json::to_vec(&batch)?.len() as u64, Ordering::SeqCst);
            self.upload_batch(
                task_id,
                lease,
                lease_lost,
                ordinal,
                &mut previous_batch_digest,
                batch,
            )
            .await?;
            ordinal += 1;
        }
        if ordinal as usize != upload.manifest.expected_batch_ordinals.len() {
            return Err(boxed(StorageIntegrityError::new(
                "Archive source changed between sealing passes",
            )));
        }
        Ok(())
    }

    async fn upload_batch(
        &self,
        task_id: &str,
        lease: &StorageLease,
        lease_lost: &AtomicBool,
        ordinal: u64,
        previous_batch_digest: &mut Option<String>,
        events: Vec<TaskEvent>,
    ) -> StorageResult<ArchiveBatchReceipt> {
        self.renew(lease, lease_lost).await?;
        let batch_digest =
            compute_archive_batch_digest(previous_batch_digest.as_deref(), &events, &[])?;
        let receipt = ArchiveBatchReceipt {
            task_id: task_id.to_string(),
            generation: lease.generation.clone(),
            ordinal,
            previous_batch_digest: previous_batch_digest.clone(),
            batch_digest: batch_digest.clone(),
            entry_count: events.len() as u64,
            first_index: events.first().map(|event| event.index),
            last_index: events.last().map(|event| event.index),
        };
        let stored = self
            .long_term_store
            .archive_batch(
                task_id,
                &lease.generation,
                ArchiveBatch {
                    receipt,
                    events,
                    series_latest: vec![],
                },
            )
            .await?;
        *previous_batch_digest = Some(batch_digest);
        Ok(stored)
    }

    async fn renew(&self, lease: &StorageLease, lease_lost: &AtomicBool) -> StorageResult<()> {
        if lease_lost.load(Ordering::SeqCst) {
            return Err(boxed(StorageFenceConflictError::new(
                "Storage lease was lost",
            )));
        }
        match self
            .short_term_store
            .renew_storage_lock(lease, self.storage_lock_ttl_ms)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => {
                lease_lost.store(true, Ordering::SeqCst);
                Err(boxed(StorageFenceConflictError::new(
                    "Storage lease was lost",
                )))
            }
        }
    }

    fn observe(&self, observation: serde_json::Value) {
        if let Some(observer) = &self.observer {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                observer(&observation);
            }));
        }
    }

    fn error_code(error: &(dyn std::error::Error + Send + Sync + 'static)) -> &'static str {
        if error.downcast_ref::<StoragePreconditionError>().is_some() {
            "storage_precondition_failed"
        } else if error.downcast_ref::<StorageFenceConflictError>().is_some() {
            "storage_fence_conflict"
        } else if error.downcast_ref::<StorageBusyError>().is_some() {
            "storage_busy"
        } else if error.downcast_ref::<StorageIntegrityError>().is_some() {
            "storage_integrity_error"
        } else if error
            .downcast_ref::<StorageReleaseUnsupportedError>()
            .is_some()
        {
            "storage_release_unsupported"
        } else {
            "storage_unavailable"
        }
    }

    async fn reopen_after_failure(&self, task_id: &str, lease: &StorageLease) {
        let Ok(true) = self
            .short_term_store
            .renew_storage_lock(lease, self.storage_lock_ttl_ms)
            .await
        else {
            return;
        };
        let Ok(presence) = self
            .short_term_store
            .get_task_storage_presence(task_id)
            .await
        else {
            return;
        };
        if !presence.write_fence {
            return;
        }
        let Ok(token) = self
            .short_term_store
            .reopen_write_fence(lease, lease.storage_epoch)
            .await
        else {
            return;
        };
        let Ok(Some(current)) = self
            .long_term_store
            .get_task_storage_metadata(task_id)
            .await
        else {
            return;
        };
        if current.storage_epoch != lease.storage_epoch
            || (current.storage_state == StorageState::Releasing
                && current.active_release_generation.as_deref() != Some(lease.generation.as_str()))
        {
            return;
        }
        let next = TaskStorageMetadata {
            storage_state: StorageState::Hot,
            storage_epoch: token.storage_epoch,
            active_release_generation: None,
            cold_at: None,
            ..current.clone()
        };
        let _ = self
            .long_term_store
            .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
                task_id: task_id.to_string(),
                expected_storage_state: current.storage_state,
                expected_storage_epoch: current.storage_epoch,
                expected_release_generation: current.active_release_generation,
                next,
            })
            .await;
    }

    fn require_capabilities(&self) -> StorageResult<()> {
        if !self.short_term_store.supports_hot_cold_release()
            || !self.long_term_store.supports_hot_cold_release()
        {
            return Err(boxed(StorageReleaseUnsupportedError::default()));
        }
        Ok(())
    }
}

fn boxed<E>(error: E) -> Box<dyn std::error::Error + Send + Sync>
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(error)
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1_000.0
}
