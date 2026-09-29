use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use taskcast_core::*;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Fault {
    #[default]
    None,
    ClaimLost,
    ClaimLostDuringWait,
    WaitForClaimRenewal,
    CleanupStorageLost,
    MetadataUnavailable,
    ColdStateChanged,
    CleanupBeginChanged,
    ReleaseMissing,
    ReleasePrecondition,
    ImportPending,
    ImportMetadataMissing,
    ImportMetadataChanged,
    ImportCasLost,
    ImportConcurrentCreated,
    ImportBusy,
    PublishEpochChanged,
    RehydrateAlreadyCommitted,
}

struct Durable {
    inner: Mutex<Arc<MemoryLongTermStore>>,
    fault: Mutex<Fault>,
    renewals: AtomicUsize,
    restored: AtomicBool,
    stale_metadata: Mutex<Option<TaskStorageMetadata>>,
}
impl Durable {
    fn new(inner: Arc<MemoryLongTermStore>) -> Self {
        Self {
            inner: Mutex::new(inner),
            fault: Mutex::new(Fault::None),
            renewals: AtomicUsize::new(0),
            restored: AtomicBool::new(false),
            stale_metadata: Mutex::new(None),
        }
    }
    fn inner(&self) -> Arc<MemoryLongTermStore> {
        self.inner.lock().unwrap().clone()
    }
    fn mode(&self) -> Fault {
        *self.fault.lock().unwrap()
    }
    fn set(&self, fault: Fault) {
        *self.fault.lock().unwrap() = fault;
    }
}
struct Hot {
    inner: Arc<MemoryShortTermStore>,
    fault: Mutex<Fault>,
    locked: AtomicBool,
}
impl Hot {
    fn new(inner: Arc<MemoryShortTermStore>) -> Self {
        Self {
            inner,
            fault: Mutex::new(Fault::None),
            locked: AtomicBool::new(false),
        }
    }
    fn inner(&self) -> Arc<MemoryShortTermStore> {
        self.inner.clone()
    }
    fn mode(&self) -> Fault {
        *self.fault.lock().unwrap()
    }
    fn set(&self, fault: Fault) {
        *self.fault.lock().unwrap() = fault;
    }
}
#[derive(Default)]
struct Legacy {
    inner: MemoryLongTermStore,
}
impl Legacy {
    fn inner(&self) -> &MemoryLongTermStore {
        &self.inner
    }
}

