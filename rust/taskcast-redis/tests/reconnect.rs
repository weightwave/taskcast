mod support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use redis::AsyncCommands;
use support::TcpFaultProxy;
use taskcast_core::{
    DependencyName, DependencyObservation, DependencyObservationState, DependencyObserver,
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
