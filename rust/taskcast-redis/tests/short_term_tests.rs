//! Integration tests for `RedisShortTermStore` trait methods.
//!
//! Each test spins up a fresh Redis via testcontainers and exercises the
//! `ShortTermStore` trait directly (no engine involved).
//!
//! Run with: `cargo test -p taskcast-redis --test short_term_tests`

use taskcast_core::types::{
    AssignMode, ConnectionMode, DurableSeriesState, EventQueryOptions, HotWriteToken, Level,
    RehydrateSnapshot, SeriesMode, ShortTermStore, SinceCursor, StorageFenceConflictError,
    StorageIntegrityError, StorageWriterRegistration, Task, TaskError, TaskFilter, TaskStatus,
    Worker, WorkerAssignment, WorkerAssignmentStatus, WorkerFilter, WorkerMatchRule, WorkerStatus,
};
use taskcast_core::TaskEvent;
use taskcast_redis::RedisShortTermStore;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::GenericImage;

// ── Helpers ─────────────────────────────────────────────────────────────────

fn make_task(id: &str) -> Task {
    Task {
        id: id.to_string(),
        r#type: None,
        status: TaskStatus::Pending,
        params: Some(
            [("prompt".to_string(), serde_json::json!("hello"))]
                .into_iter()
                .collect(),
        ),
        result: None,
        error: None,
        metadata: None,
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
        created_at: 1000.0,
        updated_at: 1000.0,
        completed_at: None,
        ttl: None,
    }
}

fn make_event(task_id: &str, index: u64) -> TaskEvent {
    TaskEvent {
        id: format!("evt-{}-{}", task_id, index),
        task_id: task_id.to_string(),
        index,
        timestamp: 1000.0 + index as f64 * 100.0,
        r#type: "llm.delta".to_string(),
        level: Level::Info,
        data: serde_json::json!({"text": format!("msg-{}", index)}),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

/// Create a RedisShortTermStore backed by the given Redis URL.
async fn make_store(redis_url: &str) -> RedisShortTermStore {
    let client = redis::Client::open(redis_url).unwrap();
    let conn = client.get_multiplexed_async_connection().await.unwrap();
    RedisShortTermStore::new(conn, Some("test")).with_legacy_series_writes(false)
}

/// Flush the test Redis instance.
async fn flush_redis(redis_url: &str) {
    let client = redis::Client::open(redis_url).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("FLUSHALL")
        .query_async::<()>(&mut conn)
        .await
        .unwrap();
}

/// Start a Redis container and return (container, redis_url).
async fn start_redis() -> (Option<testcontainers::ContainerAsync<GenericImage>>, String) {
    if let Ok(url) = std::env::var("TASKCAST_TEST_REDIS_URL") {
        return (None, url);
    }
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");
    (Some(container), url)
}

// ── Task CRUD Tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_a_task() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let task = make_task("task-1");
    store.save_task(task.clone()).await.unwrap();

    let retrieved = store.get_task("task-1").await.unwrap();
    assert!(retrieved.is_some(), "task should exist after save");
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, "task-1");
    assert_eq!(retrieved.status, TaskStatus::Pending);
    assert_eq!(retrieved.created_at, 1000.0);
    assert_eq!(retrieved.params, task.params);
}

#[tokio::test]
async fn return_none_for_missing_task() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let result = store.get_task("nonexistent").await.unwrap();
    assert!(result.is_none(), "get_task for missing ID must return None");
}

#[tokio::test]
async fn upsert_task_on_conflict() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let mut task = make_task("task-upsert");
    store.save_task(task.clone()).await.unwrap();

    // Update status and save again
    task.status = TaskStatus::Running;
    task.updated_at = 2000.0;
    store.save_task(task.clone()).await.unwrap();

    let retrieved = store.get_task("task-upsert").await.unwrap().unwrap();
    assert_eq!(
        retrieved.status,
        TaskStatus::Running,
        "second save must overwrite the first"
    );
    assert_eq!(retrieved.updated_at, 2000.0);
}

#[tokio::test]
async fn preserve_optional_fields_on_round_trip() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let task = Task {
        id: "task-full".to_string(),
        r#type: Some("llm.chat".to_string()),
        status: TaskStatus::Completed,
        params: Some(
            [("model".to_string(), serde_json::json!("gpt-4"))]
                .into_iter()
                .collect(),
        ),
        result: Some(
            [("answer".to_string(), serde_json::json!("42"))]
                .into_iter()
                .collect(),
        ),
        error: Some(TaskError {
            code: Some("E001".to_string()),
            message: "something went wrong".to_string(),
            details: Some(
                [("trace".to_string(), serde_json::json!("stack..."))]
                    .into_iter()
                    .collect(),
            ),
        }),
        metadata: Some(
            [("user".to_string(), serde_json::json!("alice"))]
                .into_iter()
                .collect(),
        ),
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
        created_at: 1000.0,
        updated_at: 2000.0,
        completed_at: Some(3000.0),
        ttl: Some(60),
    };

    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("task-full").await.unwrap().unwrap();

    assert_eq!(retrieved.r#type, Some("llm.chat".to_string()));
    assert_eq!(retrieved.status, TaskStatus::Completed);
    assert_eq!(retrieved.result, task.result);
    assert_eq!(retrieved.error, task.error);
    assert_eq!(retrieved.metadata, task.metadata);
    assert_eq!(retrieved.completed_at, Some(3000.0));
    assert_eq!(retrieved.ttl, Some(60));
}

#[tokio::test]
async fn handle_task_with_no_optional_fields() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let task = Task {
        id: "task-minimal".to_string(),
        r#type: None,
        status: TaskStatus::Pending,
        params: None,
        result: None,
        error: None,
        metadata: None,
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
        created_at: 500.0,
        updated_at: 500.0,
        completed_at: None,
        ttl: None,
    };

    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("task-minimal").await.unwrap().unwrap();

    assert_eq!(retrieved.id, "task-minimal");
    assert!(retrieved.r#type.is_none());
    assert!(retrieved.params.is_none());
    assert!(retrieved.result.is_none());
    assert!(retrieved.error.is_none());
    assert!(retrieved.metadata.is_none());
    assert!(retrieved.completed_at.is_none());
    assert!(retrieved.ttl.is_none());
}

// ── Index Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn generate_monotonic_indices_starting_from_0() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let idx0 = store.next_index("task-idx").await.unwrap();
    let idx1 = store.next_index("task-idx").await.unwrap();
    let idx2 = store.next_index("task-idx").await.unwrap();

    assert_eq!(idx0, 0, "first index must be 0");
    assert_eq!(idx1, 1, "second index must be 1");
    assert_eq!(idx2, 2, "third index must be 2");
}

#[tokio::test]
async fn maintain_separate_counters_per_task() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let a0 = store.next_index("task-a").await.unwrap();
    let b0 = store.next_index("task-b").await.unwrap();
    let a1 = store.next_index("task-a").await.unwrap();
    let b1 = store.next_index("task-b").await.unwrap();

    assert_eq!(a0, 0);
    assert_eq!(b0, 0, "task-b counter is independent of task-a");
    assert_eq!(a1, 1);
    assert_eq!(b1, 1);
}

// ── Event Append / Retrieve Tests ───────────────────────────────────────────

#[tokio::test]
async fn append_and_retrieve_events_in_order() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let e0 = make_event("task-ev", 0);
    let e1 = make_event("task-ev", 1);
    let e2 = make_event("task-ev", 2);

    store.append_event("task-ev", e0.clone()).await.unwrap();
    store.append_event("task-ev", e1.clone()).await.unwrap();
    store.append_event("task-ev", e2.clone()).await.unwrap();

    let events = store.get_events("task-ev", None).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].id, e0.id);
    assert_eq!(events[1].id, e1.id);
    assert_eq!(events[2].id, e2.id);
    assert_eq!(events[0].index, 0);
    assert_eq!(events[1].index, 1);
    assert_eq!(events[2].index, 2);
}