#[async_trait]
impl LongTermStore for Durable {
    fn supports_terminal_cleanup(&self) -> bool {
        self.inner().supports_terminal_cleanup()
    }
    async fn can_cleanup_task(&self, claim: &CleanupClaim) -> Result<bool, BoxError> {
        if self.mode() == Fault::ClaimLostDuringWait {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        // Keep the storage operation pending until the heartbeat actually renews
        // the claim, instead of depending on a fixed scheduling delay.
        if self.mode() == Fault::WaitForClaimRenewal {
            while self.renewals.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        self.inner().can_cleanup_task(claim).await
    }
    async fn claim_cleanup_tasks(
        &self,
        limit: u64,
        claim_ttl_ms: u64,
    ) -> Result<Vec<CleanupClaim>, BoxError> {
        self.inner().claim_cleanup_tasks(limit, claim_ttl_ms).await
    }
    async fn renew_cleanup_claim(&self, claim: &CleanupClaim, ttl: u64) -> Result<bool, BoxError> {
        let call = self.renewals.fetch_add(1, Ordering::SeqCst);
        if self.mode() == Fault::ClaimLost
            || (self.mode() == Fault::ClaimLostDuringWait && call > 0)
        {
            return Ok(false);
        }
        self.inner().renew_cleanup_claim(claim, ttl).await
    }
    async fn defer_cleanup_claim(&self, claim: &CleanupClaim, delay: u64) -> Result<(), BoxError> {
        self.inner().defer_cleanup_claim(claim, delay).await
    }
    async fn begin_task_cleanup(
        &self,
        claim: &CleanupClaim,
        epoch: u64,
        through: i64,
    ) -> Result<bool, BoxError> {
        if self.mode() == Fault::CleanupBeginChanged {
            return Ok(false);
        }
        self.inner().begin_task_cleanup(claim, epoch, through).await
    }
    async fn delete_task_cleanup_batch(
        &self,
        claim: &CleanupClaim,
        limit: u64,
    ) -> Result<CleanupBatchResult, BoxError> {
        self.inner().delete_task_cleanup_batch(claim, limit).await
    }
    async fn save_task_with_context(
        &self,
        task: Task,
        context: Option<&DurableWriteContext>,
    ) -> Result<(), BoxError> {
        self.inner().save_task_with_context(task, context).await
    }
    async fn save_event_with_context(
        &self,
        event: TaskEvent,
        context: Option<&DurableWriteContext>,
    ) -> Result<(), BoxError> {
        self.inner().save_event_with_context(event, context).await
    }
    async fn replace_last_series_event_with_context(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        context: Option<&DurableWriteContext>,
    ) -> Result<(), BoxError> {
        self.inner()
            .replace_last_series_event_with_context(task_id, series_id, event, context)
            .await
    }
    async fn accumulate_series_with_context(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
        context: Option<&DurableWriteContext>,
    ) -> Result<TaskEvent, BoxError> {
        self.inner()
            .accumulate_series_with_context(task_id, series_id, event, field, context)
            .await
    }
    fn supports_hot_cold_release(&self) -> bool {
        self.inner().supports_hot_cold_release()
    }
    fn supports_durable_ttl(&self) -> bool {
        self.inner().supports_durable_ttl()
    }
    fn supports_task_creation_claims(&self) -> bool {
        self.inner().supports_task_creation_claims()
    }
    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_task(task).await
    }
    async fn create_task_if_absent(
        &self,
        task: Task,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().create_task_if_absent(task).await
    }
    async fn claim_task_creation(
        &self,
        task: Task,
        creation_token: &str,
        claim_ttl_ms: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .claim_task_creation(task, creation_token, claim_ttl_ms)
            .await
    }
    async fn complete_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .complete_task_creation(task_id, creation_token)
            .await
    }
    async fn abort_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .abort_task_creation(task_id, creation_token)
            .await
    }
    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_task(task_id).await
    }
    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_event(event).await
    }
    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .replace_last_series_event(task_id, series_id, event)
            .await
    }
    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .accumulate_series(task_id, series_id, event, field)
            .await
    }
    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_events(task_id, opts).await
    }
    fn supports_series_compaction(&self) -> bool {
        self.inner().supports_series_compaction()
    }
    fn supports_task_archive_restore(&self) -> bool {
        true
    }
    fn shares_task_archive_restore_storage(&self) -> bool {
        self.inner().shares_task_archive_restore_storage()
    }
    async fn validate_task_archive_restore(
        &self,
        data: &TaskArchiveRestoreData,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !options.unwrap_or_default().overwrite
            && self.inner().get_task(&data.task.id).await?.is_some()
        {
            return Err(std::io::Error::other("Task already exists").into());
        }
        Ok(())
    }
    async fn restore_task_archive(
        &self,
        data: TaskArchiveRestoreData,
        _options: Option<TaskArchiveImportOptions>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if self.mode() == Fault::ImportPending {
            std::future::pending::<()>().await;
        }
        let previous = self.inner();
        let overwritten = previous.get_task(&data.task.id).await?.is_some();
        let next = Arc::new(MemoryLongTermStore::new());
        next.save_task(data.task.clone()).await?;
        for event in data.events {
            next.save_event(event).await?;
        }
        let metadata = next
            .get_task_storage_metadata(&data.task.id)
            .await?
            .unwrap();
        next.compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: data.task.id.clone(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: TaskStorageMetadata {
                storage_state: StorageState::Cold,
                storage_epoch: data.storage_epoch.unwrap(),
                archive_watermark: data.next_index as i64 - 1,
                cold_at: Some(1.0),
                ..metadata
            },
        })
        .await?;
        *self.inner.lock().unwrap() = next;
        self.restored.store(true, Ordering::SeqCst);
        Ok(overwritten)
    }
    async fn get_task_storage_metadata(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskStorageMetadata>, Box<dyn std::error::Error + Send + Sync>> {
        if self.mode() == Fault::MetadataUnavailable {
            return Err(std::io::Error::other("durable unavailable").into());
        }
        if let Some(stale) = self.stale_metadata.lock().unwrap().take() {
            return Ok(Some(stale));
        }
        if self.restored.load(Ordering::SeqCst) && self.mode() == Fault::ImportMetadataMissing {
            return Ok(None);
        }
        let mut metadata = self.inner().get_task_storage_metadata(task_id).await?;
        if self.restored.load(Ordering::SeqCst) && self.mode() == Fault::ImportMetadataChanged {
            metadata.as_mut().unwrap().storage_epoch += 1;
        }
        Ok(metadata)
    }
    async fn persist_storage_release_request(
        &self,
        request: StorageReleaseRequest,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        match self.mode() {
            Fault::ReleaseMissing => Ok(false),
            Fault::ReleasePrecondition => Err(Box::new(StoragePreconditionError::new(
                "release generation changed",
            ))),
            _ => self.inner().persist_storage_release_request(request).await,
        }
    }
    async fn clear_storage_release_request(
        &self,
        request: &StorageReleaseRequest,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().clear_storage_release_request(request).await
    }
    async fn list_storage_release_requests(
        &self,
        limit: u64,
    ) -> Result<Vec<StorageReleaseRequest>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().list_storage_release_requests(limit).await
    }
    async fn compare_and_set_task_storage_metadata(
        &self,
        update: TaskStorageMetadataCas,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if self.mode() == Fault::ImportCasLost && self.restored.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let committed = self
            .inner()
            .compare_and_set_task_storage_metadata(update)
            .await?;
        Ok(committed && self.mode() != Fault::RehydrateAlreadyCommitted)
    }
    async fn begin_archive(
        &self,
        generation: ArchiveGeneration,
    ) -> Result<ArchiveGeneration, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().begin_archive(generation).await
    }
    async fn archive_batch(
        &self,
        task_id: &str,
        generation: &str,
        batch: ArchiveBatch,
    ) -> Result<ArchiveBatchReceipt, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().archive_batch(task_id, generation, batch).await
    }
    async fn finalize_archive(
        &self,
        task_id: &str,
        generation: &str,
        task: Task,
        series_latest: Vec<DurableSeriesState>,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .finalize_archive(task_id, generation, task, series_latest)
            .await
    }
    async fn get_archive_watermark(
        &self,
        task_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_archive_watermark(task_id).await
    }
    async fn get_last_event_index(
        &self,
        task_id: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_last_event_index(task_id).await
    }
    async fn get_recent_events(
        &self,
        task_id: &str,
        limit: u64,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_recent_events(task_id, limit).await
    }
    async fn get_durable_series_state(
        &self,
        task_id: &str,
    ) -> Result<Vec<DurableSeriesState>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_durable_series_state(task_id).await
    }
    async fn claim_overdue_tasks(
        &self,
        limit: u64,
        claim_ttl_ms: u64,
    ) -> Result<Vec<TtlClaim>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().claim_overdue_tasks(limit, claim_ttl_ms).await
    }
    async fn terminalize_ttl_claim(
        &self,
        claim: TtlClaim,
        task: Task,
        event: TaskEvent,
        assignment: Option<WorkerAssignment>,
    ) -> Result<Option<TerminalProjection>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .terminalize_ttl_claim(claim, task, event, assignment)
            .await
    }
    async fn claim_terminal_projections(
        &self,
        limit: u64,
        claim_token: &str,
        claim_ttl_ms: u64,
    ) -> Result<Vec<TerminalProjection>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .claim_terminal_projections(limit, claim_token, claim_ttl_ms)
            .await
    }
    async fn complete_terminal_projection(
        &self,
        projection: &TerminalProjection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().complete_terminal_projection(projection).await
    }
    async fn save_durable_assignment(
        &self,
        assignment: WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_durable_assignment(assignment).await
    }
    async fn delete_durable_assignment(
        &self,
        task_id: &str,
        assignment_id: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .delete_durable_assignment(task_id, assignment_id)
            .await
    }
    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_worker_event(event).await
    }
    async fn get_worker_events(
        &self,
        worker_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_worker_events(worker_id, opts).await
    }
}

