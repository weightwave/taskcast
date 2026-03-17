use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use taskcast_core::{
    BroadcastProvider, CreateTaskInput, EventQueryOptions, Level, LongTermStore,
    MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput, SeriesMode,
    ShortTermStore, Task, TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus, WorkerAuditEvent,
};
use tokio::sync::{Mutex, RwLock};

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_engine() -> (TaskEngine, Arc<MemoryShortTermStore>, Arc<MemoryBroadcastProvider>) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: store.clone(),
        broadcast: broadcast.clone(),
        long_term_store: None,
        hooks: None,
    });
    (engine, store, broadcast)
}

fn make_engine_with_long_term(
    long_term: Arc<MockLongTermStore>,
) -> (TaskEngine, Arc<MemoryShortTermStore>, Arc<MemoryBroadcastProvider>) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: store.clone(),
        broadcast: broadcast.clone(),
        long_term_store: Some(long_term),
        hooks: None,
    });
    (engine, store, broadcast)
}

async fn create_running_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(CreateTaskInput {
            id: Some(task_id.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(task_id, TaskStatus::Running, None)
        .await
        .unwrap();
}

/// Filter out internal taskcast: events, keep only user-published events.
fn user_events(events: &[TaskEvent]) -> Vec<TaskEvent> {
    events
        .iter()
        .filter(|e| !e.r#type.starts_with("taskcast:"))
        .cloned()
        .collect()
}

// ─── Mock LongTermStore ──────────────────────────────────────────────────────

struct MockLongTermStore {
    events: RwLock<Vec<TaskEvent>>,
}

impl MockLongTermStore {
    fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LongTermStore for MockLongTermStore {
    async fn save_task(&self, _task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_task(
        &self,
        _task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(None)
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.events.write().await.push(event);
        Ok(())
    }

    async fn get_events(
        &self,
        _task_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.events.read().await.clone())
    }

    async fn save_worker_event(
        &self,
        _event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_worker_events(
        &self,
        _worker_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }
}

// ─── latest mode ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn latest_keeps_only_latest_after_5_publishes() {
    let (engine, store, _) = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 0..5 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "value": i }),
                    series_id: Some("pct".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = store.get_events("t1", None).await.unwrap();
    let user = user_events(&events);
    assert_eq!(user.len(), 1);
    assert_eq!(user[0].data, json!({ "value": 4 }));
}

#[tokio::test]
async fn latest_first_event_stored_once() {
    let (engine, store, _) = make_engine();
    create_running_task(&engine, "t1").await;

    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "progress".to_string(),
                level: Level::Info,
                data: json!({ "value": "first" }),
                series_id: Some("pct".to_string()),
                series_mode: Some(SeriesMode::Latest),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let events = store.get_events("t1", None).await.unwrap();
    let user = user_events(&events);
    assert_eq!(user.len(), 1);
    assert_eq!(user[0].data, json!({ "value": "first" }));
}

#[tokio::test]
async fn latest_multiple_series_deduplicated_independently() {
    let (engine, store, _) = make_engine();
    create_running_task(&engine, "t1").await;

    // Publish 3 events to series A
    for i in 0..3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "series": "A", "value": i }),
                    series_id: Some("sA".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    // Publish 3 events to series B
    for i in 0..3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "series": "B", "value": i }),
                    series_id: Some("sB".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = store.get_events("t1", None).await.unwrap();
    let user = user_events(&events);
    assert_eq!(user.len(), 2);

    let series_a = user.iter().find(|e| e.data["series"] == "A").unwrap();
    let series_b = user.iter().find(|e| e.data["series"] == "B").unwrap();
    assert_eq!(series_a.data, json!({ "series": "A", "value": 2 }));
    assert_eq!(series_b.data, json!({ "series": "B", "value": 2 }));
}

#[tokio::test]
async fn latest_broadcast_fires_every_time() {
    let (engine, _, broadcast) = make_engine();
    create_running_task(&engine, "t1").await;

    let received = Arc::new(Mutex::new(Vec::<TaskEvent>::new()));
    let received_clone = received.clone();
    let _unsub = broadcast.subscribe("t1", Box::new(move |event| {
        let received = received_clone.clone();
        tokio::spawn(async move {
            received.lock().await.push(event);
        });
    })).await;

    for i in 0..5 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "value": i }),
                    series_id: Some("pct".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    // Let async broadcast settle
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let received = received.lock().await;
    let user = user_events(&received);
    assert_eq!(user.len(), 5);
}