#[tokio::test]
async fn return_empty_vec_when_no_events_exist() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let events = store.get_events("no-events-here", None).await.unwrap();
    assert!(
        events.is_empty(),
        "get_events for missing task must return empty vec"
    );
}

// ── Event Filtering Tests ───────────────────────────────────────────────────

#[tokio::test]
async fn filter_events_by_since_index() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for i in 0..5 {
        store
            .append_event("task-si", make_event("task-si", i))
            .await
            .unwrap();
    }

    // since.index = 2 should return events with index > 2
    let events = store
        .get_events(
            "task-si",
            Some(EventQueryOptions {
                since: Some(SinceCursor {
                    id: None,
                    index: Some(2),
                    timestamp: None,
                }),
                limit: None,
            }),
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 2, "should return events with index > 2");
    assert_eq!(events[0].index, 3);
    assert_eq!(events[1].index, 4);
}

#[tokio::test]
async fn filter_events_by_since_timestamp() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for i in 0..5 {
        store
            .append_event("task-st", make_event("task-st", i))
            .await
            .unwrap();
    }

    // Timestamps are 1000, 1100, 1200, 1300, 1400
    // since.timestamp = 1200.0 should return events with timestamp > 1200.0
    let events = store
        .get_events(
            "task-st",
            Some(EventQueryOptions {
                since: Some(SinceCursor {
                    id: None,
                    index: None,
                    timestamp: Some(1200.0),
                }),
                limit: None,
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        events.len(),
        2,
        "should return events with timestamp > 1200.0"
    );
    assert_eq!(events[0].index, 3);
    assert_eq!(events[1].index, 4);
}

#[tokio::test]
async fn filter_events_by_since_id() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for i in 0..5 {
        store
            .append_event("task-sid", make_event("task-sid", i))
            .await
            .unwrap();
    }

    // since.id = "evt-task-sid-2" should return events after that one
    let events = store
        .get_events(
            "task-sid",
            Some(EventQueryOptions {
                since: Some(SinceCursor {
                    id: Some("evt-task-sid-2".to_string()),
                    index: None,
                    timestamp: None,
                }),
                limit: None,
            }),
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 2, "should return events after evt-task-sid-2");
    assert_eq!(events[0].id, "evt-task-sid-3");
    assert_eq!(events[1].id, "evt-task-sid-4");
}

#[tokio::test]
async fn return_all_events_when_since_id_not_found() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for i in 0..3 {
        store
            .append_event("task-nf", make_event("task-nf", i))
            .await
            .unwrap();
    }

    // since.id that doesn't exist should return all events
    let events = store
        .get_events(
            "task-nf",
            Some(EventQueryOptions {
                since: Some(SinceCursor {
                    id: Some("nonexistent-id".to_string()),
                    index: None,
                    timestamp: None,
                }),
                limit: None,
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        events.len(),
        3,
        "should return all events when since.id is not found"
    );
}

#[tokio::test]
async fn respect_limit_parameter() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for i in 0..5 {
        store
            .append_event("task-lim", make_event("task-lim", i))
            .await
            .unwrap();
    }

    let events = store
        .get_events(
            "task-lim",
            Some(EventQueryOptions {
                since: None,
                limit: Some(2),
            }),
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 2, "limit=2 should return only 2 events");
    assert_eq!(events[0].index, 0, "should return the first events");
    assert_eq!(events[1].index, 1);
}

#[tokio::test]
async fn apply_limit_after_since_filter() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for i in 0..10 {
        store
            .append_event("task-sl", make_event("task-sl", i))
            .await
            .unwrap();
    }

    // since.index = 5 gives events at indices 6,7,8,9 -- limit to 2
    let events = store
        .get_events(
            "task-sl",
            Some(EventQueryOptions {
                since: Some(SinceCursor {
                    id: None,
                    index: Some(5),
                    timestamp: None,
                }),
                limit: Some(2),
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        events.len(),
        2,
        "limit should be applied after since filter"
    );
    assert_eq!(events[0].index, 6);
    assert_eq!(events[1].index, 7);
}

// ── Series Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn manage_series_latest_set_and_get() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let event = make_event("task-ser", 0);
    store
        .set_series_latest("task-ser", "progress", event.clone())
        .await
        .unwrap();

    let latest = store
        .get_series_latest("task-ser", "progress")
        .await
        .unwrap();
    assert!(latest.is_some(), "series latest should exist after set");
    let latest = latest.unwrap();
    assert_eq!(latest.id, event.id);
    assert_eq!(latest.task_id, "task-ser");
}

#[tokio::test]
async fn return_none_for_missing_series() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let result = store
        .get_series_latest("task-x", "nonexistent-series")
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "get_series_latest for missing series must return None"
    );
}

#[tokio::test]
async fn update_series_latest_on_conflict() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let e1 = make_event("task-su", 0);
    let e2 = make_event("task-su", 1);

    store
        .set_series_latest("task-su", "progress", e1)
        .await
        .unwrap();
    store
        .set_series_latest("task-su", "progress", e2.clone())
        .await
        .unwrap();

    let latest = store
        .get_series_latest("task-su", "progress")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        latest.id, e2.id,
        "second set_series_latest should overwrite the first"
    );
    assert_eq!(latest.index, 1);
}

#[tokio::test]
async fn replace_last_series_event_in_event_list() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    // Append 3 events; the second one is a series event
    let e0 = make_event("task-rse", 0);
    let mut e1 = make_event("task-rse", 1);
    e1.series_id = Some("progress".to_string());
    e1.series_mode = Some(SeriesMode::Latest);
    let e2 = make_event("task-rse", 2);

    store.append_event("task-rse", e0.clone()).await.unwrap();
    store.append_event("task-rse", e1.clone()).await.unwrap();
    store.append_event("task-rse", e2.clone()).await.unwrap();

    // Set e1 as the series latest
    store
        .set_series_latest("task-rse", "progress", e1.clone())
        .await
        .unwrap();

    // Now replace it with a new version
    let mut replacement = make_event("task-rse", 3);
    replacement.id = "evt-task-rse-replacement".to_string();
    replacement.series_id = Some("progress".to_string());
    replacement.series_mode = Some(SeriesMode::Latest);
    replacement.data = serde_json::json!({"text": "replaced"});

    store
        .replace_last_series_event("task-rse", "progress", replacement.clone())
        .await
        .unwrap();

    // Check that the event list has the replacement in place of e1
    let events = store.get_events("task-rse", None).await.unwrap();
    assert_eq!(
        events.len(),
        3,
        "event count should stay at 3 (replaced, not appended)"
    );
    assert_eq!(events[0].id, e0.id, "first event should be unchanged");
    assert_eq!(
        events[1].id, replacement.id,
        "second event should be the replacement"
    );
    assert_eq!(events[1].data, serde_json::json!({"text": "replaced"}));
    assert_eq!(events[2].id, e2.id, "third event should be unchanged");

    // Also verify series latest was updated
    let latest = store
        .get_series_latest("task-rse", "progress")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, replacement.id);
}