#[async_trait]
impl ShortTermStore for Hot {
    fn supports_hot_cold_release(&self) -> bool {
        self.inner().supports_hot_cold_release()
    }
    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_task(task).await
    }
    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        if self.mode() == Fault::ImportConcurrentCreated && self.locked.load(Ordering::SeqCst) {
            return Ok(Some(make_task(task_id)));
        }
        self.inner().get_task(task_id).await
    }
    async fn append_event(
        &self,
        task_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().append_event(task_id, event).await
    }
    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_events(task_id, opts).await
    }
    async fn set_ttl(
        &self,
        task_id: &str,
        ttl_seconds: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().set_ttl(task_id, ttl_seconds).await
    }
    async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_series_latest(task_id, series_id).await
    }
    async fn set_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .set_series_latest(task_id, series_id, event)
            .await
    }
    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .replace_last_series_event(task_id, series_id, event)
            .await
    }
    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .accumulate_series(task_id, series_id, event, field)
            .await
    }
    async fn next_index(
        &self,
        task_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().next_index(task_id).await
    }
    fn supports_task_archive_restore(&self) -> bool {
        self.inner().supports_task_archive_restore()
    }
    async fn validate_task_archive_restore(
        &self,
        data: &TaskArchiveRestoreData,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .validate_task_archive_restore(data, options)
            .await
    }
    async fn restore_task_archive(
        &self,
        data: TaskArchiveRestoreData,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().restore_task_archive(data, options).await
    }
    async fn acquire_storage_lock(
        &self,
        task_id: &str,
        lock_token: &str,
        generation: &str,
        ttl_ms: u64,
    ) -> Result<Option<StorageLease>, Box<dyn std::error::Error + Send + Sync>> {
        if self.mode() == Fault::ImportBusy {
            return Ok(None);
        }
        let lease = self
            .inner()
            .acquire_storage_lock(task_id, lock_token, generation, ttl_ms)
            .await?;
        self.locked.store(lease.is_some(), Ordering::SeqCst);
        Ok(lease)
    }
    async fn renew_storage_lock(
        &self,
        lease: &StorageLease,
        ttl_ms: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if matches!(
            self.mode(),
            Fault::ImportPending | Fault::CleanupStorageLost
        ) {
            return Ok(false);
        }
        self.inner().renew_storage_lock(lease, ttl_ms).await
    }
    async fn release_storage_lock(
        &self,
        lease: &StorageLease,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().release_storage_lock(lease).await
    }
    async fn get_write_fence(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskWriteFence>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_write_fence(task_id).await
    }
    async fn close_write_fence(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<ClosedWriteFence, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().close_write_fence(lease, expected_epoch).await
    }
    async fn reopen_write_fence(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<HotWriteToken, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().reopen_write_fence(lease, expected_epoch).await
    }
    async fn commit_event_fenced(
        &self,
        task_id: &str,
        event: TaskEvent,
        token: &HotWriteToken,
    ) -> Result<SeriesResult, Box<dyn std::error::Error + Send + Sync>> {
        if self.mode() == Fault::PublishEpochChanged {
            self.set(Fault::None);
            let lease = self
                .inner()
                .acquire_storage_lock(task_id, "racing-release", "racing-release", 30000)
                .await?
                .unwrap();
            self.inner()
                .close_write_fence(&lease, token.storage_epoch)
                .await?;
            self.inner()
                .reopen_write_fence(&lease, token.storage_epoch)
                .await?;
            self.inner().release_storage_lock(&lease).await?;
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        self.inner()
            .commit_event_fenced(task_id, event, token)
            .await
    }
    async fn get_task_mutation_snapshot(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskMutationSnapshot>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_task_mutation_snapshot(task_id).await
    }
    async fn commit_task_events_fenced(
        &self,
        task: Task,
        expected_revision: &str,
        events: Vec<TaskEvent>,
        token: &HotWriteToken,
    ) -> Result<Option<Vec<TaskEvent>>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .commit_task_events_fenced(task, expected_revision, events, token)
            .await
    }
    async fn save_task_fenced(
        &self,
        task: Task,
        token: &HotWriteToken,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_task_fenced(task, token).await
    }
    async fn read_archive_source_page(
        &self,
        task_id: &str,
        watermark: i64,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<ArchiveSourcePage, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .read_archive_source_page(task_id, watermark, cursor, limit)
            .await
    }
    async fn delete_task_storage_fenced(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .delete_task_storage_fenced(lease, expected_epoch)
            .await
    }
    async fn restore_hot_task_fenced(
        &self,
        snapshot: RehydrateSnapshot,
        lease: &StorageLease,
        next_epoch: u64,
    ) -> Result<HotWriteToken, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .restore_hot_task_fenced(snapshot, lease, next_epoch)
            .await
    }
    async fn project_terminal_fenced(
        &self,
        projection: &TerminalProjection,
        lease: &StorageLease,
        expected_epoch: u64,
        next_epoch: u64,
    ) -> Result<TerminalProjectionResult, Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .project_terminal_fenced(projection, lease, expected_epoch, next_epoch)
            .await
    }
    async fn get_task_storage_presence(
        &self,
        task_id: &str,
    ) -> Result<TaskStoragePresence, Box<dyn std::error::Error + Send + Sync>> {
        let mut presence = self.inner().get_task_storage_presence(task_id).await?;
        if self.mode() == Fault::ColdStateChanged {
            presence.next_index = true;
        }
        Ok(presence)
    }
    async fn register_storage_writer(
        &self,
        registration: StorageWriterRegistration,
        ttl_ms: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner()
            .register_storage_writer(registration, ttl_ms)
            .await
    }
    async fn list_storage_writers(
        &self,
    ) -> Result<Vec<StorageWriterRegistration>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().list_storage_writers().await
    }
    async fn list_tasks(
        &self,
        filter: TaskFilter,
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().list_tasks(filter).await
    }
    async fn save_worker(
        &self,
        worker: Worker,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_worker(worker).await
    }
    async fn get_worker(
        &self,
        worker_id: &str,
    ) -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_worker(worker_id).await
    }
    async fn list_workers(
        &self,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().list_workers(filter).await
    }
    async fn delete_worker(
        &self,
        worker_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().delete_worker(worker_id).await
    }
    async fn claim_task(
        &self,
        task_id: &str,
        worker_id: &str,
        cost: u32,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().claim_task(task_id, worker_id, cost).await
    }
    async fn add_assignment(
        &self,
        assignment: WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().add_assignment(assignment).await
    }
    async fn remove_assignment(
        &self,
        task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().remove_assignment(task_id).await
    }
    async fn get_worker_assignments(
        &self,
        worker_id: &str,
    ) -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_worker_assignments(worker_id).await
    }
    async fn get_task_assignment(
        &self,
        task_id: &str,
    ) -> Result<Option<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_task_assignment(task_id).await
    }
    async fn clear_ttl(
        &self,
        task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().clear_ttl(task_id).await
    }
    async fn list_by_status(
        &self,
        statuses: &[TaskStatus],
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().list_by_status(statuses).await
    }
}

