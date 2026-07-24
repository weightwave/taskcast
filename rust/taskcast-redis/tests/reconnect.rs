mod support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use redis::AsyncCommands;
use support::{redis_command_matches, TcpFaultProxy};
use taskcast_core::types::{BroadcastProvider, Level, ShortTermStore};
use taskcast_core::{
    DependencyName, DependencyObservation, DependencyObservationState, DependencyObserver,
    TaskEvent,
};
use taskcast_redis::{command_check, create_connection_manager, create_managed_redis_adapters};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::GenericImage;

async fn eventually_ping(manager: &redis::aio::ConnectionManager) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if command_check(manager).await.is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "manager did not recover before deadline"
        );
        tokio::task::yield_now().await;
    }
}

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<DependencyObservation>>,
}

impl DependencyObserver for RecordingObserver {
    fn observe(&self, observation: DependencyObservation) {
        self.observations.lock().unwrap().push(observation);
    }
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

fn make_event(id: &str) -> TaskEvent {
    TaskEvent {
        id: id.to_string(),
        task_id: "task-long-outage".to_string(),
        index: 0,
        timestamp: 1_000.0,
        r#type: "managed.event".to_string(),
        level: Level::Info,
        data: serde_json::json!({"text": "managed"}),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

#[test]
fn redis_matcher_handles_fragmented_and_coalesced_resp_commands() {
    let ping = b"*1\r\n$4\r\nPING\r\n";
    let increment = b"*2\r\n$4\r\nINCR\r\n$23\r\ntaskcast:test:no-replay\r\n";
    let request = [increment.as_slice(), ping.as_slice()].concat();
    let expected = [b"INCR".as_slice(), b"taskcast:test:no-replay".as_slice()];

    assert!(!redis_command_matches(
        &increment[..increment.len() - 3],
        &expected
    ));
    assert!(redis_command_matches(&request, &expected));
    assert!(!redis_command_matches(
        &[ping.as_slice(), increment.as_slice()].concat(),
        &expected
    ));
    assert!(!redis_command_matches(
        b"*2\r\n$4\r\nINCR\r\n$14\r\ntaskcast:other\r\n",
        &expected
    ));
}

#[tokio::test]
async fn managed_command_does_not_replay_incr_after_its_response_is_lost() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let upstream = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    let direct_client = redis::Client::open(format!("redis://{upstream}")).unwrap();
    let mut direct = direct_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let manager = create_connection_manager(redis::Client::open(proxy.redis_url()).unwrap())
        .await
        .unwrap();
    let key = "taskcast:test:no-replay";
    direct.set::<_, _, ()>(key, 0).await.unwrap();
    let matched_before = proxy.matched_commands();

    proxy
        .drop_next_response(move |request| {
            redis_command_matches(request, &[b"INCR", key.as_bytes()])
        })
        .await;
    let mut interrupted = manager.clone();
    let result = tokio::time::timeout(Duration::from_secs(5), async move {
        redis::cmd("INCR")
            .arg(key)
            .query_async::<i64>(&mut interrupted)
            .await
    })
    .await
    .expect("ambiguous INCR did not settle before the deadline");

    proxy.open().await;
    eventually_ping(&manager).await;
    let upstream_value = direct.get::<_, i64>(key).await.unwrap();
    let matched_commands = proxy.matched_commands() - matched_before;
    assert!(
        result.is_err(),
        "the ambiguous INCR must fail: result={result:?}, \
         upstream_value={upstream_value}, matched_commands={matched_commands}"
    );
    assert_eq!(upstream_value, 1);
    assert_eq!(matched_commands, 1);
    proxy.stop().await;
}

#[tokio::test]
async fn managed_command_uses_one_coordinated_reconnect_and_recovers() {
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

    let client = redis::Client::open(proxy.redis_url()).unwrap();
    let manager = create_connection_manager(client).await.unwrap();
    command_check(&manager).await.unwrap();
    assert_eq!(proxy.accepted_connections(), 1);

    let mut first = manager.clone();
    proxy.refuse().await;
    let first_result: redis::RedisResult<String> = first.get("taskcast:managed:current").await;
    assert!(
        first_result.is_err(),
        "the command that sees the outage must fail"
    );

    proxy.open().await;
    let baseline = proxy.accepted_connections();
    let calls = (0..50).map(|_| {
        let mut clone = manager.clone();
        tokio::spawn(async move { redis::cmd("PING").query_async::<String>(&mut clone).await })
    });
    let results = futures::future::join_all(calls).await;
    for result in results {
        assert_eq!(result.unwrap().unwrap(), "PONG");
    }
    eventually_ping(&manager).await;

    let reconnect_connections = proxy.accepted_connections() - baseline;
    assert!(
        reconnect_connections <= 3,
        "50 callers opened {reconnect_connections} reconnect sockets"
    );
    proxy.stop().await;
}

#[tokio::test]
async fn managed_paths_recover_store_and_new_pubsub_messages_after_long_outage() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let upstream = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    let managed = create_managed_redis_adapters(
        redis::Client::open(proxy.redis_url()).unwrap(),
        Some("managed-long-outage"),
        None,
    )
    .await
    .unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_handler = Arc::clone(&received);
    let unsubscribe = managed
        .adapters
        .broadcast
        .subscribe(
            "task-long-outage",
            Box::new(move |event| received_handler.lock().unwrap().push(event.id)),
        )
        .await;

    managed
        .adapters
        .broadcast
        .publish("task-long-outage", make_event("before-long-outage"))
        .await
        .unwrap();
    eventually(Duration::from_secs(2), || {
        *received.lock().unwrap() == ["before-long-outage"]
    })
    .await;

    let accepted_before_outage = proxy.accepted_connections();
    proxy.blackhole().await;
    let mut interrupted_manager = managed.command_manager.clone();
    let interrupted = tokio::spawn(async move {
        redis::cmd("GET")
            .arg("taskcast:managed:interrupted")
            .query_async::<Option<String>>(&mut interrupted_manager)
            .await
    });
    eventually(Duration::from_secs(2), || {
        proxy.accepted_connections() - accepted_before_outage >= 2
    })
    .await;
    proxy.refuse().await;
    let interrupted_result = tokio::time::timeout(Duration::from_secs(5), interrupted)
        .await
        .expect("blackholed command did not settle after refusal")
        .unwrap();
    assert!(
        interrupted_result.is_err(),
        "the command interrupted by the outage must fail"
    );

    // One command-manager attempt plus one PubSub-supervisor attempt per
    // round: wait through two refused rounds after the blackholed round.
    eventually(Duration::from_secs(10), || {
        proxy.accepted_connections() - accepted_before_outage >= 6
    })
    .await;
    proxy.open().await;
    eventually_ping(&managed.command_manager).await;
    eventually(Duration::from_secs(10), || managed.pubsub.is_subscribed()).await;

    assert_eq!(
        managed
            .adapters
            .short_term_store
            .next_index("task-managed-recovered")
            .await
            .unwrap(),
        0
    );
    managed
        .adapters
        .broadcast
        .publish("task-long-outage", make_event("after-long-outage"))
        .await
        .unwrap();
    eventually(Duration::from_secs(2), || {
        *received.lock().unwrap() == ["before-long-outage", "after-long-outage"]
    })
    .await;

    // Two coordinated paths, with one allowed transition race per path:
    // blackhole + two refused rounds + race + successful recovery.
    assert!(
        proxy.accepted_connections() - accepted_before_outage <= 10,
        "the command manager and PubSub supervisor exceeded their fixed reconnect bound"
    );
    unsubscribe();
    managed.pubsub.shutdown().await;
    proxy.stop().await;
}

#[tokio::test]
async fn fifty_callers_share_the_command_and_pubsub_reconnect_paths() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let upstream = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    let managed = create_managed_redis_adapters(
        redis::Client::open(proxy.redis_url()).unwrap(),
        Some("managed-connection-bound"),
        None,
    )
    .await
    .unwrap();
    let accepted_before_drop = proxy.accepted_connections();