#[tokio::test]
async fn append_when_no_previous_series_event() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    // Append one regular event
    let e0 = make_event("task-anp", 0);
    store.append_event("task-anp", e0.clone()).await.unwrap();

    // replace_last_series_event with no prior series latest should append
    let mut series_event = make_event("task-anp", 1);
    series_event.series_id = Some("new-series".to_string());
    series_event.series_mode = Some(SeriesMode::Accumulate);

    store
        .replace_last_series_event("task-anp", "new-series", series_event.clone())
        .await
        .unwrap();

    let events = store.get_events("task-anp", None).await.unwrap();
    assert_eq!(
        events.len(),
        2,
        "should have appended the series event since no previous existed"
    );
    assert_eq!(events[0].id, e0.id);
    assert_eq!(events[1].id, series_event.id);

    // Series latest should be set
    let latest = store
        .get_series_latest("task-anp", "new-series")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, series_event.id);
}

// ── TTL Tests ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_ttl_sets_expiry_on_keys() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    // Save a task, append an event, set a series latest, and bump the index
    let task = make_task("task-ttl");
    store.save_task(task).await.unwrap();
    store
        .append_event("task-ttl", make_event("task-ttl", 0))
        .await
        .unwrap();
    let _ = store.next_index("task-ttl").await.unwrap();
    store
        .set_series_latest("task-ttl", "ser1", make_event("task-ttl", 1))
        .await
        .unwrap();

    // Set TTL to a large value (300s) -- we just check that the keys have an expiry
    store.set_ttl("task-ttl", 300).await.unwrap();

    // Verify the task key still exists (the TTL hasn't expired yet)
    let task_still_exists = store.get_task("task-ttl").await.unwrap();
    assert!(
        task_still_exists.is_some(),
        "task should still exist immediately after setting TTL"
    );

    // Verify events still exist
    let events = store.get_events("task-ttl", None).await.unwrap();
    assert_eq!(events.len(), 1, "events should still be accessible");

    // Use raw Redis to verify TTL was actually set
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let ttl: i64 = redis::cmd("TTL")
        .arg("test:task:task-ttl")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(
        ttl > 0 && ttl <= 300,
        "task key should have a positive TTL, got {}",
        ttl
    );

    let events_ttl: i64 = redis::cmd("TTL")
        .arg("test:events:task-ttl")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(
        events_ttl > 0 && events_ttl <= 300,
        "events key should have a positive TTL, got {}",
        events_ttl
    );

    let idx_ttl: i64 = redis::cmd("TTL")
        .arg("test:idx:task-ttl")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(
        idx_ttl > 0 && idx_ttl <= 300,
        "idx key should have a positive TTL, got {}",
        idx_ttl
    );
}

// ── Series Event Round-Trip Test ────────────────────────────────────────────

#[tokio::test]
async fn preserve_series_id_and_series_mode_on_events() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let mut event = make_event("task-srt", 0);
    event.series_id = Some("progress-stream".to_string());
    event.series_mode = Some(SeriesMode::Accumulate);

    store.append_event("task-srt", event.clone()).await.unwrap();

    let events = store.get_events("task-srt", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].series_id,
        Some("progress-stream".to_string()),
        "series_id must survive round-trip"
    );
    assert_eq!(
        events[0].series_mode,
        Some(SeriesMode::Accumulate),
        "series_mode must survive round-trip"
    );

    // Also test via set/get_series_latest
    store
        .set_series_latest("task-srt", "progress-stream", event.clone())
        .await
        .unwrap();
    let latest = store
        .get_series_latest("task-srt", "progress-stream")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.series_id, Some("progress-stream".to_string()));
    assert_eq!(latest.series_mode, Some(SeriesMode::Accumulate));
}

// ── Worker Helpers ──────────────────────────────────────────────────────────

fn make_worker(id: &str) -> Worker {
    Worker {
        id: id.to_string(),
        status: WorkerStatus::Idle,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 0,
        weight: 50,
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
        status: WorkerAssignmentStatus::Assigned,
        assigned_at: 1000.0,
    }
}

// ── Worker CRUD Tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_worker() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let worker = make_worker("w-1");
    store.save_worker(worker.clone()).await.unwrap();

    let retrieved = store.get_worker("w-1").await.unwrap();
    assert!(retrieved.is_some(), "worker should exist after save");
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, "w-1");
    assert_eq!(retrieved.status, WorkerStatus::Idle);
    assert_eq!(retrieved.capacity, 5);
    assert_eq!(retrieved.used_slots, 0);
    assert_eq!(retrieved.weight, 50);
    assert_eq!(retrieved.connection_mode, ConnectionMode::Pull);
    assert_eq!(retrieved.connected_at, 1000.0);
    assert_eq!(retrieved.last_heartbeat_at, 1000.0);
    assert!(retrieved.metadata.is_none());
}

#[tokio::test]
async fn get_nonexistent_worker_returns_none() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let result = store.get_worker("nonexistent").await.unwrap();
    assert!(
        result.is_none(),
        "get_worker for missing ID must return None"
    );
}

#[tokio::test]
async fn list_workers_all() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    store.save_worker(make_worker("w-a")).await.unwrap();
    store.save_worker(make_worker("w-b")).await.unwrap();
    store.save_worker(make_worker("w-c")).await.unwrap();

    let workers = store.list_workers(None).await.unwrap();
    assert_eq!(workers.len(), 3, "should return all 3 workers");
}

#[tokio::test]
async fn list_workers_filter_by_status() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let mut w1 = make_worker("w-idle");
    w1.status = WorkerStatus::Idle;
    let mut w2 = make_worker("w-busy");
    w2.status = WorkerStatus::Busy;
    let mut w3 = make_worker("w-draining");
    w3.status = WorkerStatus::Draining;

    store.save_worker(w1).await.unwrap();
    store.save_worker(w2).await.unwrap();
    store.save_worker(w3).await.unwrap();

    let idle_workers = store
        .list_workers(Some(WorkerFilter {
            status: Some(vec![WorkerStatus::Idle]),
            connection_mode: None,
        }))
        .await
        .unwrap();
    assert_eq!(idle_workers.len(), 1);
    assert_eq!(idle_workers[0].id, "w-idle");

    let busy_or_draining = store
        .list_workers(Some(WorkerFilter {
            status: Some(vec![WorkerStatus::Busy, WorkerStatus::Draining]),
            connection_mode: None,
        }))
        .await
        .unwrap();
    assert_eq!(busy_or_draining.len(), 2);
}

#[tokio::test]
async fn list_workers_filter_by_connection_mode() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let mut w1 = make_worker("w-pull");
    w1.connection_mode = ConnectionMode::Pull;
    let mut w2 = make_worker("w-ws");
    w2.connection_mode = ConnectionMode::Websocket;

    store.save_worker(w1).await.unwrap();
    store.save_worker(w2).await.unwrap();

    let ws_workers = store
        .list_workers(Some(WorkerFilter {
            status: None,
            connection_mode: Some(vec![ConnectionMode::Websocket]),
        }))
        .await
        .unwrap();
    assert_eq!(ws_workers.len(), 1);
    assert_eq!(ws_workers[0].id, "w-ws");
}

#[tokio::test]
async fn delete_worker() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    store.save_worker(make_worker("w-del")).await.unwrap();
    assert!(store.get_worker("w-del").await.unwrap().is_some());

    store.delete_worker("w-del").await.unwrap();

    assert!(
        store.get_worker("w-del").await.unwrap().is_none(),
        "worker should be gone after delete"
    );
    let workers = store.list_workers(None).await.unwrap();
    assert!(
        workers.iter().all(|w| w.id != "w-del"),
        "deleted worker should not appear in list"
    );
}