#[async_trait]
impl LongTermStore for Legacy {
    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_task(task).await
    }
    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_task(task_id).await
    }
    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_event(event).await
    }
    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_events(task_id, opts).await
    }
    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner().save_worker_event(event).await
    }
    async fn get_worker_events(
        &self,
        worker_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner().get_worker_events(worker_id, opts).await
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}
fn make_task(id: &str) -> Task {
    serde_json::from_value(json!({"id": id, "status": "running", "createdAt": 1, "updatedAt": 1}))
        .unwrap()
}
fn event(id: &str, index: u64) -> TaskEvent {
    serde_json::from_value(json!({"id": format!("{id}-{index}"), "taskId": id, "index": index, "timestamp": 1000 + index, "type": "message", "level": "info", "data": {"delta":"A"}})).unwrap()
}
fn config() -> ResolvedCleanupConfig {
    ResolvedCleanupConfig {
        enabled: true,
        rules: vec![serde_json::from_value(json!({"target":"events","trigger":{}})).unwrap()],
    }
}
fn engine(hot: Arc<dyn ShortTermStore>, durable: Arc<dyn LongTermStore>) -> TaskEngine {
    TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    })
}
async fn fixture(count: u64, archived: bool, series: bool) -> (Arc<Hot>, Arc<Durable>, TaskEngine) {
    let hot = Arc::new(Hot::new(Arc::new(MemoryShortTermStore::new())));
    let durable = Arc::new(Durable::new(Arc::new(MemoryLongTermStore::new())));
    let mut task = make_task("task");
    task.cleanup_policy_version = Some(1);
    task.cleanup_resolved_at = Some(1.0);
    task.cleanup = Some(CleanupConfig {
        rules: config().rules,
    });
    let context = DurableWriteContext {
        creation_token: "creation".into(),
    };
    assert!(durable
        .inner()
        .claim_task_creation(task.clone(), &context.creation_token, 30000)
        .await
        .unwrap());
    assert!(durable
        .inner()
        .complete_task_creation("task", &context.creation_token)
        .await
        .unwrap());
    hot.inner.save_task(task.clone()).await.unwrap();
    for index in 0..count {
        let mut value = event("task", index);
        if series {
            value.series_id = Some(format!("series-{index}"));
            value.series_mode = Some(SeriesMode::Latest);
        }
        let result = hot
            .inner
            .commit_event_fenced(
                "task",
                value,
                &HotWriteToken {
                    task_id: "task".into(),
                    storage_epoch: 1,
                    creation_token: Some(context.creation_token.clone()),
                },
            )
            .await
            .unwrap();
        durable
            .inner()
            .save_event_with_context(result.event, Some(&context))
            .await
            .unwrap();
    }
    task.status = TaskStatus::Completed;
    task.completed_at = Some(1000.0);
    task.result = Some(serde_json::from_value(json!({"kept":true})).unwrap());
    durable
        .inner()
        .save_task_with_context(task.clone(), Some(&context))
        .await
        .unwrap();
    hot.inner.save_task(task).await.unwrap();
    if archived {
        StorageCoordinator::new(hot.clone(), durable.clone())
            .with_archive_batch_size(1)
            .release_task_storage(
                "task",
                ReleasePreconditions {
                    expected_last_event_index: count as i64 - 1,
                    inactive_since: now(),
                },
            )
            .await
            .unwrap();
    }
    let engine = engine(hot.clone(), durable.clone())
        .with_cleanup_config(config())
        .unwrap();
    (hot, durable, engine)
}
fn archive() -> TaskArchive {
    let mut accumulated = event("imported", 0);
    accumulated.series_id = Some("output".into());
    accumulated.series_mode = Some(SeriesMode::Accumulate);
    accumulated.series_acc_field = Some("delta".into());
    let mut latest = event("imported", 1);
    latest.series_id = Some("progress".into());
    latest.series_mode = Some(SeriesMode::Latest);
    TaskArchive {
        schema: "taskcast.taskArchive".into(),
        version: 1,
        exported_at: 2000.0,
        task: make_task("imported"),
        events: vec![accumulated, latest],
    }
}
fn import_fixture() -> (Arc<Hot>, Arc<Durable>, TaskEngine) {
    let hot = Arc::new(Hot::new(Arc::new(MemoryShortTermStore::new())));
    let durable = Arc::new(Durable::new(Arc::new(MemoryLongTermStore::new())));
    let engine = engine(hot.clone(), durable.clone());
    (hot, durable, engine)
}

