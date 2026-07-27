use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::state_machine::is_terminal;
use crate::storage_coordinator::StorageCoordinator;
use crate::types::{
    BroadcastProvider, ClosedWriteFence, HotWriteToken, LongTermStore, ShortTermStore,
    StorageBusyError, StorageFenceConflictError, StorageIntegrityError, StorageLease,
    StorageReleaseUnsupportedError, StorageState, Task, TaskEvent, TaskStatus, TerminalProjection,
    TtlClaim, WorkerAssignment,
};

type StorageResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type TimeoutProjectionCallback = Arc<dyn Fn(&Task, &TaskStatus) + Send + Sync>;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableTtlSweepResult {
    pub claimed: u64,
    pub timed_out: u64,
    pub race_lost: u64,
    pub failed: u64,
    pub projected: u64,
}

pub struct TtlCoordinator {
    short_term_store: Arc<dyn ShortTermStore>,
    long_term_store: Arc<dyn LongTermStore>,
    broadcast: Arc<dyn BroadcastProvider>,
    storage_coordinator: StorageCoordinator,
    storage_lock_ttl_ms: u64,
    on_timeout_projected: Option<TimeoutProjectionCallback>,
}

struct PreparedTimeout {
    task: Task,
    event: TaskEvent,
    assignment: Option<WorkerAssignment>,
    from: TaskStatus,
}

impl TtlCoordinator {
    pub fn new(
        short_term_store: Arc<dyn ShortTermStore>,
        long_term_store: Arc<dyn LongTermStore>,
        broadcast: Arc<dyn BroadcastProvider>,
    ) -> StorageResult<Self> {
        if !short_term_store.supports_hot_cold_release() || !long_term_store.supports_durable_ttl()
        {
            return Err(Box::new(StorageReleaseUnsupportedError::new(
                "Configured stores do not support durable TTL projection",
            )));
        }
        let storage_coordinator =
            StorageCoordinator::new(Arc::clone(&short_term_store), Arc::clone(&long_term_store));
        Ok(Self {
            short_term_store,
            long_term_store,
            broadcast,
            storage_coordinator,
            storage_lock_ttl_ms: 30_000,
            on_timeout_projected: None,
        })
    }

    pub fn with_storage_lock_ttl_ms(mut self, ttl_ms: u64) -> Self {
        assert!(ttl_ms > 0, "TTL storage lock duration must be positive");
        self.storage_lock_ttl_ms = ttl_ms;
        self
    }

    pub(crate) fn with_on_timeout_projected(mut self, callback: TimeoutProjectionCallback) -> Self {
        self.on_timeout_projected = Some(callback);
        self
    }

    pub async fn sweep_overdue(
        &self,
        limit: u64,
        claim_ttl_ms: Option<u64>,
    ) -> StorageResult<DurableTtlSweepResult> {
        let claims = self
            .long_term_store
            .claim_overdue_tasks(limit, claim_ttl_ms.unwrap_or(self.storage_lock_ttl_ms))
            .await?;
        let mut result = DurableTtlSweepResult {
            claimed: claims.len() as u64,
            ..Default::default()
        };
        for claim in claims {
            match self.process_claim(claim).await {
                Ok(true) => {
                    result.timed_out += 1;
                    result.projected += 1;
                }
                Ok(false) => result.race_lost += 1,
                Err(_) => result.failed += 1,
            }
        }
        Ok(result)
    }

    pub async fn sweep_terminal_projections(
        &self,
        limit: u64,
        claim_ttl_ms: Option<u64>,
    ) -> StorageResult<DurableTtlSweepResult> {
        let projections = self
            .long_term_store
            .claim_terminal_projections(
                limit,
                &ulid::Ulid::new().to_string(),
                claim_ttl_ms.unwrap_or(self.storage_lock_ttl_ms),
            )
            .await?;
        let mut result = DurableTtlSweepResult {
            claimed: projections.len() as u64,
            ..Default::default()
        };
        for projection in projections {
            match self.project_claimed_terminal(&projection).await {
                Ok(true) => result.projected += 1,
                Ok(false) => {}
                Err(_) => result.failed += 1,
            }
        }
        Ok(result)
    }

