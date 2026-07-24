mod support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use redis::AsyncCommands;
use support::TcpFaultProxy;
use taskcast_redis::{command_check, create_connection_manager};
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
