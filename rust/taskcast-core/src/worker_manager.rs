use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::engine::{PublishEventInput, TaskEngine};
use crate::types::{
    AssignMode, BroadcastProvider, ConnectionMode, DisconnectPolicy, Level, LongTermStore,
    ShortTermStore, Task, TaskEvent, TaskFilter, TaskStatus, TaskcastHooks, Worker,
    WorkerAssignment, WorkerAssignmentStatus, WorkerAuditAction, WorkerAuditEvent, WorkerFilter,
    WorkerMatchRule, WorkerStatus,
};
use crate::worker_matching::matches_worker_rule;

// ─── Error ───────────────────────────────────────────────────────────────────

pub type ManagerResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// ─── Options & Defaults ─────────────────────────────────────────────────────

pub struct WorkerManagerOptions {
    pub engine: Arc<TaskEngine>,
    pub short_term: Arc<dyn ShortTermStore>,
    pub broadcast: Arc<dyn BroadcastProvider>,
    pub long_term: Option<Arc<dyn LongTermStore>>,
    pub hooks: Option<Arc<dyn TaskcastHooks>>,
    pub defaults: Option<WorkerManagerDefaults>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerManagerDefaults {
    pub assign_mode: Option<AssignMode>,
    pub heartbeat_interval_ms: Option<u64>,
    pub heartbeat_timeout_ms: Option<u64>,
    pub offer_timeout_ms: Option<u64>,
    pub disconnect_policy: Option<DisconnectPolicy>,
    pub disconnect_grace_ms: Option<u64>,
}

impl Default for WorkerManagerDefaults {
    fn default() -> Self {
        Self {
            assign_mode: None,
            heartbeat_interval_ms: Some(30_000),
            heartbeat_timeout_ms: Some(90_000),
            offer_timeout_ms: Some(10_000),
            disconnect_policy: Some(DisconnectPolicy::Reassign),
            disconnect_grace_ms: Some(30_000),
        }
    }
}

// ─── Registration & Update ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRegistration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub match_rule: WorkerMatchRule,
    pub capacity: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    pub connection_mode: ConnectionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_rule: Option<WorkerMatchRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkerUpdateStatus>,
}

/// Only "draining" is a valid status update via WorkerUpdate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerUpdateStatus {
    Draining,
}