#[tokio::test]
async fn legacy_adapters_explicitly_reject_every_cleanup_contract() {
    let store = Legacy::default();
    let claim = CleanupClaim {
        task_id: "legacy".into(),
        creation_token: "creation".into(),
        claim_token: "claim".into(),
        target: CleanupTarget::Events,
        completed_at: 1.0,
        task_version: 0,
    };
    store.save_task(make_task("legacy")).await.unwrap();
    assert!(store.get_task("legacy").await.unwrap().is_some());
    assert!(!store.supports_terminal_cleanup());
    assert!(store
        .can_cleanup_task(&claim)
        .await
        .unwrap_err()
        .is::<StorageReleaseUnsupportedError>());
    assert!(store
        .claim_cleanup_tasks(1, 30000)
        .await
        .unwrap_err()
        .is::<StorageReleaseUnsupportedError>());
    assert!(store
        .renew_cleanup_claim(&claim, 30000)
        .await
        .unwrap_err()
        .is::<StorageReleaseUnsupportedError>());
    assert!(store
        .defer_cleanup_claim(&claim, 1)
        .await
        .unwrap_err()
        .is::<StorageReleaseUnsupportedError>());
    assert!(store
        .begin_task_cleanup(&claim, 1, -1)
        .await
        .unwrap_err()
        .is::<StorageReleaseUnsupportedError>());
    assert!(store
        .delete_task_cleanup_batch(&claim, 1)
        .await
        .unwrap_err()
        .is::<StorageReleaseUnsupportedError>());
    assert!(
        CleanupCoordinator::new(Arc::new(MemoryShortTermStore::new()), Arc::new(store)).is_err()
    );
}

#[test]
fn cleanup_deadline_checks_the_current_task_type_instead_of_only_the_enrolled_rule() {
    let mut task = make_task("typed");
    task.status = TaskStatus::Completed;
    task.completed_at = Some(1000.0);
    task.cleanup_policy_version = Some(1);
    task.cleanup = Some(
        serde_json::from_value(
            json!({"rules":[{"target":"events","trigger":{},"match":{"taskTypes":["search.*"]}}]}),
        )
        .unwrap(),
    );
    for task_type in [None, Some("other")] {
        task.r#type = task_type.map(str::to_owned);
        assert_eq!(cleanup_deadline(&task, CleanupTarget::Events), None);
    }
    task.r#type = Some("search.youtube".into());
    assert_eq!(cleanup_deadline(&task, CleanupTarget::Events), Some(1000.0));
}

#[test]
fn cleanup_status_only_rules_apply_to_tasks_without_a_type_filter() {
    let mut task = make_task("status-only");
    task.status = TaskStatus::Completed;
    task.completed_at = Some(1000.0);
    task.cleanup_policy_version = Some(1);
    task.cleanup = Some(
        serde_json::from_value(json!({"rules":[{
            "target":"events", "trigger":{"afterMs":200}, "match":{"status":["completed"]}
        }]}))
        .unwrap(),
    );
    assert_eq!(cleanup_deadline(&task, CleanupTarget::Events), Some(1200.0));
    task.status = TaskStatus::Failed;
    assert_eq!(cleanup_deadline(&task, CleanupTarget::Events), None);
}

