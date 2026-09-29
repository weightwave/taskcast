use super::*;
use crate::{cleanup_deadline, CleanupTarget, StoragePreconditionError};

#[derive(Clone)]
pub(super) struct MemoryCleanupClaim {
    pub claim: CleanupClaim,
    pub until: u128,
    pub in_progress: bool,
}
fn bound(value: u64, zero: bool) -> Result<(), BoxError> {
    if (!zero && value == 0) || value > i32::MAX as u64 {
        return Err(Box::new(StoragePreconditionError::new(
            "Invalid cleanup batch or lease bound",
        )));
    }
    Ok(())
}
impl MemoryLongTermStore {
    pub(super) fn cleanup_ready(&self, claim: &CleanupClaim) -> Result<bool, BoxError> {
        let _guard = self.lifecycle_guard.lock().unwrap();
        Ok(self.cleanup_valid(claim) && self.cleanup_settled(&claim.task_id))
    }
    pub(super) fn cleanup_claim(
        &self,
        limit: u64,
        ttl: u64,
    ) -> Result<Vec<CleanupClaim>, BoxError> {
        bound(limit, false)?;
        bound(ttl, false)?;
        let _guard = self.lifecycle_guard.lock().unwrap();
        let now = MemoryShortTermStore::now_ms();
        let tasks = self.tasks.read().unwrap();
        let creation = self.creation_claims.read().unwrap();
        let metadata = self.metadata.read().unwrap();
        let retry = self.cleanup_retry.read().unwrap();
        let mut claims = self.cleanup_claims.write().unwrap();
        let mut candidates = vec![];
        for task in tasks.values() {
            let Some(created) = creation.get(&task.id).filter(|c| c.completed_at.is_some()) else {
                continue;
            };
            let prior = claims.get(&task.id);
            if prior.is_some_and(|c| c.until > now) || retry.get(&task.id).is_some_and(|t| *t > now)
            {
                continue;
            }
            let all = cleanup_deadline(task, CleanupTarget::All);
            let events =
                if task.history_expired_at.is_none() || prior.is_some_and(|p| p.in_progress) {
                    cleanup_deadline(task, CleanupTarget::Events)
                } else {
                    None
                };
            let Some(due) = [all, events]
                .into_iter()
                .flatten()
                .min_by(f64::total_cmp)
                .filter(|due| *due <= now as f64)
            else {
                continue;
            };
            let target = if let Some(p) = prior.filter(|p| p.in_progress) {
                p.claim.target.clone()
            } else if all.is_some_and(|d| d <= now as f64) {
                CleanupTarget::All
            } else {
                CleanupTarget::Events
            };
            candidates.push((
                due.max(retry.get(&task.id).copied().unwrap_or(0) as f64),
                CleanupClaim {
                    task_id: task.id.clone(),
                    creation_token: created.token.clone(),
                    claim_token: ulid::Ulid::new().to_string(),
                    target,
                    completed_at: task.completed_at.unwrap(),
                    task_version: metadata.get(&task.id).unwrap().task_version,
                },
                prior.is_some_and(|p| p.in_progress),
            ));
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.task_id.cmp(&b.1.task_id)));
        let mut result = vec![];
        for (_, claim, in_progress) in candidates.into_iter().take(limit as usize) {
            claims.insert(
                claim.task_id.clone(),
                MemoryCleanupClaim {
                    claim: claim.clone(),
                    until: now + ttl as u128,
                    in_progress,
                },
            );
            result.push(claim);
        }
        Ok(result)
    }
    pub(super) fn cleanup_renew(&self, claim: &CleanupClaim, ttl: u64) -> Result<bool, BoxError> {
        bound(ttl, false)?;
        let _guard = self.lifecycle_guard.lock().unwrap();
        let now = MemoryShortTermStore::now_ms();
        let mut claims = self.cleanup_claims.write().unwrap();
        let Some(current) = claims.get_mut(&claim.task_id).filter(|c| {
            c.claim.claim_token == claim.claim_token
                && c.claim.creation_token == claim.creation_token
                && c.until > now
        }) else {
            return Ok(false);
        };
        current.until = now + ttl as u128;
        Ok(true)
    }
    pub(super) fn cleanup_defer(&self, claim: &CleanupClaim, delay: u64) -> Result<(), BoxError> {
        bound(delay, true)?;
        let _guard = self.lifecycle_guard.lock().unwrap();
        let mut claims = self.cleanup_claims.write().unwrap();
        if let Some(current) = claims.get_mut(&claim.task_id).filter(|c| {
            c.claim.claim_token == claim.claim_token
                && c.claim.creation_token == claim.creation_token
        }) {
            current.until = 0;
            self.cleanup_retry.write().unwrap().insert(
                claim.task_id.clone(),
                MemoryShortTermStore::now_ms() + delay as u128,
            );
        }
        Ok(())
    }
    fn cleanup_valid(&self, claim: &CleanupClaim) -> bool {
        self.cleanup_claims
            .read()
            .unwrap()
            .get(&claim.task_id)
            .is_some_and(|c| {
                c.until > MemoryShortTermStore::now_ms()
                    && c.claim.claim_token == claim.claim_token
                    && c.claim.target == claim.target
            })
            && self
                .creation_claims
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|c| c.token == claim.creation_token)
            && self
                .tasks
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|t| {
                    t.cleanup_policy_version == Some(1)
                        && t.completed_at == Some(claim.completed_at)
                        && is_memory_terminal(&t.status)
                })
            && self
                .metadata
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|m| m.task_version == claim.task_version)
    }
    fn cleanup_settled(&self, id: &str) -> bool {
        !self.durable_assignments.read().unwrap().contains_key(id)
            && !self
                .terminal_projections
                .read()
                .unwrap()
                .values()
                .any(|p| p.projection.task.id == id && p.projected_at.is_none())
            && !self
                .generations
                .read()
                .unwrap()
                .values()
                .any(|g| g.task_id == id && g.status == ArchiveGenerationStatus::Open)
    }
    pub(super) fn cleanup_begin(
        &self,
        claim: &CleanupClaim,
        epoch: u64,
        through: i64,
    ) -> Result<bool, BoxError> {
        bound(epoch, false)?;
        if through < -1 {
            return Err(Box::new(StoragePreconditionError::new(
                "Invalid cleanup watermark",
            )));
        }
        let _guard = self.lifecycle_guard.lock().unwrap();
        if !self.cleanup_valid(claim) || !self.cleanup_settled(&claim.task_id) {
            return Ok(false);
        }
        let metadata = self
            .metadata
            .read()
            .unwrap()
            .get(&claim.task_id)
            .cloned()
            .unwrap();
        let task = self
            .tasks
            .read()
            .unwrap()
            .get(&claim.task_id)
            .cloned()
            .unwrap();
        if metadata.storage_state != StorageState::Cold
            || metadata.storage_epoch != epoch
            || metadata.active_release_generation.is_some()
            || metadata.archive_watermark != through
            || !cleanup_deadline(&task, claim.target.clone())
                .is_some_and(|d| d <= MemoryShortTermStore::now_ms() as f64)
            || self
                .events
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|events| events.iter().any(|e| e.index as i64 > through))
            || self
                .series
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|states| states.iter().any(|s| s.through_index as i64 > through))
        {
            return Ok(false);
        }
        self.tasks
            .write()
            .unwrap()
            .get_mut(&claim.task_id)
            .unwrap()
            .history_expired_at
            .get_or_insert(MemoryShortTermStore::now_ms() as f64);
        self.cleanup_claims
            .write()
            .unwrap()
            .get_mut(&claim.task_id)
            .unwrap()
            .in_progress = true;
        Ok(true)
    }
    pub(super) fn cleanup_delete(
        &self,
        claim: &CleanupClaim,
        limit: u64,
    ) -> Result<CleanupBatchResult, BoxError> {
        bound(limit, false)?;
        let _guard = self.lifecycle_guard.lock().unwrap();
        if !self.cleanup_valid(claim)
            || !self.cleanup_settled(&claim.task_id)
            || !self
                .cleanup_claims
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|c| c.in_progress)
            || !self
                .tasks
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|t| t.history_expired_at.is_some())
            || !self
                .metadata
                .read()
                .unwrap()
                .get(&claim.task_id)
                .is_some_and(|m| m.storage_state == StorageState::Cold)
        {
            return Err(Box::new(StorageFenceConflictError::new(
                "Cleanup claim or cold-state precondition was lost",
            )));
        }
        let count = {
            let mut events = self.events.write().unwrap();
            let events = events.entry(claim.task_id.clone()).or_default();
            let count = events.len().min(limit as usize);
            events.drain(..count);
            if !events.is_empty() {
                return Ok(CleanupBatchResult {
                    deleted_events: count as u64,
                    complete: false,
                });
            }
            count as u64
        };
        {
            let mut series = self.series.write().unwrap();
            if let Some(states) = series.get_mut(&claim.task_id) {
                let count = states.len().min(limit as usize);
                states.drain(..count);
            }
        }
        let mut batch_budget = limit;
        let mut generation_budget = limit;
        {
            let mut batches = self.batches.write().unwrap();
            let mut generations = self.generations.write().unwrap();
            let keys: Vec<_> = generations
                .keys()
                .filter(|(id, _)| id == &claim.task_id)
                .cloned()
                .collect();
            for key in keys {
                if let Some(rows) = batches.get_mut(&key) {
                    while batch_budget > 0 && !rows.is_empty() {
                        rows.pop_first();
                        batch_budget -= 1;
                    }
                }
                if batches.get(&key).is_none_or(|b| b.is_empty()) && generation_budget > 0 {
                    batches.remove(&key);
                    generations.remove(&key);
                    generation_budget -= 1;
                }
            }
        }
        {
            let mut projections = self.terminal_projections.write().unwrap();
            let keys: Vec<_> = projections
                .iter()
                .filter(|(_, p)| p.projection.task.id == claim.task_id && p.projected_at.is_some())
                .map(|(k, _)| k.clone())
                .take(limit as usize)
                .collect();
            for key in keys {
                projections.remove(&key);
            }
        }
        if self
            .series
            .read()
            .unwrap()
            .get(&claim.task_id)
            .is_some_and(|s| !s.is_empty())
            || self
                .generations
                .read()
                .unwrap()
                .keys()
                .any(|(id, _)| id == &claim.task_id)
            || self
                .terminal_projections
                .read()
                .unwrap()
                .values()
                .any(|p| p.projection.task.id == claim.task_id)
        {
            return Ok(CleanupBatchResult {
                deleted_events: count,
                complete: false,
            });
        }
        self.cleanup_claims.write().unwrap().remove(&claim.task_id);
        self.cleanup_retry.write().unwrap().remove(&claim.task_id);
        if claim.target == CleanupTarget::All {
            self.tasks.write().unwrap().remove(&claim.task_id);
            self.metadata.write().unwrap().remove(&claim.task_id);
            self.creation_claims.write().unwrap().remove(&claim.task_id);
            self.events.write().unwrap().remove(&claim.task_id);
            self.series.write().unwrap().remove(&claim.task_id);
            self.release_requests
                .write()
                .unwrap()
                .remove(&claim.task_id);
            self.ttl_claims.write().unwrap().remove(&claim.task_id);
        }
        Ok(CleanupBatchResult {
            deleted_events: count,
            complete: true,
        })
    }
}
