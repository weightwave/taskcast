use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::types::{
    BroadcastProvider, EventQueryOptions, ShortTermStore, Task, TaskEvent, TaskFilter, TaskStatus,
    Worker, WorkerAssignment, WorkerFilter,
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

        Box::new(move || {
            let mut listeners = listeners.write().unwrap();
            if let Some(handlers) = listeners.get_mut(&channel) {
                handlers.retain(|h| (Arc::as_ptr(h) as *const () as usize) != handler_addr);
            }
        })
    }
}

// ─── MemoryShortTermStore ───────────────────────────────────────────────────

pub struct MemoryShortTermStore {
    tasks: RwLock<HashMap<String, Task>>,
    events: RwLock<HashMap<String, Vec<TaskEvent>>>,
    series_latest: RwLock<HashMap<String, TaskEvent>>,
    index_counters: RwLock<HashMap<String, Arc<AtomicU64>>>,
    workers: RwLock<HashMap<String, Worker>>,
    assignments: RwLock<Vec<WorkerAssignment>>,
}

impl MemoryShortTermStore {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            series_latest: RwLock::new(HashMap::new()),
            index_counters: RwLock::new(HashMap::new()),
            workers: RwLock::new(HashMap::new()),
            assignments: RwLock::new(Vec::new()),
        }
    }
}

impl Default for MemoryShortTermStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ShortTermStore for MemoryShortTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut tasks = self.tasks.write().unwrap();
        tasks.insert(task.id.clone(), task);
        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let tasks = self.tasks.read().unwrap();
        Ok(tasks.get(task_id).cloned())
    }

    async fn append_event(
        &self,
        task_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut events = self.events.write().unwrap();
        events
            .entry(task_id.to_string())
            .or_default()
            .push(event);
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
        store
            .set_series_latest("t1", "s1", e2)
            .await
            .unwrap();

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

        store.add_assignment(make_assignment("t1", "w1")).await.unwrap();
        store.add_assignment(make_assignment("t2", "w1")).await.unwrap();
        store.add_assignment(make_assignment("t3", "w2")).await.unwrap();

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

        store.add_assignment(make_assignment("t1", "w1")).await.unwrap();
        store.add_assignment(make_assignment("t2", "w2")).await.unwrap();

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