    async fn process_claim(&self, claim: TtlClaim) -> StorageResult<bool> {
        let token = self
            .storage_coordinator
            .ensure_task_hot_for_write(&claim.task_id)
            .await?;
        let lease = self
            .short_term_store
            .acquire_storage_lock(
                &claim.task_id,
                &ulid::Ulid::new().to_string(),
                &format!("ttl:{}", claim.claim_token),
                self.storage_lock_ttl_ms,
            )
            .await?
            .ok_or_else(|| {
                Box::new(StorageBusyError::new("TTL task storage is busy"))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let mut terminalized = false;
        let mut fence_closed = false;
        let outcome = async {
            self.renew(&lease).await?;
            let closed = self
                .short_term_store
                .close_write_fence(&lease, token.storage_epoch)
                .await?;
            fence_closed = true;
            let prepared = self.prepare_timeout(&claim, &closed).await?;
            self.renew(&lease).await?;
            let projection = self
                .long_term_store
                .terminalize_ttl_claim(claim, prepared.task, prepared.event, prepared.assignment)
                .await?;
            let Some(projection) = projection else {
                self.reopen_after_race(&lease, token.storage_epoch).await?;
                fence_closed = false;
                return Ok(false);
            };
            terminalized = true;
            self.project_with_lease(&projection, &lease, token.storage_epoch)
                .await?;
            fence_closed = false;
            self.broadcast
                .publish(&projection.task.id, projection.event.clone())
                .await?;
            self.long_term_store
                .complete_terminal_projection(&projection)
                .await?;
            if let Some(callback) = &self.on_timeout_projected {
                callback(&projection.task, &prepared.from);
            }
            Ok(true)
        }
        .await;
        if outcome.is_err() && !terminalized && fence_closed {
            let _ = self.reopen_after_race(&lease, token.storage_epoch).await;
        }
        let _ = self.short_term_store.release_storage_lock(&lease).await;
        outcome
    }

    async fn prepare_timeout(
        &self,
        claim: &TtlClaim,
        closed: &ClosedWriteFence,
    ) -> StorageResult<PreparedTimeout> {
        let snapshot = self
            .short_term_store
            .get_task_mutation_snapshot(&claim.task_id)
            .await?
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new("TTL hot task is missing"))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let durable_task = self
            .long_term_store
            .get_task(&claim.task_id)
            .await?
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new("TTL durable task is missing"))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let durable_last_index = self
            .long_term_store
            .get_last_event_index(&claim.task_id)
            .await?;
        let assignment = self
            .short_term_store
            .get_task_assignment(&claim.task_id)
            .await?;
        let metadata = self
            .long_term_store
            .get_task_storage_metadata(&claim.task_id)
            .await?
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new("TTL task metadata is missing"))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let hot_task = snapshot.task;
        if is_terminal(&hot_task.status)
            || hot_task.status != durable_task.status
            || hot_task.updated_at != durable_task.updated_at
            || hot_task.assigned_worker != durable_task.assigned_worker
            || metadata.task_version != claim.task_version
            || metadata.execution_deadline_at != Some(claim.execution_deadline_at)
        {
            return Err(Box::new(StorageFenceConflictError::new(format!(
                "TTL task changed after it was claimed: {}",
                claim.task_id
            ))));
        }
        if closed.high_watermark != durable_last_index || closed.high_watermark == i64::MAX {
            return Err(Box::new(StorageFenceConflictError::new(format!(
                "TTL task history is not durably caught up: {}",
                claim.task_id
            ))));
        }
        let now = now_millis();
        let from = hot_task.status.clone();
        let mut task = hot_task;
        task.status = TaskStatus::Timeout;
        task.updated_at = now;
        task.completed_at = Some(now);
        task.assigned_worker = None;
        task.reason = None;
        task.blocked_request = None;
        task.resume_at = None;
        let event = TaskEvent {
            id: ulid::Ulid::new().to_string(),
            task_id: claim.task_id.clone(),
            index: (closed.high_watermark + 1) as u64,
            timestamp: now,
            r#type: "taskcast:status".to_string(),
            level: crate::types::Level::Info,
            data: serde_json::json!({"status": "timeout"}),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        };
        Ok(PreparedTimeout {
            task,
            event,
            assignment,
            from,
        })
    }

    async fn project_claimed_terminal(
        &self,
        projection: &TerminalProjection,
    ) -> StorageResult<bool> {
        if projection.claim_token.is_none() || projection.claim_until.is_none() {
            return Err(Box::new(StorageIntegrityError::new(
                "Claimed terminal projection has no claim",
            )));
        }
        let metadata = self
            .long_term_store
            .get_task_storage_metadata(&projection.task.id)
            .await?
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new(
                    "Terminal projection task metadata is missing",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
        if metadata.storage_state == StorageState::Releasing {
            return Err(Box::new(StorageBusyError::new(
                "Terminal projection storage is being released",
            )));
        }
        let token = if metadata.storage_state == StorageState::Cold {
            self.storage_coordinator
                .ensure_task_hot_for_write(&projection.task.id)
                .await?
        } else {
            HotWriteToken {
                task_id: projection.task.id.clone(),
                storage_epoch: metadata.storage_epoch,
            }
        };
        let lease = self
            .short_term_store
            .acquire_storage_lock(
                &projection.task.id,
                &ulid::Ulid::new().to_string(),
                &format!(
                    "terminal:{}:{}",
                    projection.projection_id,
                    projection.claim_token.as_deref().unwrap_or_default()
                ),
                self.storage_lock_ttl_ms,
            )
            .await?
            .ok_or_else(|| {
                Box::new(StorageBusyError::new("Terminal projection storage is busy"))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        let outcome = async {
            self.renew(&lease).await?;
            self.short_term_store
                .close_write_fence(&lease, token.storage_epoch)
                .await?;
            let projected = self
                .project_with_lease(projection, &lease, token.storage_epoch)
                .await?;
            self.broadcast
                .publish(&projection.task.id, projection.event.clone())
                .await?;
            self.long_term_store
                .complete_terminal_projection(projection)
                .await?;
            Ok(projected)
        }
        .await;
        let _ = self.short_term_store.release_storage_lock(&lease).await;
        outcome
    }

    async fn project_with_lease(
        &self,
        projection: &TerminalProjection,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> StorageResult<bool> {
        self.renew(lease).await?;
        let next_epoch = expected_epoch.checked_add(1).ok_or_else(|| {
            Box::new(StorageIntegrityError::new(
                "TTL storage epoch exceeds safe bounds",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        let result = self
            .short_term_store
            .project_terminal_fenced(projection, lease, expected_epoch, next_epoch)
            .await?;
        let metadata = self
            .long_term_store
            .get_task_storage_metadata(&projection.task.id)
            .await?
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new("TTL task metadata is missing"))
                    as Box<dyn std::error::Error + Send + Sync>
            })?;
        if metadata.storage_epoch != next_epoch {
            let mut next = metadata.clone();
            next.storage_state = StorageState::Hot;
            next.storage_epoch = next_epoch;
            next.active_release_generation = None;
            next.cold_at = None;
            let installed = self
                .long_term_store
                .compare_and_set_task_storage_metadata(crate::types::TaskStorageMetadataCas {
                    task_id: projection.task.id.clone(),
                    expected_storage_state: metadata.storage_state,
                    expected_storage_epoch: expected_epoch,
                    expected_release_generation: metadata.active_release_generation,
                    next,
                })
                .await?;
            if !installed
                && self
                    .long_term_store
                    .get_task_storage_metadata(&projection.task.id)
                    .await?
                    .is_none_or(|current| current.storage_epoch != next_epoch)
            {
                return Err(Box::new(StorageFenceConflictError::new(
                    "TTL terminal projection lost its storage metadata race",
                )));
            }
        }
        Ok(result.projected)
    }

    async fn reopen_after_race(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> StorageResult<()> {
        self.renew(lease).await?;
        let token = self
            .short_term_store
            .reopen_write_fence(lease, expected_epoch)
            .await?;
        let Some(metadata) = self
            .long_term_store
            .get_task_storage_metadata(&lease.task_id)
            .await?
        else {
            return Ok(());
        };
        if metadata.storage_epoch == token.storage_epoch {
            return Ok(());
        }
        let mut next = metadata.clone();
        next.storage_state = StorageState::Hot;
        next.storage_epoch = token.storage_epoch;
        next.active_release_generation = None;
        next.cold_at = None;
        self.long_term_store
            .compare_and_set_task_storage_metadata(crate::types::TaskStorageMetadataCas {
                task_id: lease.task_id.clone(),
                expected_storage_state: metadata.storage_state,
                expected_storage_epoch: expected_epoch,
                expected_release_generation: metadata.active_release_generation,
                next,
            })
            .await?;
        Ok(())
    }

    async fn renew(&self, lease: &StorageLease) -> StorageResult<()> {
        if !self
            .short_term_store
            .renew_storage_lock(lease, self.storage_lock_ttl_ms)
            .await?
        {
            return Err(Box::new(StorageFenceConflictError::new(
                "TTL storage lease was lost",
            )));
        }
        Ok(())
    }
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
