use crate::{
    BoxError, CleanupBatchResult, CleanupClaim, EngineError, LongTermStore, ShortTermStore,
    StorageBusyError, StorageFenceConflictError, StorageLease, StoragePreconditionError,
    StorageReleaseUnsupportedError, StorageState, TaskEngine,
};
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupSweepResult {
    pub claimed: u64,
    pub completed: u64,
    pub deferred: u64,
    pub failed: u64,
    pub deleted_events: u64,
}

pub struct CleanupCoordinator {
    hot: Arc<dyn ShortTermStore>,
    durable: Arc<dyn LongTermStore>,
}
impl CleanupCoordinator {
    pub fn new(
        hot: Arc<dyn ShortTermStore>,
        durable: Arc<dyn LongTermStore>,
    ) -> Result<Self, BoxError> {
        if !hot.supports_hot_cold_release()
            || !durable.supports_terminal_cleanup()
            || !durable.supports_hot_cold_release()
            || !durable.supports_task_creation_claims()
        {
            return Err(Box::new(StorageReleaseUnsupportedError::new(
                "Terminal cleanup requires complete fenced hot and durable store capabilities",
            )));
        }
        Ok(Self { hot, durable })
    }
    pub async fn sweep(
        &self,
        engine: &TaskEngine,
        limit: u64,
        batch_size: u64,
        ttl: u64,
    ) -> Result<CleanupSweepResult, BoxError> {
        if [limit, batch_size, ttl]
            .iter()
            .any(|v| *v == 0 || *v > i32::MAX as u64)
        {
            return Err(Box::new(StoragePreconditionError::new(
                "Invalid cleanup sweep bound",
            )));
        }
        let claims = self.durable.claim_cleanup_tasks(limit, ttl).await?;
        let mut result = CleanupSweepResult {
            claimed: claims.len() as u64,
            ..Default::default()
        };
        for claim in claims {
            match self.process(engine, &claim, batch_size, ttl).await {
                Ok(batch) => {
                    result.deleted_events += batch.deleted_events;
                    if batch.complete {
                        result.completed += 1;
                    } else {
                        result.deferred += 1;
                        let _ = self.durable.defer_cleanup_claim(&claim, 5000).await;
                    }
                }
                Err(error) => {
                    if error.is::<StorageBusyError>()
                        || error.is::<StorageFenceConflictError>()
                        || error.is::<StoragePreconditionError>()
                    {
                        result.deferred += 1;
                    } else {
                        result.failed += 1;
                    }
                    let _ = self.durable.defer_cleanup_claim(&claim, 5000).await;
                }
            }
        }
        Ok(result)
    }
    async fn writers_ready(&self) -> Result<(), BoxError> {
        if self
            .hot
            .list_storage_writers()
            .await?
            .iter()
            .any(|w| w.storage_protocol_version < 3)
        {
            return Err(Box::new(StorageBusyError::new(
                "Terminal cleanup requires storage protocol v3 writers",
            )));
        }
        Ok(())
    }
    async fn renew(
        &self,
        claim: &CleanupClaim,
        lease: &Mutex<Option<StorageLease>>,
        ttl: u64,
    ) -> Result<(), BoxError> {
        if !self.durable.renew_cleanup_claim(claim, ttl).await? {
            return Err(Box::new(StorageFenceConflictError::new(
                "Cleanup claim was lost",
            )));
        }
        let lease = lease.lock().unwrap().clone();
        if let Some(lease) = lease {
            if !self.hot.renew_storage_lock(&lease, ttl).await? {
                return Err(Box::new(StorageFenceConflictError::new(
                    "Cleanup storage lease was lost",
                )));
            }
        }
        Ok(())
    }
    async fn process(
        &self,
        engine: &TaskEngine,
        claim: &CleanupClaim,
        batch_size: u64,
        ttl: u64,
    ) -> Result<CleanupBatchResult, BoxError> {
        let lease = Mutex::new(None);
        let outcome = {
            let operation = self.process_inner(engine, claim, batch_size, ttl, &lease);
            let heartbeat = async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis((ttl / 3).max(1))).await;
                    if let Err(error) = self.renew(claim, &lease, ttl).await {
                        break error;
                    }
                }
            };
            tokio::pin!(operation, heartbeat);
            // Both futures keep progressing: renewal may wait for a row lock held
            // by an archive transaction that the operation must finish first.
            tokio::select! {
                biased;
                result = &mut operation => result,
                error = &mut heartbeat => Err(error),
            }
        };
        let owned = lease.lock().unwrap().take();
        if let Some(owned) = owned {
            let _ = self.hot.release_storage_lock(&owned).await;
        }
        outcome
    }
    async fn process_inner(
        &self,
        engine: &TaskEngine,
        claim: &CleanupClaim,
        batch_size: u64,
        ttl: u64,
        lease: &Mutex<Option<StorageLease>>,
    ) -> Result<CleanupBatchResult, BoxError> {
        self.renew(claim, lease, ttl).await?;
        self.writers_ready().await?;
        if !self.durable.can_cleanup_task(claim).await? {
            return Err(Box::new(StorageBusyError::new(
                "Task cleanup dependencies are not settled",
            )));
        }
        let metadata = self
            .durable
            .get_task_storage_metadata(&claim.task_id)
            .await?
            .filter(|m| m.creation_token.as_deref() == Some(&claim.creation_token))
            .ok_or_else(|| Box::new(StorageFenceConflictError::default()) as BoxError)?;
        if metadata.storage_state != StorageState::Cold {
            // The archive release path takes its own lease. Do not nest it.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as f64;
            engine
                .release_task_storage_at_current_durable_index(&claim.task_id, now)
                .await
                .map_err(|error| match error {
                    EngineError::Store(source) => source,
                    other => Box::new(other) as BoxError,
                })?;
        }
        self.renew(claim, lease, ttl).await?;
        let owned = self
            .hot
            .acquire_storage_lock(
                &claim.task_id,
                &ulid::Ulid::new().to_string(),
                &format!("cleanup:{}", claim.claim_token),
                ttl,
            )
            .await?
            .ok_or_else(|| Box::new(StorageBusyError::new("Task storage is busy")) as BoxError)?;
        *lease.lock().unwrap() = Some(owned);
        self.renew(claim, lease, ttl).await?;
        self.writers_ready().await?;
        let task = self.durable.get_task(&claim.task_id).await?;
        let metadata = self
            .durable
            .get_task_storage_metadata(&claim.task_id)
            .await?;
        let presence = self.hot.get_task_storage_presence(&claim.task_id).await?;
        if !task.is_some_and(|t| {
            t.completed_at == Some(claim.completed_at) && t.cleanup_policy_version == Some(1)
        }) || !metadata.as_ref().is_some_and(|m| {
            m.creation_token.as_deref() == Some(&claim.creation_token)
                && m.storage_state == StorageState::Cold
        }) || presence.task
            || presence.event_count != 0
            || presence.next_index
            || presence.series_state_count != 0
            || presence.write_fence
        {
            return Err(Box::new(StorageFenceConflictError::new(
                "Cleanup task generation or cold state changed",
            )));
        }
        let metadata = metadata.unwrap();
        let current = CleanupClaim {
            task_version: metadata.task_version,
            ..claim.clone()
        };
        self.renew(claim, lease, ttl).await?;
        if !self
            .durable
            .begin_task_cleanup(&current, metadata.storage_epoch, metadata.archive_watermark)
            .await?
        {
            return Err(Box::new(StorageBusyError::new(
                "Task cleanup preconditions changed",
            )));
        }
        self.renew(claim, lease, ttl).await?;
        self.durable
            .delete_task_cleanup_batch(&current, batch_size)
            .await
    }
}