#[tokio::test]
async fn terminal_cache_remains_readable_with_a_legacy_durable_adapter() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(Legacy::default());
    let mut cached = make_task("legacy-read");
    cached.status = TaskStatus::Completed;
    cached.completed_at = Some(1000.0);
    cached.cleanup_policy_version = Some(1);
    hot.save_task(cached.clone()).await.unwrap();
    assert!(!durable.supports_terminal_cleanup());
    assert!(durable.get_task(&cached.id).await.unwrap().is_none());
    let engine = engine(hot.clone(), durable);
    assert_eq!(
        engine.get_task(&cached.id).await.unwrap(),
        Some(cached.clone())
    );
    let hot_only = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        long_term_store: None,
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    assert_eq!(hot_only.get_task(&cached.id).await.unwrap(), Some(cached));
}

#[tokio::test]
async fn cleanup_bounds_are_rejected_before_claiming_or_deleting_tasks() {
    let (_, durable, engine) = fixture(1, true, false).await;
    for bounds in [(0, 1, 1), (1, 0, 1), (1, 1, 0), (i32::MAX as u64 + 1, 1, 1)] {
        assert!(
            matches!(engine.sweep_cleanup(bounds.0, bounds.1, bounds.2).await, Err(EngineError::Store(source)) if source.is::<StoragePreconditionError>())
        );
    }
    assert_eq!(
        durable
            .inner()
            .get_events("task", None)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        engine.sweep_cleanup(1, 10, 30000).await.unwrap().completed,
        1
    );
}

#[tokio::test]
async fn cleanup_failures_are_classified_and_keep_history_available() {
    for fault in [
        Fault::ClaimLost,
        Fault::CleanupStorageLost,
        Fault::MetadataUnavailable,
        Fault::ColdStateChanged,
        Fault::CleanupBeginChanged,
    ] {
        let (hot, durable, engine) = fixture(2, true, false).await;
        durable.set(fault);
        hot.set(fault);
        let result = engine.sweep_cleanup(1, 10, 30000).await.unwrap();
        assert_eq!(result.completed, 0, "{fault:?}");
        assert_eq!(result.deleted_events, 0, "{fault:?}");
        assert_eq!(
            result.failed,
            u64::from(fault == Fault::MetadataUnavailable),
            "{fault:?}"
        );
        assert_eq!(
            result.deferred,
            u64::from(fault != Fault::MetadataUnavailable),
            "{fault:?}"
        );
        assert!(durable
            .inner()
            .get_task("task")
            .await
            .unwrap()
            .unwrap()
            .history_expired_at
            .is_none());
        assert_eq!(
            durable
                .inner()
                .get_events("task", None)
                .await
                .unwrap()
                .len(),
            2
        );
    }
}