// ── claim_task Tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn claim_task_succeeds_for_pending_task_with_capacity() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let task = make_task("t-claim");
    store.save_task(task).await.unwrap();

    let worker = make_worker("w-claim");
    store.save_worker(worker).await.unwrap();

    let result = store.claim_task("t-claim", "w-claim", 1).await.unwrap();
    assert!(
        result,
        "claim should succeed for pending task with available capacity"
    );

    // Verify task was updated
    let task = store.get_task("t-claim").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
    assert_eq!(task.assigned_worker, Some("w-claim".to_string()));
    assert_eq!(task.cost, Some(1));

    // Verify worker used_slots was incremented
    let worker = store.get_worker("w-claim").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);
}

#[tokio::test]
async fn claim_task_fails_when_worker_has_no_capacity() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let task = make_task("t-nocap");
    store.save_task(task).await.unwrap();

    let mut worker = make_worker("w-nocap");
    worker.capacity = 2;
    worker.used_slots = 2; // Already full
    store.save_worker(worker).await.unwrap();

    let result = store.claim_task("t-nocap", "w-nocap", 1).await.unwrap();
    assert!(
        !result,
        "claim should fail when worker has no remaining capacity"
    );

    // Task should remain pending
    let task = store.get_task("t-nocap").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
}

#[tokio::test]
async fn claim_task_fails_for_non_pending_task() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let mut task = make_task("t-running");
    task.status = TaskStatus::Running;
    store.save_task(task).await.unwrap();

    let worker = make_worker("w-run");
    store.save_worker(worker).await.unwrap();

    let result = store.claim_task("t-running", "w-run", 1).await.unwrap();
    assert!(
        !result,
        "claim should fail for a running (non-pending) task"
    );
}

// ── Assignment Tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn add_and_get_worker_assignments() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let a1 = make_assignment("t-a1", "w-1");
    let a2 = make_assignment("t-a2", "w-1");
    store.add_assignment(a1.clone()).await.unwrap();
    store.add_assignment(a2.clone()).await.unwrap();

    let assignments = store.get_worker_assignments("w-1").await.unwrap();
    assert_eq!(assignments.len(), 2, "worker should have 2 assignments");

    let task_ids: Vec<&str> = assignments.iter().map(|a| a.task_id.as_str()).collect();
    assert!(task_ids.contains(&"t-a1"));
    assert!(task_ids.contains(&"t-a2"));
}

#[tokio::test]
async fn remove_assignment() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let assignment = make_assignment("t-rem", "w-rem");
    store.add_assignment(assignment).await.unwrap();

    // Verify it exists
    let task_assignment = store.get_task_assignment("t-rem").await.unwrap();
    assert!(task_assignment.is_some());

    // Remove and verify
    store.remove_assignment("t-rem").await.unwrap();

    let task_assignment = store.get_task_assignment("t-rem").await.unwrap();
    assert!(
        task_assignment.is_none(),
        "assignment should be gone after remove"
    );

    let worker_assignments = store.get_worker_assignments("w-rem").await.unwrap();
    assert!(
        worker_assignments.is_empty(),
        "worker assignments should be empty after removal"
    );
}

#[tokio::test]
async fn get_task_assignment() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let assignment = make_assignment("t-ga", "w-ga");
    store.add_assignment(assignment.clone()).await.unwrap();

    let retrieved = store.get_task_assignment("t-ga").await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.task_id, "t-ga");
    assert_eq!(retrieved.worker_id, "w-ga");
    assert_eq!(retrieved.status, WorkerAssignmentStatus::Assigned);
    assert_eq!(retrieved.assigned_at, 1000.0);
}

#[tokio::test]
async fn get_task_assignment_returns_none_for_missing() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let result = store.get_task_assignment("nonexistent").await.unwrap();
    assert!(
        result.is_none(),
        "get_task_assignment for missing task must return None"
    );
}

// ── list_tasks Filter Tests ─────────────────────────────────────────────────

#[tokio::test]
async fn list_tasks_filter_by_status() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let mut t1 = make_task("t-pending");
    t1.status = TaskStatus::Pending;
    let mut t2 = make_task("t-running");
    t2.status = TaskStatus::Running;
    let mut t3 = make_task("t-completed");
    t3.status = TaskStatus::Completed;

    store.save_task(t1).await.unwrap();
    store.save_task(t2).await.unwrap();
    store.save_task(t3).await.unwrap();

    let tasks = store
        .list_tasks(TaskFilter {
            status: Some(vec![TaskStatus::Running]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "t-running");
}

#[tokio::test]
async fn list_tasks_filter_by_assign_mode() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let mut t1 = make_task("t-pull");
    t1.assign_mode = Some(AssignMode::Pull);
    let mut t2 = make_task("t-ws");
    t2.assign_mode = Some(AssignMode::WsOffer);
    let t3 = make_task("t-none");
    // no assign_mode

    store.save_task(t1).await.unwrap();
    store.save_task(t2).await.unwrap();
    store.save_task(t3).await.unwrap();

    let tasks = store
        .list_tasks(TaskFilter {
            assign_mode: Some(vec![AssignMode::Pull]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "t-pull");
}

#[tokio::test]
async fn list_tasks_with_limit() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for i in 0..5 {
        store
            .save_task(make_task(&format!("t-lim-{}", i)))
            .await
            .unwrap();
    }

    let tasks = store
        .list_tasks(TaskFilter {
            limit: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 2, "limit=2 should return only 2 tasks");
}

#[tokio::test]
async fn list_tasks_with_exclude_task_ids() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    store.save_task(make_task("t-ex-1")).await.unwrap();
    store.save_task(make_task("t-ex-2")).await.unwrap();
    store.save_task(make_task("t-ex-3")).await.unwrap();

    let tasks = store
        .list_tasks(TaskFilter {
            exclude_task_ids: Some(vec!["t-ex-1".to_string(), "t-ex-3".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "t-ex-2");
}

// ── accumulate_series Tests ─────────────────────────────────────────────────

fn make_accumulate_event(
    task_id: &str,
    index: u64,
    field: &str,
    value: serde_json::Value,
) -> TaskEvent {
    TaskEvent {
        id: format!("evt-{}-{}", task_id, index),
        task_id: task_id.to_string(),
        index,
        timestamp: 1000.0 + index as f64 * 100.0,
        r#type: "llm.chunk".to_string(),
        level: Level::Info,
        data: serde_json::json!({ field: value }),
        series_id: Some("s".to_string()),
        series_mode: Some(SeriesMode::Accumulate),
        series_acc_field: Some(field.to_string()),
        series_snapshot: None,
        _accumulated_data: None,
    }
}

#[tokio::test]
async fn accumulate_series_first_event_returns_unchanged() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let event = make_accumulate_event("task-acc", 0, "delta", serde_json::json!("hello"));

    let result = store
        .accumulate_series("task-acc", "s", event.clone(), "delta")
        .await
        .unwrap();

    assert_eq!(result.data, serde_json::json!({"delta": "hello"}));
    assert_eq!(result.id, event.id);
}

#[tokio::test]
async fn accumulate_series_concatenates_string_field() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let e0 = make_accumulate_event("task-acc2", 0, "delta", serde_json::json!("hello"));
    store
        .accumulate_series("task-acc2", "s", e0, "delta")
        .await
        .unwrap();

    let e1 = make_accumulate_event("task-acc2", 1, "delta", serde_json::json!(" world"));

    let result = store
        .accumulate_series("task-acc2", "s", e1.clone(), "delta")
        .await
        .unwrap();
    assert_eq!(result.data, serde_json::json!({"delta": "hello world"}));
    assert_eq!(result.id, e1.id);
}

#[tokio::test]
async fn accumulate_series_three_events_chain() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for (i, text) in ["A", "B", "C"].iter().enumerate() {
        let e = make_accumulate_event("task-acc3", i as u64, "delta", serde_json::json!(text));
        let r = store
            .accumulate_series("task-acc3", "s", e, "delta")
            .await
            .unwrap();
        if i == 2 {
            assert_eq!(r.data, serde_json::json!({"delta": "ABC"}));
        }
    }
}

#[tokio::test]
async fn accumulate_series_non_string_field_no_concat() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let e0 = make_accumulate_event("task-acc4", 0, "delta", serde_json::json!(42));
    store
        .accumulate_series("task-acc4", "s", e0, "delta")
        .await
        .unwrap();

    let e1 = make_accumulate_event("task-acc4", 1, "delta", serde_json::json!(99));

    let result = store
        .accumulate_series("task-acc4", "s", e1, "delta")
        .await
        .unwrap();
    // Non-string field: no concatenation, returns the new event as-is
    assert_eq!(result.data, serde_json::json!({"delta": 99}));
}