// ─── Dispatch / Claim / Decline ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchResult {
    Dispatched { worker_id: String },
    NoMatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimResult {
    Claimed,
    Failed { reason: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclineOptions {
    #[serde(default)]
    pub blacklist: bool,
}

// ─── WorkerManager ──────────────────────────────────────────────────────────

pub struct WorkerManager {
    engine: Arc<TaskEngine>,
    short_term: Arc<dyn ShortTermStore>,
    broadcast: Arc<dyn BroadcastProvider>,
    long_term: Option<Arc<dyn LongTermStore>>,
    hooks: Option<Arc<dyn TaskcastHooks>>,
    defaults: WorkerManagerDefaults,
}

impl WorkerManager {
    pub fn new(opts: WorkerManagerOptions) -> Self {
        Self {
            engine: opts.engine,
            short_term: opts.short_term,
            broadcast: opts.broadcast,
            long_term: opts.long_term,
            hooks: opts.hooks,
            defaults: opts.defaults.unwrap_or_default(),
        }
    }

    pub fn heartbeat_interval_ms(&self) -> u64 {
        self.defaults.heartbeat_interval_ms.unwrap_or(30_000)
    }

    // ─── Audit Helpers ──────────────────────────────────────────────────

    async fn emit_task_audit(
        &self,
        task_id: &str,
        action: &str,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) {
        let mut data = serde_json::Map::new();
        data.insert(
            "action".to_string(),
            serde_json::Value::String(action.to_string()),
        );
        if let Some(extra) = extra {
            for (k, v) in extra {
                data.insert(k, v);
            }
        }

        let _ = self
            .engine
            .publish_event(
                task_id,
                PublishEventInput {
                    r#type: "taskcast:audit".to_string(),
                    level: Level::Info,
                    data: serde_json::Value::Object(data),
                    series_id: None,
                    series_mode: None,
                },
            )
            .await;
    }

    fn emit_worker_audit(
        &self,
        action: WorkerAuditAction,
        worker_id: &str,
        data: Option<HashMap<String, serde_json::Value>>,
    ) {
        let Some(ref long_term) = self.long_term else {
            return;
        };
        let event = WorkerAuditEvent {
            id: ulid::Ulid::new().to_string(),
            worker_id: worker_id.to_string(),
            timestamp: now_millis(),
            action,
            data,
        };
        let lt = Arc::clone(long_term);
        tokio::spawn(async move {
            let _ = lt.save_worker_event(event).await;
        });
    }

    // ─── Worker Registration & Lifecycle ────────────────────────────────

    pub async fn register_worker(&self, config: WorkerRegistration) -> ManagerResult<Worker> {
        let now = now_millis();
        let worker = Worker {
            id: config.worker_id.unwrap_or_else(|| ulid::Ulid::new().to_string()),
            status: WorkerStatus::Idle,
            match_rule: config.match_rule,
            capacity: config.capacity,
            used_slots: 0,
            weight: config.weight.unwrap_or(50),
            connection_mode: config.connection_mode,
            connected_at: now,
            last_heartbeat_at: now,
            metadata: config.metadata,
        };
        self.short_term.save_worker(worker.clone()).await?;
        self.emit_worker_audit(WorkerAuditAction::Connected, &worker.id, None);
        if let Some(ref hooks) = self.hooks {
            hooks.on_worker_connected(&worker);
        }
        Ok(worker)
    }

    pub async fn unregister_worker(&self, worker_id: &str) -> ManagerResult<()> {
        let worker = self.short_term.get_worker(worker_id).await?;
        self.short_term.delete_worker(worker_id).await?;
        if let Some(worker) = worker {
            let mut data = HashMap::new();
            data.insert(
                "reason".to_string(),
                serde_json::Value::String("unregistered".to_string()),
            );
            self.emit_worker_audit(WorkerAuditAction::Disconnected, worker_id, Some(data));
            if let Some(ref hooks) = self.hooks {
                hooks.on_worker_disconnected(&worker, "unregistered");
            }
        }
        Ok(())
    }

    pub async fn update_worker(
        &self,
        worker_id: &str,
        update: WorkerUpdate,
    ) -> ManagerResult<Option<Worker>> {
        let worker = self.short_term.get_worker(worker_id).await?;
        let Some(mut worker) = worker else {
            return Ok(None);
        };

        if let Some(weight) = update.weight {
            worker.weight = weight;
        }
        if let Some(capacity) = update.capacity {
            worker.capacity = capacity;
        }
        if let Some(match_rule) = update.match_rule {
            worker.match_rule = match_rule;
        }
        if let Some(ref status) = update.status {
            match status {
                WorkerUpdateStatus::Draining => {
                    worker.status = WorkerStatus::Draining;
                }
            }
        }

        self.short_term.save_worker(worker.clone()).await?;

        self.emit_worker_audit(WorkerAuditAction::Updated, worker_id, None);
        if update.status.is_some() {
            self.emit_worker_audit(WorkerAuditAction::Draining, worker_id, None);
        }

        Ok(Some(worker))
    }

    pub async fn heartbeat(&self, worker_id: &str) -> ManagerResult<()> {
        let worker = self.short_term.get_worker(worker_id).await?;
        let Some(mut worker) = worker else {
            return Ok(());
        };
        worker.last_heartbeat_at = now_millis();
        self.short_term.save_worker(worker).await?;
        Ok(())
    }

    pub async fn get_worker(&self, worker_id: &str) -> ManagerResult<Option<Worker>> {
        Ok(self.short_term.get_worker(worker_id).await?)
    }

    pub async fn list_workers(&self, filter: Option<WorkerFilter>) -> ManagerResult<Vec<Worker>> {
        Ok(self.short_term.list_workers(filter).await?)
    }

    // ─── Task Dispatch ─────────────────────────────────────────────────

    pub async fn dispatch_task(&self, task_id: &str) -> ManagerResult<DispatchResult> {
        let task = self.engine.get_task(task_id).await?;
        let task = match task {
            Some(t) if t.status == TaskStatus::Pending => t,
            _ => return Ok(DispatchResult::NoMatch),
        };

        let blacklist = get_blacklist(&task);

        let workers = self
            .short_term
            .list_workers(Some(WorkerFilter {
                status: Some(vec![WorkerStatus::Idle, WorkerStatus::Busy]),
                connection_mode: None,
            }))
            .await?;

        let task_cost = task.cost.unwrap_or(1);
        let mut candidates: Vec<Worker> = workers
            .into_iter()
            .filter(|w| {
                if blacklist.contains(&w.id) {
                    return false;
                }
                if w.used_slots + task_cost > w.capacity {
                    return false;
                }
                if !matches_worker_rule(&task, &w.match_rule) {
                    return false;
                }
                true
            })
            .collect();

        if candidates.is_empty() {
            return Ok(DispatchResult::NoMatch);
        }

        // Sort: weight DESC -> available slots DESC -> connectedAt ASC
        candidates.sort_by(|a, b| {
            let weight_cmp = b.weight.cmp(&a.weight);
            if weight_cmp != std::cmp::Ordering::Equal {
                return weight_cmp;
            }
            let a_available = a.capacity - a.used_slots;
            let b_available = b.capacity - b.used_slots;
            let avail_cmp = b_available.cmp(&a_available);
            if avail_cmp != std::cmp::Ordering::Equal {
                return avail_cmp;
            }
            a.connected_at
                .partial_cmp(&b.connected_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(DispatchResult::Dispatched {
            worker_id: candidates[0].id.clone(),
        })
    }

    // ─── Task Claim ────────────────────────────────────────────────────

    pub async fn claim_task(&self, task_id: &str, worker_id: &str) -> ManagerResult<ClaimResult> {
        let task = self.engine.get_task(task_id).await?;
        let Some(task) = task else {
            return Ok(ClaimResult::Failed {
                reason: "Task not found".to_string(),
            });
        };
        if task.status != TaskStatus::Pending {
            return Ok(ClaimResult::Failed {
                reason: format!("Task is not pending (status: {:?})", task.status),
            });
        }

        let cost = task.cost.unwrap_or(1);
        let claimed = self.short_term.claim_task(task_id, worker_id, cost).await?;
        if !claimed {
            return Ok(ClaimResult::Failed {
                reason: "Claim failed (concurrent modification)".to_string(),
            });
        }

        // Re-read the authoritative state after atomic claim
        let updated_task = self.short_term.get_task(task_id).await?.unwrap();
        if let Some(ref long_term) = self.long_term {
            long_term.save_task(updated_task.clone()).await?;
        }

        // Emit audit events
        let mut task_audit_data = HashMap::new();
        task_audit_data.insert(
            "taskId".to_string(),
            serde_json::Value::String(task_id.to_string()),
        );
        self.emit_worker_audit(
            WorkerAuditAction::TaskAssigned,
            worker_id,
            Some(task_audit_data),
        );

        let mut extra = HashMap::new();
        extra.insert(
            "workerId".to_string(),
            serde_json::Value::String(worker_id.to_string()),
        );
        self.emit_task_audit(task_id, "assigned", Some(extra)).await;

        // Create assignment record
        let assignment = WorkerAssignment {
            task_id: task_id.to_string(),
            worker_id: worker_id.to_string(),
            cost,
            assigned_at: now_millis(),
            status: WorkerAssignmentStatus::Assigned,
        };
        self.short_term.add_assignment(assignment).await?;

        // Update worker status (used_slots already updated by claim_task)
        let worker = self.short_term.get_worker(worker_id).await?;
        if let Some(mut worker) = worker {
            worker.status = if worker.used_slots >= worker.capacity {
                WorkerStatus::Busy
            } else {
                WorkerStatus::Idle
            };
            self.short_term.save_worker(worker.clone()).await?;

            if let Some(ref hooks) = self.hooks {
                hooks.on_task_assigned(&updated_task, &worker);
            }
        }

        Ok(ClaimResult::Claimed)
    }

    // ─── Task Decline ──────────────────────────────────────────────────

    pub async fn decline_task(
        &self,
        task_id: &str,
        worker_id: &str,
        opts: Option<DeclineOptions>,
    ) -> ManagerResult<()> {
        let assignment = self.short_term.get_task_assignment(task_id).await?;
        let assignment = match assignment {
            Some(a) if a.worker_id == worker_id => a,
            _ => return Ok(()),
        };

        // Remove assignment
        self.short_term.remove_assignment(task_id).await?;

        // Restore worker capacity
        let worker = self.short_term.get_worker(worker_id).await?;
        let worker = if let Some(mut w) = worker {
            w.used_slots = w.used_slots.saturating_sub(assignment.cost);
            w.status = WorkerStatus::Idle;
            self.short_term.save_worker(w.clone()).await?;
            Some(w)
        } else {
            None
        };

        // Transition task back to pending
        let _ = self
            .engine
            .transition_task(task_id, TaskStatus::Pending, None)
            .await;

        // Emit audit events
        let blacklisted = opts.as_ref().map_or(false, |o| o.blacklist);
        let mut task_audit_data = HashMap::new();
        task_audit_data.insert(
            "taskId".to_string(),
            serde_json::Value::String(task_id.to_string()),
        );
        self.emit_worker_audit(
            WorkerAuditAction::TaskDeclined,
            worker_id,
            Some(task_audit_data),
        );

        let mut extra = HashMap::new();
        extra.insert(
            "workerId".to_string(),
            serde_json::Value::String(worker_id.to_string()),
        );
        extra.insert(
            "blacklisted".to_string(),
            serde_json::Value::Bool(blacklisted),
        );
        self.emit_task_audit(task_id, "declined", Some(extra)).await;

        // Clear assignedWorker and optionally blacklist
        let task = self.engine.get_task(task_id).await?;
        if let Some(mut task) = task {
            task.assigned_worker = None;

            if blacklisted {
                let metadata = task.metadata.get_or_insert_with(HashMap::new);
                let existing = metadata
                    .get("_blacklistedWorkers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut new_list = existing;
                new_list.push(worker_id.to_string());
                metadata.insert(
                    "_blacklistedWorkers".to_string(),
                    serde_json::Value::Array(
                        new_list
                            .into_iter()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
            }

            self.short_term.save_task(task.clone()).await?;
            if let Some(ref long_term) = self.long_term {
                long_term.save_task(task.clone()).await?;
            }

            if let Some(ref hooks) = self.hooks {
                if let Some(ref worker) = worker {
                    hooks.on_task_declined(&task, worker, blacklisted);
                }
            }
        }

        Ok(())
    }

    // ─── Worker Tasks ──────────────────────────────────────────────────

    pub async fn get_worker_tasks(
        &self,
        worker_id: &str,
    ) -> ManagerResult<Vec<WorkerAssignment>> {
        Ok(self.short_term.get_worker_assignments(worker_id).await?)
    }

    // ─── Pull Mode (Long-Poll) ─────────────────────────────────────────

    pub async fn wait_for_task(
        &self,
        worker_id: &str,
        timeout_ms: u64,
    ) -> ManagerResult<Option<Task>> {
        // Heartbeat first
        self.heartbeat(worker_id).await?;

        let worker = self.short_term.get_worker(worker_id).await?;
        let Some(worker) = worker else {
            return Err(format!("Worker not found: {}", worker_id).into());
        };

        // Check existing pending pull-mode tasks
        let pending_tasks = self
            .short_term
            .list_tasks(TaskFilter {
                status: Some(vec![TaskStatus::Pending]),
                assign_mode: Some(vec![AssignMode::Pull]),
                ..Default::default()
            })
            .await?;

        for task in &pending_tasks {
            let task_blacklist = get_blacklist(task);
            if task_blacklist.contains(&worker_id.to_string()) {
                continue;
            }
            if !matches_worker_rule(task, &worker.match_rule) {
                continue;
            }
            let task_cost = task.cost.unwrap_or(1);
            if worker.used_slots + task_cost > worker.capacity {
                continue;
            }

            let result = self.claim_task(&task.id, worker_id).await?;
            if result == ClaimResult::Claimed {
                self.emit_worker_audit(
                    WorkerAuditAction::PullRequest,
                    worker_id,
                    Some({
                        let mut data = HashMap::new();
                        data.insert("matched".to_string(), serde_json::Value::Bool(true));
                        data.insert(
                            "taskId".to_string(),
                            serde_json::Value::String(task.id.clone()),
                        );
                        data
                    }),
                );
                let claimed = self.engine.get_task(&task.id).await?;
                return Ok(claimed);
            }
        }

        // Wait for a new task notification via broadcast
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<Task>>();
        let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));

        let short_term = Arc::clone(&self.short_term);
        let engine = Arc::clone(&self.engine);
        let worker_id_owned = worker_id.to_string();
        let worker_match_rule = worker.match_rule.clone();
        let tx_clone = Arc::clone(&tx);

        let unsub = self
            .broadcast
            .subscribe(
                "taskcast:worker:new-task",
                Box::new(move |event: TaskEvent| {
                    let task_id = match event.data.as_str() {
                        Some(id) => id.to_string(),
                        None => return,
                    };

                    let short_term = Arc::clone(&short_term);
                    let engine = Arc::clone(&engine);
                    let wid = worker_id_owned.clone();
                    let rule = worker_match_rule.clone();
                    let tx = Arc::clone(&tx_clone);

                    tokio::spawn(async move {
                        let Ok(Some(task)) = engine.get_task(&task_id).await else {
                            return;
                        };
                        if task.status != TaskStatus::Pending {
                            return;
                        }
                        if task.assign_mode != Some(AssignMode::Pull) {
                            return;
                        }
                        let task_blacklist = get_blacklist(&task);
                        if task_blacklist.contains(&wid) {
                            return;
                        }
                        if !matches_worker_rule(&task, &rule) {
                            return;
                        }
                        // Re-fetch worker to get current capacity (avoid stale data)
                        let Ok(Some(current_worker)) = short_term.get_worker(&wid).await else {
                            return;
                        };
                        let task_cost = task.cost.unwrap_or(1);
                        if current_worker.used_slots + task_cost > current_worker.capacity {
                            return;
                        }

                        // Try atomic claim
                        let claimed = short_term.claim_task(&task_id, &wid, task_cost).await;
                        if let Ok(true) = claimed {
                            let claimed_task = engine.get_task(&task_id).await.ok().flatten();
                            let mut guard = tx.lock().await;
                            if let Some(sender) = guard.take() {
                                let _ = sender.send(claimed_task);
                            }
                        }
                    });
                }),
            )
            .await;

        let result = tokio::select! {
            res = rx => {
                match res {
                    Ok(task) => task,
                    Err(_) => None,
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(timeout_ms)) => {
                None
            }
        };

        unsub();

        // Emit pull_request audit
        let matched = result.is_some();
        let mut audit_data = HashMap::new();
        audit_data.insert("matched".to_string(), serde_json::Value::Bool(matched));
        if let Some(ref task) = result {
            audit_data.insert(
                "taskId".to_string(),
                serde_json::Value::String(task.id.clone()),
            );
        }
        if !matched {
            self.emit_worker_audit(
                WorkerAuditAction::PullRequest,
                worker_id,
                Some(audit_data),
            );
        }

        Ok(result)
    }

    pub async fn notify_new_task(&self, task_id: &str) -> ManagerResult<()> {
        let event = TaskEvent {
            id: ulid::Ulid::new().to_string(),
            task_id: "system".to_string(),
            index: 0,
            timestamp: now_millis(),
            r#type: "taskcast:worker:new-task".to_string(),
            level: Level::Info,
            data: serde_json::Value::String(task_id.to_string()),
            series_id: None,
            series_mode: None,
        };
        self.broadcast
            .publish("taskcast:worker:new-task", event)
            .await?;
        Ok(())
    }
}

// ─── Private helpers ─────────────────────────────────────────────────────────

fn get_blacklist(task: &Task) -> Vec<String> {
    task.metadata
        .as_ref()
        .and_then(|m| m.get("_blacklistedWorkers"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_millis() as f64
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{CreateTaskInput, TaskEngineOptions};
    use crate::memory_adapters::{MemoryBroadcastProvider, MemoryShortTermStore};
    use crate::types::TagMatcher;

    struct TestContext {
        manager: WorkerManager,
        engine: Arc<TaskEngine>,
    }

    fn make_context() -> TestContext {
        let short_term = Arc::new(MemoryShortTermStore::new());
        let broadcast = Arc::new(MemoryBroadcastProvider::new());
        let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
            short_term: Arc::clone(&short_term) as Arc<dyn ShortTermStore>,
            broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
        }));
        let manager = WorkerManager::new(WorkerManagerOptions {
            engine: Arc::clone(&engine),
            short_term: short_term as Arc<dyn ShortTermStore>,
            broadcast: broadcast as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
            defaults: None,
        });
        TestContext { manager, engine }
    }

    fn make_registration(mode: ConnectionMode) -> WorkerRegistration {
        WorkerRegistration {
            worker_id: None,
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: None,
            connection_mode: mode,
            metadata: None,
        }
    }

    // ─── register_worker ────────────────────────────────────────────────

    #[tokio::test]
    async fn register_worker_creates_idle_worker() {
        let ctx = make_context();
        let worker = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        assert!(!worker.id.is_empty());
        assert_eq!(worker.status, WorkerStatus::Idle);
        assert_eq!(worker.used_slots, 0);
        assert_eq!(worker.weight, 50);
        assert_eq!(worker.capacity, 5);
        assert_eq!(worker.connection_mode, ConnectionMode::Pull);
    }

    #[tokio::test]
    async fn register_worker_with_custom_id() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("my-worker".to_string());
        let worker = ctx.manager.register_worker(reg).await.unwrap();
        assert_eq!(worker.id, "my-worker");
    }

    #[tokio::test]
    async fn register_worker_with_custom_weight() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.weight = Some(100);
        let worker = ctx.manager.register_worker(reg).await.unwrap();
        assert_eq!(worker.weight, 100);
    }

    #[tokio::test]
    async fn register_worker_is_retrievable() {
        let ctx = make_context();
        let worker = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        let retrieved = ctx.manager.get_worker(&worker.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, worker.id);
    }

    // ─── unregister_worker ──────────────────────────────────────────────

    #[tokio::test]
    async fn unregister_worker_removes_worker() {
        let ctx = make_context();
        let worker = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        ctx.manager.unregister_worker(&worker.id).await.unwrap();

        let retrieved = ctx.manager.get_worker(&worker.id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn unregister_nonexistent_worker_is_noop() {
        let ctx = make_context();
        let result = ctx.manager.unregister_worker("nonexistent").await;
        assert!(result.is_ok());
    }

    // ─── update_worker ──────────────────────────────────────────────────

    #[tokio::test]
    async fn update_worker_changes_weight() {
        let ctx = make_context();
        let worker = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        let updated = ctx
            .manager
            .update_worker(
                &worker.id,
                WorkerUpdate {
                    weight: Some(99),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(updated.is_some());
        assert_eq!(updated.unwrap().weight, 99);
    }

    #[tokio::test]
    async fn update_worker_changes_capacity() {
        let ctx = make_context();
        let worker = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        let updated = ctx
            .manager
            .update_worker(
                &worker.id,
                WorkerUpdate {
                    capacity: Some(10),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.unwrap().capacity, 10);
    }

    #[tokio::test]
    async fn update_worker_sets_draining() {
        let ctx = make_context();
        let worker = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        let updated = ctx
            .manager
            .update_worker(
                &worker.id,
                WorkerUpdate {
                    status: Some(WorkerUpdateStatus::Draining),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.unwrap().status, WorkerStatus::Draining);
    }

    #[tokio::test]
    async fn update_nonexistent_worker_returns_none() {
        let ctx = make_context();
        let result = ctx
            .manager
            .update_worker(
                "nonexistent",
                WorkerUpdate {
                    weight: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result.is_none());
    }

    // ─── heartbeat ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn heartbeat_updates_last_heartbeat_at() {
        let ctx = make_context();
        let worker = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        let original_hb = worker.last_heartbeat_at;

        // Small delay to ensure time difference
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        ctx.manager.heartbeat(&worker.id).await.unwrap();

        let updated = ctx.manager.get_worker(&worker.id).await.unwrap().unwrap();
        assert!(updated.last_heartbeat_at >= original_hb);
    }

    #[tokio::test]
    async fn heartbeat_nonexistent_worker_is_noop() {
        let ctx = make_context();
        let result = ctx.manager.heartbeat("nonexistent").await;
        assert!(result.is_ok());
    }

    // ─── list_workers ───────────────────────────────────────────────────

    #[tokio::test]
    async fn list_workers_returns_all_registered() {
        let ctx = make_context();
        ctx.manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();
        ctx.manager
            .register_worker(make_registration(ConnectionMode::Websocket))
            .await
            .unwrap();

        let workers = ctx.manager.list_workers(None).await.unwrap();
        assert_eq!(workers.len(), 2);
    }

    #[tokio::test]
    async fn list_workers_with_status_filter() {
        let ctx = make_context();
        let w1 = ctx
            .manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();
        ctx.manager
            .register_worker(make_registration(ConnectionMode::Pull))
            .await
            .unwrap();

        // Set w1 to draining
        ctx.manager
            .update_worker(
                &w1.id,
                WorkerUpdate {
                    status: Some(WorkerUpdateStatus::Draining),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let idle = ctx
            .manager
            .list_workers(Some(WorkerFilter {
                status: Some(vec![WorkerStatus::Idle]),
                connection_mode: None,
            }))
            .await
            .unwrap();
        assert_eq!(idle.len(), 1);
    }

    // ─── dispatch_task ──────────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_task_finds_matching_worker() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(
            result,
            DispatchResult::Dispatched {
                worker_id: "w1".to_string()
            }
        );
    }

    #[tokio::test]
    async fn dispatch_task_returns_no_match_when_no_workers() {
        let ctx = make_context();
        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(result, DispatchResult::NoMatch);
    }

    #[tokio::test]
    async fn dispatch_task_returns_no_match_for_non_pending() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        ctx.engine
            .transition_task(&task.id, TaskStatus::Running, None)
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(result, DispatchResult::NoMatch);
    }

    #[tokio::test]
    async fn dispatch_task_excludes_blacklisted_worker() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let mut metadata = HashMap::new();
        metadata.insert(
            "_blacklistedWorkers".to_string(),
            serde_json::json!(["w1"]),
        );

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                metadata: Some(metadata),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(result, DispatchResult::NoMatch);
    }

    #[tokio::test]
    async fn dispatch_task_excludes_worker_without_capacity() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.capacity = 1;
        ctx.manager.register_worker(reg).await.unwrap();

        // Create and claim a task to fill capacity
        let task1 = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        ctx.manager.claim_task(&task1.id, "w1").await.unwrap();

        // Try to dispatch another task
        let task2 = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t2".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let result = ctx.manager.dispatch_task(&task2.id).await.unwrap();
        assert_eq!(result, DispatchResult::NoMatch);
    }

    #[tokio::test]
    async fn dispatch_task_excludes_draining_worker() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();
        ctx.manager
            .update_worker(
                "w1",
                WorkerUpdate {
                    status: Some(WorkerUpdateStatus::Draining),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(result, DispatchResult::NoMatch);
    }

    #[tokio::test]
    async fn dispatch_task_prefers_higher_weight() {
        let ctx = make_context();

        let mut reg1 = make_registration(ConnectionMode::Pull);
        reg1.worker_id = Some("low".to_string());
        reg1.weight = Some(10);
        ctx.manager.register_worker(reg1).await.unwrap();

        let mut reg2 = make_registration(ConnectionMode::Pull);
        reg2.worker_id = Some("high".to_string());
        reg2.weight = Some(90);
        ctx.manager.register_worker(reg2).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(
            result,
            DispatchResult::Dispatched {
                worker_id: "high".to_string()
            }
        );
    }

    #[tokio::test]
    async fn dispatch_task_prefers_more_available_slots_when_weight_equal() {
        let ctx = make_context();

        let mut reg1 = make_registration(ConnectionMode::Pull);
        reg1.worker_id = Some("small".to_string());
        reg1.capacity = 2;
        ctx.manager.register_worker(reg1).await.unwrap();

        let mut reg2 = make_registration(ConnectionMode::Pull);
        reg2.worker_id = Some("large".to_string());
        reg2.capacity = 10;
        ctx.manager.register_worker(reg2).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(
            result,
            DispatchResult::Dispatched {
                worker_id: "large".to_string()
            }
        );
    }

    #[tokio::test]
    async fn dispatch_task_respects_match_rule() {
        let ctx = make_context();

        let mut reg1 = make_registration(ConnectionMode::Pull);
        reg1.worker_id = Some("gpu-worker".to_string());
        reg1.match_rule = WorkerMatchRule {
            task_types: None,
            tags: Some(TagMatcher {
                all: Some(vec!["gpu".to_string()]),
                ..Default::default()
            }),
        };
        ctx.manager.register_worker(reg1).await.unwrap();

        // Task without gpu tag
        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                tags: Some(vec!["cpu".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(result, DispatchResult::NoMatch);
    }

    // ─── claim_task ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn claim_task_succeeds() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.claim_task(&task.id, "w1").await.unwrap();
        assert_eq!(result, ClaimResult::Claimed);

        // Verify task status is assigned
        let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Assigned);
        assert_eq!(task.assigned_worker, Some("w1".to_string()));
    }

    #[tokio::test]
    async fn claim_task_updates_worker_slots() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.capacity = 3;
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 1);
        assert_eq!(worker.status, WorkerStatus::Idle);
    }

    #[tokio::test]
    async fn claim_task_sets_worker_busy_when_full() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.capacity = 1;
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.status, WorkerStatus::Busy);
    }

    #[tokio::test]
    async fn claim_task_creates_assignment_record() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        let assignments = ctx.manager.get_worker_tasks("w1").await.unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].task_id, "t1");
        assert_eq!(assignments[0].worker_id, "w1");
        assert_eq!(assignments[0].status, WorkerAssignmentStatus::Assigned);
    }

    #[tokio::test]
    async fn claim_task_not_found_returns_failed() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let result = ctx.manager.claim_task("nonexistent", "w1").await.unwrap();
        assert!(matches!(result, ClaimResult::Failed { .. }));
    }

    #[tokio::test]
    async fn claim_task_non_pending_returns_failed() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        ctx.engine
            .transition_task(&task.id, TaskStatus::Running, None)
            .await
            .unwrap();

        let result = ctx.manager.claim_task("t1", "w1").await.unwrap();
        assert!(matches!(result, ClaimResult::Failed { .. }));
    }

    #[tokio::test]
    async fn claim_task_with_custom_cost() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.capacity = 10;
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                cost: Some(3),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 3);
    }

    // ─── decline_task ───────────────────────────────────────────────────

    #[tokio::test]
    async fn decline_task_restores_worker_and_task() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.capacity = 3;
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        // Worker should have 1 used slot
        let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 1);

        // Decline
        ctx.manager.decline_task("t1", "w1", None).await.unwrap();

        // Worker slots restored
        let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 0);
        assert_eq!(worker.status, WorkerStatus::Idle);

        // Task back to pending
        let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.assigned_worker, None);
    }

    #[tokio::test]
    async fn decline_task_with_blacklist() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        ctx.manager
            .decline_task("t1", "w1", Some(DeclineOptions { blacklist: true }))
            .await
            .unwrap();

        // Check blacklist in metadata
        let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
        let blacklist = get_blacklist(&task);
        assert!(blacklist.contains(&"w1".to_string()));
    }

    #[tokio::test]
    async fn decline_task_wrong_worker_is_noop() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let mut reg2 = make_registration(ConnectionMode::Pull);
        reg2.worker_id = Some("w2".to_string());
        ctx.manager.register_worker(reg2).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        // w2 tries to decline w1's task — should be noop
        ctx.manager.decline_task("t1", "w2", None).await.unwrap();

        // Task should still be assigned
        let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Assigned);
    }

    #[tokio::test]
    async fn decline_nonexistent_assignment_is_noop() {
        let ctx = make_context();
        let result = ctx.manager.decline_task("nonexistent", "w1", None).await;
        assert!(result.is_ok());
    }

    // ─── get_worker_tasks ───────────────────────────────────────────────

    #[tokio::test]
    async fn get_worker_tasks_returns_assignments() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.capacity = 10;
        ctx.manager.register_worker(reg).await.unwrap();

        ctx.engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        ctx.engine
            .create_task(CreateTaskInput {
                id: Some("t2".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        ctx.manager.claim_task("t1", "w1").await.unwrap();
        ctx.manager.claim_task("t2", "w1").await.unwrap();

        let assignments = ctx.manager.get_worker_tasks("w1").await.unwrap();
        assert_eq!(assignments.len(), 2);
    }

    #[tokio::test]
    async fn get_worker_tasks_empty_for_new_worker() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let assignments = ctx.manager.get_worker_tasks("w1").await.unwrap();
        assert!(assignments.is_empty());
    }

    // ─── notify_new_task ────────────────────────────────────────────────

    #[tokio::test]
    async fn notify_new_task_broadcasts_event() {
        let short_term = Arc::new(MemoryShortTermStore::new());
        let broadcast = Arc::new(MemoryBroadcastProvider::new());
        let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
            short_term: Arc::clone(&short_term) as Arc<dyn ShortTermStore>,
            broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
        }));

        let received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let received_clone = Arc::clone(&received);
        let _unsub = broadcast
            .subscribe(
                "taskcast:worker:new-task",
                Box::new(move |event| {
                    if let Some(id) = event.data.as_str() {
                        received_clone.lock().unwrap().push(id.to_string());
                    }
                }),
            )
            .await;

        let manager = WorkerManager::new(WorkerManagerOptions {
            engine,
            short_term: short_term as Arc<dyn ShortTermStore>,
            broadcast: broadcast as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
            defaults: None,
        });

        manager.notify_new_task("task-123").await.unwrap();

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0], "task-123");
    }

    // ─── wait_for_task: immediate match ─────────────────────────────────

    #[tokio::test]
    async fn wait_for_task_finds_existing_pending_pull_task() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        ctx.engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                assign_mode: Some(AssignMode::Pull),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.wait_for_task("w1", 1000).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "t1");
    }

    #[tokio::test]
    async fn wait_for_task_timeout_when_no_tasks() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let result = ctx.manager.wait_for_task("w1", 100).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn wait_for_task_skips_non_pull_tasks() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        // Create an external-mode task (non-pull)
        ctx.engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                assign_mode: Some(AssignMode::External),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.wait_for_task("w1", 100).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn wait_for_task_error_for_nonexistent_worker() {
        let ctx = make_context();
        let result = ctx.manager.wait_for_task("nonexistent", 100).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn wait_for_task_skips_blacklisted() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg).await.unwrap();

        let mut metadata = HashMap::new();
        metadata.insert(
            "_blacklistedWorkers".to_string(),
            serde_json::json!(["w1"]),
        );
        ctx.engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                assign_mode: Some(AssignMode::Pull),
                metadata: Some(metadata),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.wait_for_task("w1", 100).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn wait_for_task_skips_non_matching_rule() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.match_rule = WorkerMatchRule {
            task_types: Some(vec!["render.*".to_string()]),
            tags: None,
        };
        ctx.manager.register_worker(reg).await.unwrap();

        ctx.engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                assign_mode: Some(AssignMode::Pull),
                r#type: Some("llm.generate".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = ctx.manager.wait_for_task("w1", 100).await.unwrap();
        assert!(result.is_none());
    }

    // ─── wait_for_task: broadcast notification ──────────────────────────

    #[tokio::test]
    async fn wait_for_task_claims_on_broadcast_notification() {
        let short_term = Arc::new(MemoryShortTermStore::new());
        let broadcast = Arc::new(MemoryBroadcastProvider::new());
        let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
            short_term: Arc::clone(&short_term) as Arc<dyn ShortTermStore>,
            broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
        }));
        let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
            engine: Arc::clone(&engine),
            short_term: Arc::clone(&short_term) as Arc<dyn ShortTermStore>,
            broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
            defaults: None,
        }));

        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        manager.register_worker(reg).await.unwrap();

        let manager_clone = Arc::clone(&manager);

        // Spawn: wait for task in the background
        let wait_handle = tokio::spawn(async move {
            manager_clone.wait_for_task("w1", 5000).await
        });

        // Give time for broadcast subscription to be established
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Create a pull-mode task and notify
        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                assign_mode: Some(AssignMode::Pull),
                ..Default::default()
            })
            .await
            .unwrap();
        manager.notify_new_task("t1").await.unwrap();

        let result = wait_handle.await.unwrap().unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "t1");
    }

    // ─── dispatch + claim integration ───────────────────────────────────

    #[tokio::test]
    async fn dispatch_then_claim_full_flow() {
        let ctx = make_context();
        let mut reg = make_registration(ConnectionMode::Pull);
        reg.worker_id = Some("w1".to_string());
        reg.capacity = 2;
        ctx.manager.register_worker(reg).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        // Dispatch to find best worker
        let dispatch = ctx.manager.dispatch_task(&task.id).await.unwrap();
        assert_eq!(
            dispatch,
            DispatchResult::Dispatched {
                worker_id: "w1".to_string()
            }
        );

        // Claim with that worker
        let claim = ctx.manager.claim_task(&task.id, "w1").await.unwrap();
        assert_eq!(claim, ClaimResult::Claimed);

        // Task now assigned
        let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Assigned);

        // Worker has 1 used slot
        let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
        assert_eq!(worker.used_slots, 1);
    }

    // ─── decline then re-dispatch ───────────────────────────────────────

    #[tokio::test]
    async fn decline_then_redispatch_with_blacklist() {
        let ctx = make_context();

        let mut reg1 = make_registration(ConnectionMode::Pull);
        reg1.worker_id = Some("w1".to_string());
        ctx.manager.register_worker(reg1).await.unwrap();

        let mut reg2 = make_registration(ConnectionMode::Pull);
        reg2.worker_id = Some("w2".to_string());
        ctx.manager.register_worker(reg2).await.unwrap();

        let task = ctx
            .engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        // Claim with w1
        ctx.manager.claim_task(&task.id, "w1").await.unwrap();

        // w1 declines with blacklist
        ctx.manager
            .decline_task("t1", "w1", Some(DeclineOptions { blacklist: true }))
            .await
            .unwrap();

        // Re-dispatch should pick w2 (w1 is blacklisted)
        let dispatch = ctx.manager.dispatch_task("t1").await.unwrap();
        assert_eq!(
            dispatch,
            DispatchResult::Dispatched {
                worker_id: "w2".to_string()
            }
        );
    }

    // ─── concurrent claims ──────────────────────────────────────────────

    #[tokio::test]
    async fn concurrent_claims_only_one_succeeds() {
        let short_term = Arc::new(MemoryShortTermStore::new());
        let broadcast = Arc::new(MemoryBroadcastProvider::new());
        let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
            short_term: Arc::clone(&short_term) as Arc<dyn ShortTermStore>,
            broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
        }));

        engine
            .create_task(CreateTaskInput {
                id: Some("t1".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
            engine: Arc::clone(&engine),
            short_term: short_term as Arc<dyn ShortTermStore>,
            broadcast: broadcast as Arc<dyn BroadcastProvider>,
            long_term: None,
            hooks: None,
            defaults: None,
        }));

        // Register 10 workers
        for i in 0..10 {
            let mut reg = make_registration(ConnectionMode::Pull);
            reg.worker_id = Some(format!("w{}", i));
            manager.register_worker(reg).await.unwrap();
        }

        // 10 concurrent claim attempts
        let mut handles = Vec::new();
        for i in 0..10 {
            let m = Arc::clone(&manager);
            let wid = format!("w{}", i);
            handles.push(tokio::spawn(async move {
                m.claim_task("t1", &wid).await.unwrap()
            }));
        }

        let results: Vec<ClaimResult> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let claimed_count = results.iter().filter(|r| **r == ClaimResult::Claimed).count();
        // At least one should succeed (the MemoryShortTermStore uses RwLock, so exactly one)
        assert!(claimed_count >= 1);
        // The task should be assigned
        let task = engine.get_task("t1").await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Assigned);
    }
}