    proxy.close_sockets().await;
    let calls = (0..50).map(|_| {
        let mut manager = managed.command_manager.clone();
        async move { redis::cmd("PING").query_async::<String>(&mut manager).await }
    });
    let results = tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(calls))
        .await
        .expect("50 commands did not settle before the reconnect deadline");
    assert_eq!(results.len(), 50);

    eventually_ping(&managed.command_manager).await;
    eventually(Duration::from_secs(10), || managed.pubsub.is_subscribed()).await;
    assert!(
        proxy.accepted_connections() - accepted_before_drop <= 4,
        "50 callers exceeded the two coordinated reconnect paths"
    );
    managed.pubsub.shutdown().await;
    proxy.stop().await;
}

#[tokio::test]
async fn pubsub_factory_returns_subscribed_with_one_pattern_and_two_connections() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let upstream = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    let client = redis::Client::open(proxy.redis_url()).unwrap();

    let managed = create_managed_redis_adapters(client, Some("managed-lifecycle"), None)
        .await
        .unwrap();

    assert!(
        managed.pubsub.is_subscribed(),
        "factory must await the initial PSUBSCRIBE"
    );
    assert_eq!(
        proxy.accepted_connections(),
        2,
        "one shared command connection plus one PubSub connection"
    );
    command_check(&managed.command_manager).await.unwrap();
    let mut command = managed.command_manager.clone();
    let patterns = redis::cmd("PUBSUB")
        .arg("NUMPAT")
        .query_async::<i64>(&mut command)
        .await
        .unwrap();
    assert_eq!(patterns, 1, "managed instance owns one wildcard pattern");

    managed.pubsub.shutdown().await;
    assert!(!managed.pubsub.is_subscribed());
    proxy.stop().await;
}