#[tokio::test]
async fn accumulate_series_missing_field_no_concat() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let e0 = make_accumulate_event("task-acc5", 0, "other", serde_json::json!("value"));
    store
        .accumulate_series("task-acc5", "s", e0, "delta")
        .await
        .unwrap();

    let e1 = make_accumulate_event("task-acc5", 1, "other", serde_json::json!("value2"));

    let result = store
        .accumulate_series("task-acc5", "s", e1, "delta")
        .await
        .unwrap();
    // Field "delta" missing from both events — no concatenation
    assert_eq!(result.data, serde_json::json!({"other": "value2"}));
}

#[tokio::test]
async fn accumulate_series_custom_field_name() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let e0 = make_accumulate_event("task-acc6", 0, "content", serde_json::json!("foo"));
    store
        .accumulate_series("task-acc6", "s", e0, "content")
        .await
        .unwrap();

    let e1 = make_accumulate_event("task-acc6", 1, "content", serde_json::json!("bar"));

    let result = store
        .accumulate_series("task-acc6", "s", e1, "content")
        .await
        .unwrap();
    assert_eq!(result.data, serde_json::json!({"content": "foobar"}));
}

#[tokio::test]
async fn accumulate_series_updates_series_latest() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    let e0 = make_accumulate_event("task-acc7", 0, "delta", serde_json::json!("first"));
    store
        .accumulate_series("task-acc7", "s", e0, "delta")
        .await
        .unwrap();

    let e1 = make_accumulate_event("task-acc7", 1, "delta", serde_json::json!("second"));
    store
        .accumulate_series("task-acc7", "s", e1.clone(), "delta")
        .await
        .unwrap();

    let latest = store
        .get_series_latest("task-acc7", "s")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.data, serde_json::json!({"delta": "firstsecond"}));
    assert_eq!(latest.id, e1.id);
}

// ── Hot/cold storage lifecycle ──────────────────────────────────────────────

#[tokio::test]
async fn storage_lifecycle_uses_epoch_fences_and_tokenized_expiring_locks() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();

    assert!(store.supports_hot_cold_release());
    let fence = store.get_write_fence("task-1").await.unwrap().unwrap();
    assert!(fence.accepting_writes);
    assert_eq!(fence.storage_epoch, 1);

    let lease = store
        .acquire_storage_lock("task-1", "owner-a", "generation-a", 1000)
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .acquire_storage_lock("task-1", "owner-b", "generation-b", 1000)
        .await
        .unwrap()
        .is_none());
    let mut stale = lease.clone();
    stale.lock_token = "stale".to_string();
    assert!(!store.renew_storage_lock(&stale, 1000).await.unwrap());
    assert!(!store.release_storage_lock(&stale).await.unwrap());
    assert!(store.renew_storage_lock(&lease, 1000).await.unwrap());
    assert!(store.release_storage_lock(&lease).await.unwrap());
    assert!(!store.release_storage_lock(&lease).await.unwrap());

    let expired = store
        .acquire_storage_lock("task-1", "expiring", "generation-expiring", 30)
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let recovered = store
        .acquire_storage_lock("task-1", "recovered", "generation-recovered", 1000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.lock_token, "recovered");
    assert!(!store.renew_storage_lock(&expired, 1000).await.unwrap());
}

#[tokio::test]
async fn storage_lifecycle_closes_writes_without_consuming_an_index_and_reopens() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let old_token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };

    let first = store
        .commit_event_fenced("task-1", make_event("task-1", 999), &old_token)
        .await
        .unwrap();
    assert_eq!(first.event.index, 0);
    let lease = store
        .acquire_storage_lock("task-1", "owner", "generation", 5000)
        .await
        .unwrap()
        .unwrap();
    let closed = store.close_write_fence(&lease, 1).await.unwrap();
    assert_eq!(closed.high_watermark, 0);

    let error = store
        .commit_event_fenced("task-1", make_event("task-1", 998), &old_token)
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<StorageFenceConflictError>().is_some());
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let index: String = redis::cmd("GET")
        .arg("test:idx:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(index, "1");

    let new_token = store.reopen_write_fence(&lease, 1).await.unwrap();
    assert_eq!(new_token.storage_epoch, 2);
    assert!(store
        .commit_event_fenced("task-1", make_event("task-1", 997), &old_token)
        .await
        .is_err());
    let second = store
        .commit_event_fenced("task-1", make_event("task-1", 996), &new_token)
        .await
        .unwrap();
    assert_eq!(second.event.index, 1);
}

#[tokio::test]
async fn storage_lifecycle_new_generation_adopts_a_closed_fence_for_recovery() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    store
        .commit_event_fenced(
            "task-1",
            make_event("task-1", 0),
            &HotWriteToken {
                task_id: "task-1".to_string(),
                storage_epoch: 1,
            },
        )
        .await
        .unwrap();
    let stale = store
        .acquire_storage_lock("task-1", "stale-owner", "stale-generation", 5_000)
        .await
        .unwrap()
        .unwrap();
    store.close_write_fence(&stale, 1).await.unwrap();
    store.release_storage_lock(&stale).await.unwrap();

    let recovery = store
        .acquire_storage_lock("task-1", "recovery-owner", "recovery-generation", 5_000)
        .await
        .unwrap()
        .unwrap();
    let adopted = store.close_write_fence(&recovery, 1).await.unwrap();
    assert_eq!(
        adopted.active_release_generation.as_deref(),
        Some("recovery-generation")
    );
    assert_eq!(adopted.high_watermark, 0);
    assert!(!store.renew_storage_lock(&stale, 5_000).await.unwrap());
    assert_eq!(
        store
            .reopen_write_fence(&recovery, 1)
            .await
            .unwrap()
            .storage_epoch,
        2
    );
}