#[tokio::test]
async fn latest_long_term_store_receives_events() {
    let long_term = Arc::new(MockLongTermStore::new());
    let (engine, _, _) = make_engine_with_long_term(long_term.clone());
    create_running_task(&engine, "t1").await;

    for i in 0..3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "value": i }),
                    series_id: Some("pct".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    // Let async long-term store saves settle
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let events = long_term.events.read().await;
    let user = user_events(&events);
    assert_eq!(user.len(), 3);
}

// ─── keep-all mode ───────────────────────────────────────────────────────────

#[tokio::test]
async fn keep_all_retains_all_5_events() {
    let (engine, store, _) = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 0..5 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "value": i }),
                    series_id: Some("log".to_string()),
                    series_mode: Some(SeriesMode::KeepAll),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = store.get_events("t1", None).await.unwrap();
    let user = user_events(&events);
    assert_eq!(user.len(), 5);
    for i in 0..5 {
        assert_eq!(user[i].data, json!({ "value": i }));
    }
}

// ─── accumulate mode ─────────────────────────────────────────────────────────

#[tokio::test]
async fn accumulate_stores_all_deltas_and_series_latest_is_accumulated() {
    let (engine, store, _) = make_engine();
    create_running_task(&engine, "t1").await;

    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "progress".to_string(),
                level: Level::Info,
                data: json!({ "delta": "hello" }),
                series_id: Some("text".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "progress".to_string(),
                level: Level::Info,
                data: json!({ "delta": " world" }),
                series_id: Some("text".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // All deltas stored
    let events = store.get_events("t1", None).await.unwrap();
    let user = user_events(&events);
    assert_eq!(user.len(), 2);
    assert_eq!(user[0].data, json!({ "delta": "hello" }));
    assert_eq!(user[1].data, json!({ "delta": " world" }));

    // Series latest is the accumulated value
    let latest = store.get_series_latest("t1", "text").await.unwrap();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().data["delta"], "hello world");
}

// ─── mixed modes ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn mixed_all_3_modes_coexist_correctly() {
    let (engine, store, _) = make_engine();
    create_running_task(&engine, "t1").await;

    // latest: 3 publishes -> 1 stored event
    for i in 0..3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "metric".to_string(),
                    level: Level::Info,
                    data: json!({ "value": i }),
                    series_id: Some("metric".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    // keep-all: 3 publishes -> 3 stored events
    for i in 0..3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "log".to_string(),
                    level: Level::Info,
                    data: json!({ "msg": format!("log-{}", i) }),
                    series_id: Some("logs".to_string()),
                    series_mode: Some(SeriesMode::KeepAll),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    // accumulate: 3 publishes -> 3 stored deltas
    for i in 0..3 {
        let ch = (b'a' + i) as char;
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "text".to_string(),
                    level: Level::Info,
                    data: json!({ "delta": ch.to_string() }),
                    series_id: Some("text".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = store.get_events("t1", None).await.unwrap();
    let user = user_events(&events);

    // 1 (latest) + 3 (keep-all) + 3 (accumulate) = 7
    assert_eq!(user.len(), 7);

    // Verify latest kept only last value
    let metric_events: Vec<_> = user.iter().filter(|e| e.r#type == "metric").collect();
    assert_eq!(metric_events.len(), 1);
    assert_eq!(metric_events[0].data, json!({ "value": 2 }));

    // Verify keep-all retained all
    let log_events: Vec<_> = user.iter().filter(|e| e.r#type == "log").collect();
    assert_eq!(log_events.len(), 3);

    // Verify accumulate stored all deltas
    let text_events: Vec<_> = user.iter().filter(|e| e.r#type == "text").collect();
    assert_eq!(text_events.len(), 3);

    // Verify accumulated series latest
    let latest = store.get_series_latest("t1", "text").await.unwrap();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().data["delta"], "abc");
}
