//! Integration tests for `RedisShortTermStore` trait methods.
//!
//! Each test spins up a fresh Redis via testcontainers and exercises the
//! `ShortTermStore` trait directly (no engine involved).
//!
//! Run with: `cargo test -p taskcast-redis --test short_term_tests`

use taskcast_core::types::{
    EventQueryOptions, Level, SeriesMode, ShortTermStore, SinceCursor, Task, TaskError,
    TaskStatus,
};
use taskcast_core::TaskEvent;
use taskcast_redis::RedisShortTermStore;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;

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
    }
}

/// Create a RedisShortTermStore backed by the given Redis URL.
async fn make_store(redis_url: &str) -> RedisShortTermStore {
    let client = redis::Client::open(redis_url).unwrap();
    let conn = client.get_multiplexed_async_connection().await.unwrap();
    RedisShortTermStore::new(conn, Some("test"))
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
async fn start_redis() -> (testcontainers::ContainerAsync<Redis>, String) {
    let container = Redis::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");
    (container, url)
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
    assert!(events.is_empty(), "get_events for missing task must return empty vec");
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

    assert_eq!(events.len(), 2, "should return events with timestamp > 1200.0");
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

    assert_eq!(events.len(), 2, "limit should be applied after since filter");
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
    assert_eq!(events.len(), 3, "event count should stay at 3 (replaced, not appended)");
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

    store
        .append_event("task-srt", event.clone())
        .await
        .unwrap();

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
