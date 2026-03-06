use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};

use crate::engine::{PublishEventInput, TaskEngine};
use crate::types::{Level, ShortTermStore, TaskStatus};

// ─── Options ─────────────────────────────────────────────────────────────────

pub struct TaskSchedulerOptions {
    pub engine: Arc<TaskEngine>,
    pub short_term_store: Arc<dyn ShortTermStore>,
    /// How often to run checks, in milliseconds. Default: 60_000 (1 minute).
    pub check_interval_ms: u64,
    /// If set, emit `taskcast:cold` for paused tasks older than this (ms).
    pub paused_cold_after_ms: Option<u64>,
    /// If set, emit `taskcast:cold` for blocked tasks older than this (ms).
    pub blocked_cold_after_ms: Option<u64>,
}

// ─── TaskScheduler ──────────────────────────────────────────────────────────

pub struct TaskScheduler {
    engine: Arc<TaskEngine>,
    short_term_store: Arc<dyn ShortTermStore>,
    check_interval_ms: u64,
    paused_cold_after_ms: Option<u64>,
    blocked_cold_after_ms: Option<u64>,
    handle: Option<JoinHandle<()>>,
}

impl TaskScheduler {
    pub fn new(opts: TaskSchedulerOptions) -> Self {
        Self {
            engine: opts.engine,
            short_term_store: opts.short_term_store,
            check_interval_ms: opts.check_interval_ms,
            paused_cold_after_ms: opts.paused_cold_after_ms,
            blocked_cold_after_ms: opts.blocked_cold_after_ms,
            handle: None,
        }
    }

    pub fn start(&mut self) {
        let engine = self.engine.clone();
        let store = self.short_term_store.clone();
        let interval_ms = self.check_interval_ms;
        let paused_cold = self.paused_cold_after_ms;
        let blocked_cold = self.blocked_cold_after_ms;

        self.handle = Some(tokio::spawn(async move {
            let mut ticker = interval(Duration::from_millis(interval_ms));
            loop {
                ticker.tick().await;
                let _ = Self::tick_inner(&engine, &store, paused_cold, blocked_cold).await;
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
            &self.engine,
            &self.short_term_store,
            self.paused_cold_after_ms,
            self.blocked_cold_after_ms,
        )
        .await
    }

    async fn tick_inner(
        engine: &Arc<TaskEngine>,
        store: &Arc<dyn ShortTermStore>,
        paused_cold: Option<u64>,
        blocked_cold: Option<u64>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Self::check_wake_up_timers(engine, store).await?;
        Self::check_cold_demotion(engine, store, paused_cold, blocked_cold).await?;
        Ok(())
    }

    /// Resume blocked tasks whose `resume_at` timestamp has passed.
    async fn check_wake_up_timers(
        engine: &Arc<TaskEngine>,
        store: &Arc<dyn ShortTermStore>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let blocked = store.list_by_status(&[TaskStatus::Blocked]).await?;
        let now = now_millis();

        for task in blocked {
            if let Some(resume_at) = task.resume_at {
                if resume_at <= now {
                    // Transition errors (e.g. concurrent modification) are non-fatal
                    let _ = engine
                        .transition_task(&task.id, TaskStatus::Running, None)
                        .await;
                }
            }
        }

        Ok(())
    }

    /// Emit `taskcast:cold` events for suspended tasks that have been idle too long.
    async fn check_cold_demotion(
        engine: &Arc<TaskEngine>,
        store: &Arc<dyn ShortTermStore>,
        paused_cold: Option<u64>,
        blocked_cold: Option<u64>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if paused_cold.is_none() && blocked_cold.is_none() {
            return Ok(());
        }

        let suspended = store
            .list_by_status(&[TaskStatus::Paused, TaskStatus::Blocked])
            .await?;
        let now = now_millis();

        for task in suspended {
            let threshold = match task.status {
                TaskStatus::Paused => paused_cold,
                TaskStatus::Blocked => blocked_cold,
                _ => None,
            };

            if let Some(threshold_ms) = threshold {
                if now - task.updated_at >= threshold_ms as f64 {
                    let _ = engine
                        .publish_event(
                            &task.id,
                            PublishEventInput {
                                r#type: "taskcast:cold".to_string(),
                                level: Level::Info,
                                data: serde_json::json!({}),
                                series_id: None,
                                series_mode: None,
                                series_acc_field: None,
                            },
                        )
                        .await;
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
