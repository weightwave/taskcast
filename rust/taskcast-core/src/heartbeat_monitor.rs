use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{sleep, interval, Duration};

use crate::engine::{TaskEngine, TransitionPayload};
use crate::types::{
    DisconnectPolicy, ShortTermStore, TaskError, TaskStatus, WorkerFilter, WorkerStatus,
};
use crate::worker_manager::WorkerManager;

// ─── Options ─────────────────────────────────────────────────────────────────

pub struct HeartbeatMonitorOptions {
    pub worker_manager: Arc<WorkerManager>,
    pub engine: Arc<TaskEngine>,
    pub short_term_store: Arc<dyn ShortTermStore>,
    /// How often to check heartbeats, in milliseconds. Default: 30_000.
    pub check_interval_ms: u64,
    /// How long before a worker is considered timed out, in milliseconds. Default: 90_000.
    pub heartbeat_timeout_ms: u64,
    /// Default disconnect policy when a task has no explicit policy. Default: Reassign.
    pub default_disconnect_policy: DisconnectPolicy,
    /// Grace period before reassigning, in milliseconds. Default: 30_000.
    pub disconnect_grace_ms: u64,
}

// ─── HeartbeatMonitor ───────────────────────────────────────────────────────

pub struct HeartbeatMonitor {
    worker_manager: Arc<WorkerManager>,
    engine: Arc<TaskEngine>,
    short_term_store: Arc<dyn ShortTermStore>,
    check_interval_ms: u64,
    heartbeat_timeout_ms: u64,
    default_disconnect_policy: DisconnectPolicy,
    disconnect_grace_ms: u64,
    handle: Option<JoinHandle<()>>,
    /// Set of worker IDs currently in a grace period (pending reassignment).
    grace_workers: Arc<RwLock<HashSet<String>>>,
}

impl HeartbeatMonitor {
    pub fn new(opts: HeartbeatMonitorOptions) -> Self {
        Self {
            worker_manager: opts.worker_manager,
            engine: opts.engine,
            short_term_store: opts.short_term_store,
            check_interval_ms: opts.check_interval_ms,
            heartbeat_timeout_ms: opts.heartbeat_timeout_ms,
            default_disconnect_policy: opts.default_disconnect_policy,
            disconnect_grace_ms: opts.disconnect_grace_ms,
            handle: None,
            grace_workers: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub fn start(&mut self) {
        let worker_manager = self.worker_manager.clone();
        let engine = self.engine.clone();
        let store = self.short_term_store.clone();
        let interval_ms = self.check_interval_ms;
        let timeout_ms = self.heartbeat_timeout_ms;
        let default_policy = self.default_disconnect_policy.clone();
        let grace_ms = self.disconnect_grace_ms;
        let grace_workers = self.grace_workers.clone();

        self.handle = Some(tokio::spawn(async move {
            let mut ticker = interval(Duration::from_millis(interval_ms));
            loop {
                ticker.tick().await;
                let _ = Self::tick_inner(
                    &worker_manager,
                    &engine,
                    &store,
                    timeout_ms,
                    &default_policy,
                    grace_ms,
                    &grace_workers,
                )
                .await;
            }
        }));
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    /// Public tick for testing — runs one check cycle immediately.
    pub async fn tick(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Self::tick_inner(
            &self.worker_manager,
            &self.engine,
            &self.short_term_store,
            self.heartbeat_timeout_ms,
            &self.default_disconnect_policy,
            self.disconnect_grace_ms,
            &self.grace_workers,
        )
        .await
    }

    async fn tick_inner(
        worker_manager: &Arc<WorkerManager>,
        engine: &Arc<TaskEngine>,
        store: &Arc<dyn ShortTermStore>,
        timeout_ms: u64,
        default_policy: &DisconnectPolicy,
        grace_ms: u64,
        grace_workers: &Arc<RwLock<HashSet<String>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let workers = store
            .list_workers(Some(WorkerFilter {
                status: Some(vec![
                    WorkerStatus::Idle,
                    WorkerStatus::Busy,
                    WorkerStatus::Draining,
                ]),
                connection_mode: None,
            }))
            .await?;

        let now = now_millis();

        for worker in workers {
            if now - worker.last_heartbeat_at > timeout_ms as f64 {
                // Skip workers already in grace period
                if grace_workers.read().await.contains(&worker.id) {
                    continue;
                }

                Self::handle_timeout(
                    worker_manager,
                    engine,
                    store,
                    &worker.id,
                    default_policy,
                    grace_ms,
                    grace_workers,
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn handle_timeout(
        worker_manager: &Arc<WorkerManager>,
        engine: &Arc<TaskEngine>,
        store: &Arc<dyn ShortTermStore>,
        worker_id: &str,
        default_policy: &DisconnectPolicy,
        grace_ms: u64,
        grace_workers: &Arc<RwLock<HashSet<String>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Mark worker offline
        let worker = store.get_worker(worker_id).await?;
        let Some(mut worker) = worker else {
            return Ok(());
        };
        worker.status = WorkerStatus::Offline;
        store.save_worker(worker).await?;

        // Get all current assignments for this worker
        let assignments = worker_manager.get_worker_tasks(worker_id).await?;

        for assignment in assignments {
            let task = engine.get_task(&assignment.task_id).await?;
            let Some(task) = task else { continue };

            let policy = task
                .disconnect_policy
                .clone()
                .unwrap_or_else(|| default_policy.clone());

            match policy {
                DisconnectPolicy::Fail => {
                    let _ = engine
                        .transition_task(
                            &assignment.task_id,
                            TaskStatus::Failed,
                            Some(TransitionPayload {
                                error: Some(TaskError {
                                    code: Some("WORKER_DISCONNECT".to_string()),
                                    message: format!(
                                        "Worker {} disconnected (heartbeat timeout)",
                                        worker_id
                                    ),
                                    details: None,
                                }),
                                ..Default::default()
                            }),
                        )
                        .await;
                    let _ = worker_manager.release_task(&assignment.task_id).await;
                }
                DisconnectPolicy::Mark => {
                    // Worker is already marked offline above; nothing else to do.
                }
                DisconnectPolicy::Reassign => {
                    // Start grace timer
                    grace_workers.write().await.insert(worker_id.to_string());

                    let wm = worker_manager.clone();
                    let eng = engine.clone();
                    let store_clone = store.clone();
                    let wid = worker_id.to_string();
                    let tid = assignment.task_id.clone();
                    let grace_set = grace_workers.clone();

                    tokio::spawn(async move {
                        sleep(Duration::from_millis(grace_ms)).await;
                        grace_set.write().await.remove(&wid);

                        // Check if worker came back during grace period
                        if let Ok(Some(w)) = store_clone.get_worker(&wid).await {
                            if w.status != WorkerStatus::Offline {
                                return;
                            }
                        }

                        // Worker is still offline — reassign the task
                        let _ = eng
                            .transition_task(&tid, TaskStatus::Pending, None)
                            .await;
                        let _ = wm.release_task(&tid).await;
                    });
                }
            }
        }

        Ok(())
    }
}

fn now_millis() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_millis() as f64
}