#[tokio::test]
async fn pubsub_shutdown_during_retry_cancels_sleep_and_future_connects() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let upstream = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    let observer = Arc::new(RecordingObserver::default());
    let client = redis::Client::open(proxy.redis_url()).unwrap();
    let managed =
        create_managed_redis_adapters(client, Some("managed-shutdown"), Some(observer.clone()))
            .await
            .unwrap();

    proxy.pause_new_connections();
    proxy.close_latest_connection().await;
    eventually(Duration::from_secs(2), || {
        !managed.pubsub.is_subscribed()
            && observer
                .observations
                .lock()
                .unwrap()
                .iter()
                .any(|observation| {
                    observation.dependency == DependencyName::RedisPubSub
                        && observation.state == DependencyObservationState::Reconnecting
                        && observation.next_retry_ms.is_some()
                })
    })
    .await;

    managed.pubsub.shutdown().await;
    let accepted_after_shutdown = proxy.accepted_connections();
    proxy.open().await;
    let unexpected_connect = tokio::time::timeout(Duration::from_millis(750), async {
        loop {
            if proxy.accepted_connections() != accepted_after_shutdown {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        unexpected_connect.is_err(),
        "shutdown must cancel retry sleep before another connection"
    );

    proxy.stop().await;
}

#[tokio::test]
async fn pubsub_unreachable_startup_fails_within_overall_deadline() {
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let upstream = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let proxy = TcpFaultProxy::start(upstream).await.unwrap();
    let redis_url = proxy.redis_url();
    proxy.stop().await;
    let started_at = Instant::now();

    let result =
        create_managed_redis_adapters(redis::Client::open(redis_url).unwrap(), None, None).await;

    assert!(result.is_err(), "unreachable Redis must fail startup");
    assert!(
        started_at.elapsed() <= Duration::from_millis(15_500),
        "command connect, PING, and PSUBSCRIBE must share one 15-second deadline"
    );
}
