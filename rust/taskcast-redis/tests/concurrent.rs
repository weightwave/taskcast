//! Concurrent correctness tests against real Redis (via testcontainers).
//!
//! These tests verify distributed invariants that only surface when multiple
//! engine instances share the same Redis backend. They require Docker.
//!
//! Run with: `cargo test -p taskcast-redis --test concurrent`
//! Skip if Docker unavailable: tests will fail with connection errors.

mod support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::TcpFaultProxy;
use taskcast_core::{
    BroadcastProvider, CreateTaskInput, Level, MemoryBroadcastProvider, PublishEventInput,
    TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus,
};
use taskcast_redis::{create_managed_redis_adapters, RedisBroadcastProvider, RedisShortTermStore};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::GenericImage;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a minimal TaskEvent for use in broadcast tests.
fn make_test_event(task_id: &str, event_type: &str) -> TaskEvent {
    TaskEvent {
        id: format!("evt-{event_type}"),
        task_id: task_id.to_string(),
        index: 0,
        timestamp: 0.0,
        r#type: event_type.to_string(),
        level: Level::Info,
        data: serde_json::json!(null),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

/// Open a fresh `RedisBroadcastProvider` against `redis_url`.
async fn make_redis_broadcast(redis_url: &str) -> RedisBroadcastProvider {
    let client = redis::Client::open(redis_url).unwrap();
    let pub_conn = client.get_multiplexed_async_connection().await.unwrap();
    let sub_conn = client.get_async_pubsub().await.unwrap();
    RedisBroadcastProvider::new(pub_conn, sub_conn, Some("test"))
}

/// Create a Redis-backed engine using the given connection URL.
async fn make_redis_engine(redis_url: &str) -> TaskEngine {
    let client = redis::Client::open(redis_url).unwrap();
    let conn = client.get_multiplexed_async_connection().await.unwrap();
    let store = RedisShortTermStore::new(conn, Some("test"));
    TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(store),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    })
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

async fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "condition did not become true before deadline"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn two_engine_instances_produce_no_duplicate_event_indices() {
    // This is the Rust equivalent of the TS regression test that found 37/60 index
    // collisions before nextIndex was moved to RedisShortTermStore with INCR.
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let redis_url = format!("redis://127.0.0.1:{port}");

    flush_redis(&redis_url).await;

    let engine1 = Arc::new(make_redis_engine(&redis_url).await);
    let engine2 = Arc::new(make_redis_engine(&redis_url).await);

    // Create task via engine1
    let task = engine1
        .create_task(CreateTaskInput {
            id: Some("shared-task".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine1
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let event_count = 30;

    // Interleave publishes from both instances concurrently
    let mut handles = Vec::new();
    for i in 0..event_count {
        let engine = Arc::clone(&engine1);
        let task_id = task.id.clone();
        handles.push(tokio::spawn(async move {
            engine
                .publish_event(
                    &task_id,
                    PublishEventInput {
                        r#type: "inst1".to_string(),
                        level: Level::Info,
                        data: serde_json::json!({ "i": i }),
                        series_id: None,
                        series_mode: None,
                        series_acc_field: None,
                    },
                )
                .await
                .unwrap()
        }));
    }
    for i in 0..event_count {
        let engine = Arc::clone(&engine2);
        let task_id = task.id.clone();
        handles.push(tokio::spawn(async move {
            engine
                .publish_event(
                    &task_id,
                    PublishEventInput {
                        r#type: "inst2".to_string(),
                        level: Level::Info,
                        data: serde_json::json!({ "i": i }),
                        series_id: None,
                        series_mode: None,
                        series_acc_field: None,
                    },
                )
                .await
                .unwrap()
        }));
    }

    let events: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    let indices: std::collections::HashSet<u64> = events.iter().map(|e| e.index).collect();
    assert_eq!(
        indices.len(),
        event_count * 2,
        "all {} event indices must be unique, got {} unique out of {}",
        event_count * 2,
        indices.len(),
        events.len()
    );
}

#[tokio::test]
async fn concurrent_publish_to_redis_maintains_monotonic_index() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let redis_url = format!("redis://127.0.0.1:{port}");

    flush_redis(&redis_url).await;

    let engine = Arc::new(make_redis_engine(&redis_url).await);
    let task = engine
        .create_task(CreateTaskInput::default())
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let count = 50;
    let mut handles = Vec::new();
    for i in 0..count {
        let engine = Arc::clone(&engine);
        let task_id = task.id.clone();
        handles.push(tokio::spawn(async move {
            engine
                .publish_event(
                    &task_id,
                    PublishEventInput {
                        r#type: "parallel".to_string(),
                        level: Level::Info,
                        data: serde_json::json!({ "i": i }),
                        series_id: None,
                        series_mode: None,
                        series_acc_field: None,
                    },
                )
                .await
                .unwrap()
        }));
    }

    let events: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    let mut indices: Vec<u64> = events.iter().map(|e| e.index).collect();
    indices.sort();

    let unique: std::collections::HashSet<u64> = indices.iter().copied().collect();
    assert_eq!(unique.len(), count, "all indices must be unique");

    let min = *indices.first().unwrap();
    let max = *indices.last().unwrap();
    assert_eq!(
        max - min,
        (count - 1) as u64,
        "indices must be consecutive"
    );
}

#[tokio::test]
async fn redis_store_100_concurrent_tasks_all_get_unique_ids() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let redis_url = format!("redis://127.0.0.1:{port}");

    flush_redis(&redis_url).await;

    let engine = Arc::new(make_redis_engine(&redis_url).await);
    let count = 100;

    let mut handles = Vec::new();
    for _ in 0..count {
        let engine = Arc::clone(&engine);
        handles.push(tokio::spawn(async move {
            engine.create_task(CreateTaskInput::default()).await.unwrap()
        }));
    }

    let tasks: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    let ids: std::collections::HashSet<_> = tasks.iter().map(|t| t.id.clone()).collect();
    assert_eq!(ids.len(), count, "all task IDs must be unique");
}

// ── RedisBroadcastProvider regression tests ───────────────────────────────────
//
// Regression for: subscribe() only registered a local handler without issuing
// any Redis SUBSCRIBE/PSUBSCRIBE command. The background listener had no active
// subscriptions so it received nothing — cross-instance delivery was silently
// broken. Fixed by issuing PSUBSCRIBE <prefix>:task:* in new().

#[tokio::test]
async fn cross_instance_broadcast_delivers_to_subscriber_on_other_instance() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let upstream = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    assert_eq!(proxy.upstream(), upstream);
    let first = create_managed_redis_adapters(
        redis::Client::open(proxy.redis_url()).unwrap(),
        Some("managed-cross-instance"),
        None,
    )
    .await
    .unwrap();
    let second = create_managed_redis_adapters(
        redis::Client::open(proxy.redis_url()).unwrap(),
        Some("managed-cross-instance"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        proxy.accepted_connections(),
        4,
        "each instance owns one command and one PubSub connection"
    );

    let received: Arc<std::sync::Mutex<Vec<TaskEvent>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let received_clone = Arc::clone(&received);
    let _unsub = second
        .adapters
        .broadcast
        .subscribe(
            "task-cross",
            Box::new(move |event| {
                received_clone.lock().unwrap().push(event);
            }),
        )
        .await;

    let before = make_test_event("task-cross", "cross.before");
    first
        .adapters
        .broadcast
        .publish("task-cross", before.clone())
        .await
        .unwrap();
    eventually(Duration::from_secs(5), || {
        received.lock().unwrap().len() == 1
    })
    .await;

    proxy.pause_new_connections();
    proxy.close_latest_connection().await;
    eventually(Duration::from_secs(2), || !second.pubsub.is_subscribed()).await;
    proxy.open().await;
    eventually(Duration::from_secs(5), || second.pubsub.is_subscribed()).await;

    let after = make_test_event("task-cross", "cross.after");
    first
        .adapters
        .broadcast
        .publish("task-cross", after.clone())
        .await
        .unwrap();
    eventually(Duration::from_secs(5), || {
        received.lock().unwrap().len() == 2
    })
    .await;

    let events = received.lock().unwrap();
    assert_eq!(events[0].id, before.id);
    assert_eq!(events[1].id, after.id);
    drop(events);
    let mut command = first.command_manager.clone();
    let patterns = redis::cmd("PUBSUB")
        .arg("NUMPAT")
        .query_async::<i64>(&mut command)
        .await
        .unwrap();
    assert_eq!(patterns, 1, "both instances use the same wildcard pattern");
    let pubsub_clients = redis::cmd("CLIENT")
        .arg("LIST")
        .arg("TYPE")
        .arg("PUBSUB")
        .query_async::<String>(&mut command)
        .await
        .unwrap()
        .lines()
        .count();
    assert_eq!(
        pubsub_clients, 2,
        "each managed instance owns one PubSub connection"
    );

    second.pubsub.shutdown().await;
    first.pubsub.shutdown().await;
    proxy.stop().await;
}

#[tokio::test]
async fn cross_instance_broadcast_wildcard_covers_multiple_task_channels() {
    // PSUBSCRIBE uses a wildcard so all task IDs are covered by a single
    // Redis subscription — no per-task SUBSCRIBE call needed.
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let redis_url = format!("redis://127.0.0.1:{port}");

    let provider_a = make_redis_broadcast(&redis_url).await;
    let provider_b = make_redis_broadcast(&redis_url).await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Subscribe to two different task channels on instance B
    let received: Arc<std::sync::Mutex<Vec<TaskEvent>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let r1 = Arc::clone(&received);
    let r2 = Arc::clone(&received);

    let _unsub1 = provider_b
        .subscribe("task-alpha", Box::new(move |e| r1.lock().unwrap().push(e)))
        .await;
    let _unsub2 = provider_b
        .subscribe("task-beta", Box::new(move |e| r2.lock().unwrap().push(e)))
        .await;

    // Publish to both channels from instance A
    provider_a
        .publish("task-alpha", make_test_event("task-alpha", "alpha.event"))
        .await
        .unwrap();
    provider_a
        .publish("task-beta", make_test_event("task-beta", "beta.event"))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let events = received.lock().unwrap();
    assert_eq!(events.len(), 2, "both task channels must be covered by the wildcard PSUBSCRIBE");
    let types: Vec<&str> = events.iter().map(|e| e.r#type.as_str()).collect();
    assert!(types.contains(&"alpha.event"), "alpha.event must be received");
    assert!(types.contains(&"beta.event"), "beta.event must be received");
}