#[tokio::test]
async fn storage_lifecycle_commits_series_atomically_and_pages_sparse_history() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let mut keep_zero = make_event("task-1", 100);
    keep_zero.id = "keep-0".to_string();
    store
        .commit_event_fenced("task-1", keep_zero, &token)
        .await
        .unwrap();
    let mut latest_one = make_event("task-1", 101);
    latest_one.id = "latest-1".to_string();
    latest_one.series_id = Some("status".to_string());
    latest_one.series_mode = Some(SeriesMode::Latest);
    store
        .commit_event_fenced("task-1", latest_one, &token)
        .await
        .unwrap();
    let mut keep_two = make_event("task-1", 102);
    keep_two.id = "keep-2".to_string();
    store
        .commit_event_fenced("task-1", keep_two, &token)
        .await
        .unwrap();
    let mut latest_three = make_event("task-1", 103);
    latest_three.id = "latest-3".to_string();
    latest_three.series_id = Some("status".to_string());
    latest_three.series_mode = Some(SeriesMode::Latest);
    store
        .commit_event_fenced("task-1", latest_three, &token)
        .await
        .unwrap();
    let mut first_delta = make_event("task-1", 104);
    first_delta.id = "delta-4".to_string();
    first_delta.series_id = Some("output".to_string());
    first_delta.series_mode = Some(SeriesMode::Accumulate);
    first_delta.series_acc_field = Some("delta".to_string());
    first_delta.data = serde_json::json!({ "delta": "hello" });
    store
        .commit_event_fenced("task-1", first_delta, &token)
        .await
        .unwrap();
    let mut second_delta = make_event("task-1", 105);
    second_delta.id = "delta-5".to_string();
    second_delta.series_id = Some("output".to_string());
    second_delta.series_mode = Some(SeriesMode::Accumulate);
    second_delta.series_acc_field = Some("delta".to_string());
    second_delta.data = serde_json::json!({ "delta": " world" });
    let result = store
        .commit_event_fenced("task-1", second_delta, &token)
        .await
        .unwrap();
    assert_eq!(result.event.data, serde_json::json!({ "delta": " world" }));
    assert_eq!(
        result.accumulated_event.unwrap().data,
        serde_json::json!({ "delta": "hello world" })
    );

    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.index).collect::<Vec<_>>(),
        vec![0, 2, 3, 4, 5]
    );
    assert!(!events.iter().any(|event| event.id == "latest-1"));

    let first_page = store
        .read_archive_source_page("task-1", 5, None, 2)
        .await
        .unwrap();
    assert_eq!(
        first_page
            .events
            .iter()
            .map(|event| event.index)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert_ne!(first_page.next_cursor.as_deref(), Some("2"));
    let second_page = store
        .read_archive_source_page("task-1", 5, first_page.next_cursor.as_deref(), 3)
        .await
        .unwrap();
    assert_eq!(
        second_page
            .events
            .iter()
            .map(|event| event.index)
            .collect::<Vec<_>>(),
        vec![3, 4, 5]
    );
    assert!(second_page.done);
}

#[tokio::test]
async fn storage_lifecycle_preserves_opaque_event_json() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let fragile = serde_json::json!({
        "empty": [],
        "nested": [[], { "empty": [] }],
        "precise": 1.2345678901234567,
        "maxSafe": 9007199254740991_u64,
    });
    let mut keep = make_event("task-1", 100);
    keep.id = "opaque-0".to_string();
    keep.data = fragile.clone();
    let committed = store
        .commit_event_fenced("task-1", keep, &token)
        .await
        .unwrap();
    assert_eq!(committed.event.data, fragile);

    let mut first_delta = make_event("task-1", 101);
    first_delta.id = "opaque-delta-1".to_string();
    first_delta.series_id = Some("output".to_string());
    first_delta.series_mode = Some(SeriesMode::Accumulate);
    first_delta.series_acc_field = Some("delta".to_string());
    first_delta.data = serde_json::json!({
        "empty": [],
        "nested": [[], { "empty": [] }],
        "precise": 1.2345678901234567,
        "maxSafe": 9007199254740991_u64,
        "delta": "hello",
    });
    store
        .commit_event_fenced("task-1", first_delta, &token)
        .await
        .unwrap();
    let mut second_delta = make_event("task-1", 102);
    second_delta.id = "opaque-delta-2".to_string();
    second_delta.series_id = Some("output".to_string());
    second_delta.series_mode = Some(SeriesMode::Accumulate);
    second_delta.series_acc_field = Some("delta".to_string());
    second_delta.data = serde_json::json!({
        "empty": [],
        "nested": [[], { "empty": [] }],
        "precise": 1.2345678901234567,
        "maxSafe": 9007199254740991_u64,
        "delta": " world",
    });
    let accumulated = store
        .commit_event_fenced("task-1", second_delta.clone(), &token)
        .await
        .unwrap();
    assert_eq!(accumulated.event.data, second_delta.data);
    let accumulated_data = accumulated.accumulated_event.unwrap().data;
    assert_eq!(accumulated_data["empty"], serde_json::json!([]));
    assert_eq!(
        accumulated_data["nested"],
        serde_json::json!([[], { "empty": [] }])
    );
    assert_eq!(
        accumulated_data["precise"],
        serde_json::json!(1.2345678901234567)
    );
    assert_eq!(
        accumulated_data["maxSafe"],
        serde_json::json!(9007199254740991_u64)
    );
    assert_eq!(accumulated_data["delta"], "hello world");
}

#[tokio::test]
async fn storage_lifecycle_preserves_accumulation_during_legacy_writer_rollout() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let compatible = RedisShortTermStore::new(
        client.get_multiplexed_async_connection().await.unwrap(),
        Some("test"),
    )
    .with_legacy_series_writes(true);
    let fixed = RedisShortTermStore::new(
        client.get_multiplexed_async_connection().await.unwrap(),
        Some("test"),
    )
    .with_legacy_series_writes(false);
    compatible.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };

    let mut new_a = make_event("task-1", 0);
    new_a.id = "new-a".to_string();
    new_a.series_id = Some("output".to_string());
    new_a.series_mode = Some(SeriesMode::Accumulate);
    new_a.data = serde_json::json!({ "delta": "A" });
    compatible
        .commit_event_fenced("task-1", new_a, &token)
        .await
        .unwrap();

    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let next_index: u64 = redis::cmd("INCR")
        .arg("test:idx:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    let mut old_delta = make_event("task-1", next_index - 1);
    old_delta.id = "old-b".to_string();
    old_delta.series_id = Some("output".to_string());
    old_delta.series_mode = Some(SeriesMode::Accumulate);
    old_delta.data = serde_json::json!({ "delta": "B" });
    let mut old_accumulated = old_delta.clone();
    old_accumulated.data = serde_json::json!({ "delta": "AB" });
    redis::pipe()
        .cmd("RPUSH")
        .arg("test:events:task-1")
        .arg(serde_json::to_string(&old_delta).unwrap())
        .cmd("SET")
        .arg("test:series:task-1:output")
        .arg(serde_json::to_string(&old_accumulated).unwrap())
        .cmd("SADD")
        .arg("test:seriesIds:task-1")
        .arg("output")
        .query_async::<()>(&mut conn)
        .await
        .unwrap();

    let mut new_c = make_event("task-1", 0);
    new_c.id = "new-c".to_string();
    new_c.series_id = Some("output".to_string());
    new_c.series_mode = Some(SeriesMode::Accumulate);
    new_c.data = serde_json::json!({ "delta": "C" });
    let committed = compatible
        .commit_event_fenced("task-1", new_c, &token)
        .await
        .unwrap();
    assert_eq!(
        committed.accumulated_event.unwrap().data,
        serde_json::json!({ "delta": "ABC" })
    );

    let mut fixed_d = make_event("task-1", 0);
    fixed_d.id = "fixed-d".to_string();
    fixed_d.series_id = Some("output".to_string());
    fixed_d.series_mode = Some(SeriesMode::Accumulate);
    fixed_d.data = serde_json::json!({ "delta": "D" });
    let committed = fixed
        .commit_event_fenced("task-1", fixed_d, &token)
        .await
        .unwrap();
    assert_eq!(
        committed.accumulated_event.unwrap().data,
        serde_json::json!({ "delta": "ABCD" })
    );
    let legacy_exists: u64 = redis::cmd("EXISTS")
        .arg("test:series:task-1:output")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(legacy_exists, 0);
    assert_eq!(fixed.get_events("task-1", None).await.unwrap().len(), 4);
}

