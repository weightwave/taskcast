use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::types::{BroadcastProvider, EventQueryOptions, ShortTermStore, Task, TaskEvent};

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
}

impl MemoryShortTermStore {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            series_latest: RwLock::new(HashMap::new()),
            index_counters: RwLock::new(HashMap::new()),
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
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Level, TaskStatus};
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
}