#[tokio::test]
async fn cleanup_heartbeat_stops_a_blocked_operation_after_the_claim_is_lost() {
    let (_, durable, engine) = fixture(1, true, false).await;
    durable.set(Fault::ClaimLostDuringWait);
    let result = tokio::time::timeout(Duration::from_millis(500), engine.sweep_cleanup(1, 10, 90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((result.deferred, result.completed), (1, 0));
    assert!(durable.renewals.load(Ordering::SeqCst) >= 2);
    assert!(durable
        .inner()
        .get_task("task")
        .await
        .unwrap()
        .unwrap()
        .history_expired_at
        .is_none());
}

#[tokio::test]
async fn cleanup_heartbeat_renews_a_claim_while_the_storage_operation_is_pending() {
    let (_, durable, engine) = fixture(1, true, false).await;
    durable.set(Fault::WaitForClaimRenewal);
    let result = tokio::time::timeout(Duration::from_secs(2), engine.sweep_cleanup(1, 10, 300))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (result.completed, result.deleted_events, result.failed),
        (1, 1, 0)
    );
    assert!(durable.renewals.load(Ordering::SeqCst) >= 2);
    assert!(durable
        .inner()
        .get_task("task")
        .await
        .unwrap()
        .unwrap()
        .history_expired_at
        .is_some());
}

#[tokio::test]
async fn cleanup_preserves_store_and_engine_error_classification_during_archive_release() {
    for fault in [Fault::ReleasePrecondition, Fault::ReleaseMissing] {
        let (hot, durable, engine) = fixture(1, false, false).await;
        durable.set(fault);
        let result = engine.sweep_cleanup(1, 10, 30000).await.unwrap();
        assert_eq!((result.completed, result.deleted_events), (0, 0));
        assert_eq!(
            result.deferred,
            u64::from(fault == Fault::ReleasePrecondition)
        );
        assert_eq!(result.failed, u64::from(fault == Fault::ReleaseMissing));
        assert!(hot.inner.get_task("task").await.unwrap().is_some());
    }
}

#[tokio::test]
async fn cleanup_rejects_invalid_watermarks_and_delete_before_begin() {
    let (_, durable, _) = fixture(2, true, false).await;
    let store = durable.inner();
    let claim = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert!(store
        .begin_task_cleanup(&claim, 1, -2)
        .await
        .unwrap_err()
        .is::<StoragePreconditionError>());
    assert!(store
        .delete_task_cleanup_batch(&claim, 1)
        .await
        .unwrap_err()
        .is::<StorageFenceConflictError>());
    assert!(store.begin_task_cleanup(&claim, 1, 1).await.unwrap());
    let first = store.delete_task_cleanup_batch(&claim, 1).await.unwrap();
    assert!(!first.complete);
    store.defer_cleanup_claim(&claim, 0).await.unwrap();
    assert!(store
        .delete_task_cleanup_batch(&claim, 1)
        .await
        .unwrap_err()
        .is::<StorageFenceConflictError>());
    assert_eq!(store.get_events("task", None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn cleanup_batches_finish_archive_receipts_and_series_after_events_are_gone() {
    let (_, durable, _) = fixture(3, true, true).await;
    let store = durable.inner();
    let claim = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert!(store.begin_task_cleanup(&claim, 1, 2).await.unwrap());
    let mut batches = Vec::new();
    loop {
        let batch = store.delete_task_cleanup_batch(&claim, 1).await.unwrap();
        let complete = batch.complete;
        batches.push(batch);
        if complete {
            break;
        }
        assert!(batches.len() < 10);
    }
    assert_eq!(batches.iter().map(|b| b.deleted_events).sum::<u64>(), 3);
    assert!(batches.iter().any(|b| b.deleted_events == 0));
    assert!(store
        .get_durable_series_state("task")
        .await
        .unwrap()
        .is_empty());
    let retained = store.get_task("task").await.unwrap().unwrap();
    assert!(retained.history_expired_at.is_some());
    assert_eq!(retained.result.unwrap()["kept"], true);
}

#[tokio::test]
async fn cleanup_completes_an_empty_archive_without_batch_receipts() {
    let (_, durable, engine) = fixture(0, true, false).await;
    let result = engine.sweep_cleanup(1, 1, 30000).await.unwrap();
    assert_eq!(
        (result.claimed, result.completed, result.deleted_events),
        (1, 1, 0)
    );
    let retained = durable.inner().get_task("task").await.unwrap().unwrap();
    assert!(retained.history_expired_at.is_some());
    assert_eq!(retained.result.unwrap()["kept"], true);
    assert!(durable
        .inner()
        .claim_cleanup_tasks(1, 30000)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn cleanup_removes_completed_terminal_projections_without_replaying_timeout() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = engine(hot.clone(), durable.clone())
        .with_cleanup_config(config())
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            id: Some("ttl-projection".into()),
            ttl: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    // Model a deadline that became due before this worker's sweep.
    let metadata = durable
        .get_task_storage_metadata("ttl-projection")
        .await
        .unwrap()
        .unwrap();
    assert!(durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "ttl-projection".into(),
            expected_storage_state: metadata.storage_state.clone(),
            expected_storage_epoch: metadata.storage_epoch,
            expected_release_generation: None,
            next: TaskStorageMetadata {
                execution_deadline_at: Some(now() - 1.0),
                ..metadata
            },
        })
        .await
        .unwrap());
    let timeout = engine.sweep_durable_ttl(1, Some(30000)).await.unwrap();
    assert_eq!(timeout.projected, 1);
    let terminal = engine.get_task("ttl-projection").await.unwrap().unwrap();
    assert_eq!(terminal.status, TaskStatus::Timeout);
    assert_eq!(terminal.completed_at.unwrap().fract(), 0.0);
    let cleanup = engine.sweep_cleanup(1, 1, 30000).await.unwrap();
    assert_eq!((cleanup.completed, cleanup.deleted_events), (1, 1));
    assert!(durable
        .get_task("ttl-projection")
        .await
        .unwrap()
        .unwrap()
        .history_expired_at
        .is_some());
    assert!(durable
        .get_events("ttl-projection", None)
        .await
        .unwrap()
        .is_empty());
    assert!(durable
        .claim_terminal_projections(1, "later-worker", 30000)
        .await
        .unwrap()
        .is_empty());
    assert!(hot.get_task("ttl-projection").await.unwrap().is_none());
}

#[tokio::test]
async fn engine_reads_terminal_cleanup_state_from_durable_storage_over_stale_hot_cache() {
    let (hot, durable, engine) = fixture(1, true, false).await;
    let stale = durable.inner().get_task("task").await.unwrap().unwrap();
    let claim = durable
        .inner()
        .claim_cleanup_tasks(1, 30000)
        .await
        .unwrap()
        .remove(0);
    assert!(durable
        .inner()
        .begin_task_cleanup(&claim, 1, 0)
        .await
        .unwrap());
    hot.inner.save_task(stale).await.unwrap();
    assert!(engine
        .get_task("task")
        .await
        .unwrap()
        .unwrap()
        .history_expired_at
        .is_some());
    assert!(engine.get_events("task", None).await.unwrap().is_empty());
    assert!(matches!(
        engine.export_task_archive("task").await,
        Err(EngineError::HistoryExpired(_))
    ));
}

#[tokio::test]
async fn fenced_import_restores_canonical_series_and_replaces_a_hot_epoch() {
    let (hot, durable, engine) = import_fixture();
    let value = archive();
    let first = engine
        .import_task_archive(value.clone(), None)
        .await
        .unwrap();
    assert_eq!((first.event_count, first.overwritten), (2, false));
    let initial = hot
        .inner
        .get_write_fence("imported")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.storage_epoch, 2);
    assert_eq!(
        hot.inner
            .get_series_latest("imported", "output")
            .await
            .unwrap()
            .unwrap()
            .data,
        json!({"delta":"A"})
    );
    assert_eq!(
        engine.get_events("imported", None).await.unwrap(),
        value.events
    );
    assert!(matches!(
        engine.import_task_archive(value.clone(), None).await,
        Err(EngineError::TaskConflict(_))
    ));
    let mut replacement = value;
    replacement.task.metadata = Some(serde_json::from_value(json!({"restored":true})).unwrap());
    let result = engine
        .import_task_archive(
            replacement.clone(),
            Some(TaskArchiveImportOptions { overwrite: true }),
        )
        .await
        .unwrap();
    assert!(result.overwritten);
    let latest = hot
        .inner
        .get_write_fence("imported")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.storage_epoch, 4);
    assert_eq!(
        durable
            .inner()
            .get_task("imported")
            .await
            .unwrap()
            .unwrap()
            .metadata,
        replacement.task.metadata
    );
    assert!(hot
        .inner
        .commit_event_fenced(
            "imported",
            event("imported", 2),
            &HotWriteToken {
                task_id: "imported".into(),
                storage_epoch: initial.storage_epoch,
                creation_token: None
            }
        )
        .await
        .unwrap_err()
        .is::<StorageFenceConflictError>());
}

#[tokio::test]
async fn fenced_import_rechecks_identity_after_lock_acquisition_and_rejects_busy_tasks() {
    for fault in [Fault::ImportConcurrentCreated, Fault::ImportBusy] {
        let (hot, durable, engine) = import_fixture();
        hot.set(fault);
        let error = engine
            .import_task_archive(archive(), None)
            .await
            .unwrap_err();
        if fault == Fault::ImportConcurrentCreated {
            assert!(matches!(error, EngineError::TaskConflict(_)));
        } else {
            assert!(matches!(error, EngineError::Store(error) if error.is::<StorageBusyError>()));
        }
        assert!(!durable.restored.load(Ordering::SeqCst));
        assert!(hot.inner.get_task("imported").await.unwrap().is_none());
    }
}

#[tokio::test]
async fn fenced_import_requires_durable_metadata_readback_and_final_cas() {
    for fault in [
        Fault::ImportMetadataMissing,
        Fault::ImportMetadataChanged,
        Fault::ImportCasLost,
    ] {
        let (hot, durable, engine) = import_fixture();
        durable.set(fault);
        let error = engine
            .import_task_archive(archive(), None)
            .await
            .unwrap_err();
        if fault == Fault::ImportMetadataMissing {
            assert!(matches!(error, EngineError::TaskNotFound(_)));
        } else {
            assert!(
                matches!(error, EngineError::Store(error) if error.is::<StorageFenceConflictError>())
            );
        }
        assert_eq!(
            durable
                .inner()
                .get_task_storage_metadata("imported")
                .await
                .unwrap()
                .unwrap()
                .storage_state,
            StorageState::Cold
        );
        if fault != Fault::ImportCasLost {
            assert!(hot.inner.get_task("imported").await.unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn fenced_import_heartbeat_cancels_a_blocked_restore_after_lease_loss() {
    let (hot, durable, engine) = import_fixture();
    durable.set(Fault::ImportPending);
    hot.set(Fault::ImportPending);
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        engine.import_task_archive(archive(), None),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        matches!(error, EngineError::Store(error) if error.is::<StorageFenceConflictError>() && error.to_string().contains("lease was lost"))
    );
    assert!(!durable.restored.load(Ordering::SeqCst));
    assert!(hot.inner.get_task("imported").await.unwrap().is_none());
    // The cancelled importer released its lease; a later owner can retry.
    assert!(hot
        .inner
        .acquire_storage_lock("imported", "retry", "retry", 30000)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn publisher_does_not_cross_an_epoch_change_after_its_first_commit_conflict() {
    let (hot, durable, engine) = import_fixture();
    let task = engine
        .create_task(CreateTaskInput {
            id: Some("writer".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    hot.set(Fault::PublishEpochChanged);
    let error = engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "message".into(),
                level: Level::Info,
                data: json!({"late":true}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, EngineError::Store(error) if error.is::<StorageFenceConflictError>() && error.to_string().contains("epoch changed"))
    );
    assert!(hot
        .inner
        .get_events("writer", None)
        .await
        .unwrap()
        .is_empty());
    let token = StorageCoordinator::new(hot, durable)
        .ensure_task_hot_for_write("writer")
        .await
        .unwrap();
    assert_eq!(token.storage_epoch, 2);
    assert!(token.creation_token.is_some());
}

#[tokio::test]
async fn rehydration_adopts_a_concurrently_committed_epoch_with_the_same_creation_identity() {
    let (hot, durable, _) = fixture(1, true, false).await;
    let coordinator = StorageCoordinator::new(hot.clone(), durable.clone());
    durable.set(Fault::RehydrateAlreadyCommitted);
    let token = coordinator.ensure_task_hot_for_write("task").await.unwrap();
    assert_eq!(token.storage_epoch, 2);
    assert_eq!(token.creation_token.as_deref(), Some("creation"));
    let cold = TaskStorageMetadata {
        storage_state: StorageState::Cold,
        storage_epoch: 1,
        ..durable
            .inner()
            .get_task_storage_metadata("task")
            .await
            .unwrap()
            .unwrap()
    };
    *durable.stale_metadata.lock().unwrap() = Some(cold);
    let already_hot = coordinator.ensure_task_hot_for_write("task").await.unwrap();
    assert_eq!(already_hot, token);
    assert_eq!(hot.inner.get_events("task", None).await.unwrap().len(), 1);
}