#[tokio::test]
async fn storage_lifecycle_replaces_latest_event_from_legacy_writer_during_rollout() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let compatible = RedisShortTermStore::new(
        client.get_multiplexed_async_connection().await.unwrap(),
        Some("test"),
    )
    .with_legacy_series_writes(true);
    compatible.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };

    let mut new_a = make_event("task-1", 0);
    new_a.id = "new-a".to_string();
    new_a.series_id = Some("status".to_string());
    new_a.series_mode = Some(SeriesMode::Latest);
    compatible
        .commit_event_fenced("task-1", new_a, &token)
        .await
        .unwrap();

    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let next_index: u64 = redis::cmd("INCR")
        .arg("test:idx:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    let mut old_latest = make_event("task-1", next_index - 1);
    old_latest.id = "old-b".to_string();
    old_latest.series_id = Some("status".to_string());
    old_latest.series_mode = Some(SeriesMode::Latest);
    let old_latest_json = serde_json::to_string(&old_latest).unwrap();
    redis::pipe()
        .cmd("LSET")
        .arg("test:events:task-1")
        .arg(0)
        .arg(&old_latest_json)
        .cmd("SET")
        .arg("test:series:task-1:status")
        .arg(&old_latest_json)
        .cmd("SADD")
        .arg("test:seriesIds:task-1")
        .arg("status")
        .query_async::<()>(&mut conn)
        .await
        .unwrap();

    let mut new_c = make_event("task-1", 0);
    new_c.id = "new-c".to_string();
    new_c.series_id = Some("status".to_string());
    new_c.series_mode = Some(SeriesMode::Latest);
    compatible
        .commit_event_fenced("task-1", new_c, &token)
        .await
        .unwrap();

    let events = compatible.get_events("task-1", None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["new-c"]
    );
    assert_eq!(
        compatible
            .get_series_latest("task-1", "status")
            .await
            .unwrap()
            .unwrap()
            .id,
        "new-c"
    );
}

#[tokio::test]
async fn storage_lifecycle_preserves_the_maximum_writable_safe_index() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let maximum_writable_index = 9_007_199_254_740_990_u64;
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("SET")
        .arg("test:idx:task-1")
        .arg(maximum_writable_index.to_string())
        .query_async::<()>(&mut conn)
        .await
        .unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };

    let committed = store
        .commit_event_fenced("task-1", make_event("task-1", 0), &token)
        .await
        .unwrap();
    assert_eq!(committed.event.index, maximum_writable_index);
    let next_index: String = redis::cmd("GET")
        .arg("test:idx:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(next_index, "9007199254740991");
    let raw_event: String = redis::cmd("LINDEX")
        .arg("test:events:task-1")
        .arg(0)
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(raw_event.contains("\"index\":9007199254740990"));
    let round_trip: TaskEvent = serde_json::from_str(&raw_event).unwrap();
    assert_eq!(round_trip.index, maximum_writable_index);
    let hot_window: String = redis::cmd("GET")
        .arg("test:hotWindow:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(
        hot_window,
        "{\"firstIndex\":9007199254740990,\"lastIndex\":9007199254740990}"
    );

    let error = store
        .commit_event_fenced("task-1", make_event("task-1", 1), &token)
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<StorageIntegrityError>().is_some());
    let event_count: u64 = redis::cmd("LLEN")
        .arg("test:events:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(event_count, 1);
    let next_index: String = redis::cmd("GET")
        .arg("test:idx:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(next_index, "9007199254740991");

    let lease = store
        .acquire_storage_lock("task-1", "owner", "generation", 5000)
        .await
        .unwrap()
        .unwrap();
    let closed = store.close_write_fence(&lease, 1).await.unwrap();
    assert_eq!(closed.high_watermark, maximum_writable_index as i64);
}

#[tokio::test]
async fn storage_lifecycle_uses_fixed_keys_for_large_series_cardinality() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };

    for index in 0..200 {
        let mut event = make_event("task-1", index);
        event.id = format!("series-{index}");
        event.series_id = Some(format!("series-{index}"));
        event.series_mode = Some(SeriesMode::Latest);
        store
            .commit_event_fenced("task-1", event, &token)
            .await
            .unwrap();
    }

    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let series_count: u64 = redis::cmd("HLEN")
        .arg("test:seriesState:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(series_count, 200);
    let legacy_keys: Vec<String> = redis::cmd("KEYS")
        .arg("test:series:task-1:*")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(legacy_keys.is_empty());
    let legacy_set: u64 = redis::cmd("EXISTS")
        .arg("test:seriesIds:task-1")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(legacy_set, 0);
}

#[tokio::test]
async fn storage_lifecycle_rejects_an_out_of_order_archive_source() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();

    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("RPUSH")
        .arg("test:events:task-1")
        .arg(serde_json::to_string(&make_event("task-1", 2)).unwrap())
        .arg(serde_json::to_string(&make_event("task-1", 1)).unwrap())
        .query_async::<()>(&mut conn)
        .await
        .unwrap();

    let error = store
        .read_archive_source_page("task-1", 2, None, 10)
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<StorageIntegrityError>().is_some());
}

#[tokio::test]
async fn storage_lifecycle_rejects_stale_delete_and_removes_all_task_keys() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let mut latest = make_event("task-1", 100);
    latest.series_id = Some("status".to_string());
    latest.series_mode = Some(SeriesMode::Latest);
    store
        .commit_event_fenced("task-1", latest, &token)
        .await
        .unwrap();
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("SET")
        .arg("test:series:task-1:orphan")
        .arg(serde_json::to_string(&make_event("task-1", 0)).unwrap())
        .query_async::<()>(&mut conn)
        .await
        .unwrap();
    let lease = store
        .acquire_storage_lock("task-1", "owner", "generation", 5000)
        .await
        .unwrap()
        .unwrap();
    store.close_write_fence(&lease, 1).await.unwrap();

    let mut stale = lease.clone();
    stale.lock_token = "stale".to_string();
    assert!(store.delete_task_storage_fenced(&stale, 1).await.is_err());
    let before_delete = store.get_task_storage_presence("task-1").await.unwrap();
    assert!(before_delete.task);
    assert_eq!(before_delete.series_state_count, 2);

    store.delete_task_storage_fenced(&lease, 1).await.unwrap();
    let presence = store.get_task_storage_presence("task-1").await.unwrap();
    assert!(!presence.task);
    assert_eq!(presence.event_count, 0);
    assert!(!presence.next_index);
    assert_eq!(presence.series_state_count, 0);
    assert!(!presence.write_fence);
    let remaining: u64 = redis::cmd("EXISTS")
        .arg("test:hotWindow:task-1")
        .arg("test:seriesIds:task-1")
        .arg("test:series:task-1:orphan")
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    assert!(store.reopen_write_fence(&lease, 1).await.is_err());
}

#[tokio::test]
async fn storage_lifecycle_isolates_wildcard_and_prefix_colliding_task_ids() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;

    for task_id in ["*", "victim", "parent", "parent:child"] {
        store.save_task(make_task(task_id)).await.unwrap();
        let mut event = make_event(task_id, 0);
        event.id = format!("{task_id}-latest");
        event.series_id = Some("status".to_string());
        event.series_mode = Some(SeriesMode::Latest);
        store
            .commit_event_fenced(
                task_id,
                event,
                &HotWriteToken {
                    task_id: task_id.to_string(),
                    storage_epoch: 1,
                },
            )
            .await
            .unwrap();
    }

    for task_id in ["*", "parent"] {
        let lease = store
            .acquire_storage_lock(
                task_id,
                &format!("{task_id}-owner"),
                &format!("{task_id}-generation"),
                5000,
            )
            .await
            .unwrap()
            .unwrap();
        store.close_write_fence(&lease, 1).await.unwrap();
        store.delete_task_storage_fenced(&lease, 1).await.unwrap();
    }

    assert_eq!(
        store
            .get_series_latest("victim", "status")
            .await
            .unwrap()
            .unwrap()
            .id,
        "victim-latest"
    );
    assert_eq!(
        store
            .get_series_latest("parent:child", "status")
            .await
            .unwrap()
            .unwrap()
            .id,
        "parent:child-latest"
    );
    assert_eq!(
        store
            .get_task_storage_presence("victim")
            .await
            .unwrap()
            .series_state_count,
        1
    );
    assert_eq!(
        store
            .get_task_storage_presence("parent:child")
            .await
            .unwrap()
            .series_state_count,
        1
    );
}

#[tokio::test]
async fn storage_lifecycle_restores_a_bounded_window_and_preserves_the_next_index() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let lease = store
        .acquire_storage_lock("task-1", "owner", "generation", 5000)
        .await
        .unwrap()
        .unwrap();
    store.close_write_fence(&lease, 1).await.unwrap();
    store.delete_task_storage_fenced(&lease, 1).await.unwrap();

    let mut event_seven = make_event("task-1", 7);
    event_seven.id = "event-7".to_string();
    let mut event_nine = make_event("task-1", 9);
    event_nine.id = "event-9".to_string();
    event_nine.series_id = Some("status".to_string());
    event_nine.series_mode = Some(SeriesMode::Latest);
    event_nine.data = serde_json::json!({
        "status": "ready",
        "empty": [],
        "nested": [[], { "empty": [] }],
        "precise": 1.2345678901234567,
        "maxSafe": 9007199254740991_u64,
    });
    let snapshot = RehydrateSnapshot {
        task: make_task("task-1"),
        archive_watermark: 9,
        max_event_index: 9,
        replay_events: vec![event_seven.clone(), event_nine.clone()],
        series_latest: vec![DurableSeriesState {
            task_id: "task-1".to_string(),
            series_id: "status".to_string(),
            mode: SeriesMode::Latest,
            event: event_nine,
            through_index: 9,
        }],
        storage_epoch: 1,
    };
    let token = store
        .restore_hot_task_fenced(snapshot.clone(), &lease, 2)
        .await
        .unwrap();
    assert_eq!(token.storage_epoch, 2);
    assert_eq!(
        store.get_events("task-1", None).await.unwrap(),
        snapshot.replay_events
    );
    let committed = store
        .commit_event_fenced("task-1", make_event("task-1", 999), &token)
        .await
        .unwrap();
    assert_eq!(committed.event.index, 10);

    let mut stale = lease.clone();
    stale.lock_token = "stale".to_string();
    assert!(store
        .restore_hot_task_fenced(snapshot.clone(), &stale, 2)
        .await
        .is_err());
    assert_eq!(
        store
            .restore_hot_task_fenced(snapshot, &lease, 2)
            .await
            .unwrap(),
        token
    );
    assert_eq!(store.get_events("task-1", None).await.unwrap().len(), 3);
}

#[tokio::test]
async fn storage_lifecycle_commits_task_and_derived_events_atomically() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let revision = store
        .get_task_mutation_snapshot("task-1")
        .await
        .unwrap()
        .unwrap()
        .revision;
    let mut blocked = make_task("task-1");
    blocked.status = TaskStatus::Blocked;
    blocked.reason = Some("approval".to_string());
    let mut status = make_event("task-1", 999);
    status.r#type = "taskcast:status".to_string();
    status.series_id = None;
    status.series_mode = None;
    let mut blocked_event = make_event("task-1", 999);
    blocked_event.r#type = "taskcast:blocked".to_string();
    blocked_event.series_id = None;
    blocked_event.series_mode = None;
    let committed = store
        .commit_task_events_fenced(blocked, &revision, vec![status, blocked_event], &token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        committed
            .iter()
            .map(|event| event.index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        store.get_task("task-1").await.unwrap().unwrap().status,
        TaskStatus::Blocked
    );
    assert_eq!(store.get_events("task-1", None).await.unwrap(), committed);

    let lease = store
        .acquire_storage_lock("task-1", "owner", "generation", 5_000)
        .await
        .unwrap()
        .unwrap();
    store.close_write_fence(&lease, 1).await.unwrap();
    let mut completed = make_task("task-1");
    completed.status = TaskStatus::Completed;
    let error = store
        .commit_task_events_fenced(
            completed,
            &store
                .get_task_mutation_snapshot("task-1")
                .await
                .unwrap()
                .unwrap()
                .revision,
            vec![make_event("task-1", 999)],
            &token,
        )
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<StorageFenceConflictError>().is_some());
    assert_eq!(
        store.get_task("task-1").await.unwrap().unwrap().status,
        TaskStatus::Blocked
    );
    assert_eq!(store.get_events("task-1", None).await.unwrap().len(), 2);
}

#[tokio::test]
async fn storage_lifecycle_allows_only_one_transition_from_the_same_status() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let revision = store
        .get_task_mutation_snapshot("task-1")
        .await
        .unwrap()
        .unwrap()
        .revision;
    let mut completed = make_task("task-1");
    completed.status = TaskStatus::Completed;
    let mut failed = make_task("task-1");
    failed.status = TaskStatus::Failed;
    let first = store.commit_task_events_fenced(
        completed,
        &revision,
        vec![make_event("task-1", 999)],
        &token,
    );
    let second =
        store.commit_task_events_fenced(failed, &revision, vec![make_event("task-1", 999)], &token);

    let (first, second) = tokio::join!(first, second);
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_some()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_none()).count(), 1);
    assert_eq!(store.get_events("task-1", None).await.unwrap().len(), 1);
    assert!(matches!(
        store.get_task("task-1").await.unwrap().unwrap().status,
        TaskStatus::Completed | TaskStatus::Failed
    ));
}

#[tokio::test]
async fn storage_lifecycle_rejects_stale_revision_after_same_status_reclaim() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store.save_task(make_task("task-1")).await.unwrap();
    store.save_worker(make_worker("worker-a")).await.unwrap();
    store.save_worker(make_worker("worker-b")).await.unwrap();
    assert!(store.claim_task("task-1", "worker-a", 1).await.unwrap());
    let snapshot = store
        .get_task_mutation_snapshot("task-1")
        .await
        .unwrap()
        .unwrap();
    assert!(store.claim_task("task-1", "worker-b", 1).await.unwrap());
    let mut running = snapshot.task;
    running.status = TaskStatus::Running;
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let result = store
        .commit_task_events_fenced(
            running,
            &snapshot.revision,
            vec![make_event("task-1", 999)],
            &token,
        )
        .await
        .unwrap();

    assert!(result.is_none());
    let current = store.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(current.status, TaskStatus::Assigned);
    assert_eq!(current.assigned_worker.as_deref(), Some("worker-b"));
    assert!(store.get_events("task-1", None).await.unwrap().is_empty());
}

#[tokio::test]
async fn storage_writer_readiness_expires_without_a_heartbeat() {
    let (_container, redis_url) = start_redis().await;
    flush_redis(&redis_url).await;
    let store = make_store(&redis_url).await;
    store
        .register_storage_writer(
            StorageWriterRegistration {
                instance_id: "writer-a".to_string(),
                storage_protocol_version: 1,
                build: "test-build".to_string(),
                expires_at: 0.0,
            },
            40,
        )
        .await
        .unwrap();
    let writers = store.list_storage_writers().await.unwrap();
    assert_eq!(writers.len(), 1);
    assert_eq!(writers[0].instance_id, "writer-a");
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert!(store.list_storage_writers().await.unwrap().is_empty());
}
