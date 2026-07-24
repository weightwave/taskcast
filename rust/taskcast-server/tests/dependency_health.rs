use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use taskcast_core::{
    DependencyErrorKind, DependencyName, DependencyObservation, DependencyObservationState,
    DependencyObserver, DependencyUnavailableError,
};
use taskcast_server::{DependencyCheck, DependencyHealthLogger, DependencyHealthRegistry};

#[derive(Default)]
struct CollectingLogger {
    records: Mutex<Vec<serde_json::Value>>,
}

impl DependencyHealthLogger for CollectingLogger {
    fn log(&self, record: &serde_json::Value) {
        self.records.lock().unwrap().push(record.clone());
    }
}

fn check<F, Fut>(function: F) -> DependencyCheck
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), DependencyUnavailableError>> + Send + 'static,
{
    Arc::new(move || {
        let future: Pin<Box<dyn Future<Output = Result<(), DependencyUnavailableError>> + Send>> =
            Box::pin(function());
        future
    })
}

#[tokio::test]
async fn readiness_is_sanitized_and_inactive_dependencies_do_no_work() {
    let logger = Arc::new(CollectingLogger::default());
    let health = DependencyHealthRegistry::with_logger(logger.clone());
    let inactive_calls = Arc::new(AtomicUsize::new(0));
    health
        .register(DependencyName::RedisCommand, check(|| async { Ok(()) }))
        .unwrap();
    health
        .register(
            DependencyName::RedisPubSub,
            check(|| async {
                Err(DependencyUnavailableError::new(
                    DependencyName::RedisPubSub,
                    DependencyErrorKind::ConnectionClosed,
                    std::io::Error::other("must not leak"),
                ))
            }),
        )
        .unwrap();
    let inactive = {
        let calls = inactive_calls.clone();
        check(move || {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
    };
    drop(inactive);

    let result = health.check_readiness(Duration::from_secs(2)).await;
    let json = serde_json::to_value(result).unwrap();

    assert_eq!(json["ok"], false);
    assert_eq!(json["dependencies"]["redisCommand"]["state"], "healthy");
    assert_eq!(
        json["dependencies"]["redisPubSub"],
        serde_json::json!({
            "state": "unhealthy",
            "errorKind": "connection_closed"
        })
    );
    assert!(json["dependencies"]["postgres"].is_null());
    assert_eq!(inactive_calls.load(Ordering::SeqCst), 0);
    assert!(!json.to_string().contains("must not leak"));
    assert!(!serde_json::to_string(&*logger.records.lock().unwrap())
        .unwrap()
        .contains("must not leak"));
}

#[tokio::test]
async fn checks_start_concurrently_and_share_one_overall_deadline() {
    let health = DependencyHealthRegistry::new();
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    for name in [DependencyName::RedisCommand, DependencyName::Postgres] {
        let started = started.clone();
        let release = release.clone();
        health
            .register(
                name,
                check(move || {
                    let started = started.clone();
                    let release = release.clone();
                    async move {
                        started.fetch_add(1, Ordering::SeqCst);
                        release.notified().await;
                        Ok(())
                    }
                }),
            )
            .unwrap();
    }

    let beginning = Instant::now();
    let readiness = health.check_readiness(Duration::from_millis(25)).await;
    let elapsed = beginning.elapsed();
    let json = serde_json::to_value(readiness).unwrap();

    assert_eq!(started.load(Ordering::SeqCst), 2);
    assert!(elapsed < Duration::from_millis(250));
    assert_eq!(json["ok"], false);
    assert_eq!(json["dependencies"]["redisCommand"]["errorKind"], "timeout");
    assert_eq!(json["dependencies"]["postgres"]["errorKind"], "timeout");
}

#[test]
fn transitions_are_deduplicated_and_recovery_is_sanitized() {
    let logger = Arc::new(CollectingLogger::default());
    let health = DependencyHealthRegistry::with_logger(logger.clone());
    health
        .register(DependencyName::RedisPubSub, check(|| async { Ok(()) }))
        .unwrap();
    let degraded = DependencyObservation {
        dependency: DependencyName::RedisPubSub,
        state: DependencyObservationState::Reconnecting,
        error_kind: Some(DependencyErrorKind::ConnectionReset),
        attempt: Some(1),
        next_retry_ms: Some(500),
    };

    health.observe(degraded);
    health.observe(degraded);
    health.observe(DependencyObservation {
        dependency: DependencyName::RedisPubSub,
        state: DependencyObservationState::Healthy,
        error_kind: None,
        attempt: None,
        next_retry_ms: None,
    });

    let records = logger.records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["event"], "dependency_state_change");
    assert_eq!(records[0]["level"], "warn");
    assert_eq!(records[0]["attempt"], 1);
    assert_eq!(records[1]["event"], "dependency_state_change");
    assert_eq!(records[1]["level"], "info");
    assert!(records[1]["downtimeMs"].is_number());
    let serialized = serde_json::to_string(&*records).unwrap().to_lowercase();
    for secret_field in [
        "url",
        "host",
        "port",
        "credential",
        "password",
        "raw",
        "sql",
        "argument",
        "authorization",
        "payload",
    ] {
        assert!(!serialized.contains(secret_field));
    }
}

#[test]
fn duplicate_registration_is_rejected_and_attempts_are_pubsub_only() {
    let health = DependencyHealthRegistry::new();
    health
        .register(DependencyName::RedisCommand, check(|| async { Ok(()) }))
        .unwrap();
    assert!(health
        .register(DependencyName::RedisCommand, check(|| async { Ok(()) }))
        .is_err());
    health
        .register(DependencyName::RedisPubSub, check(|| async { Ok(()) }))
        .unwrap();
    health.observe(DependencyObservation {
        dependency: DependencyName::RedisCommand,
        state: DependencyObservationState::Unhealthy,
        error_kind: Some(DependencyErrorKind::Unavailable),
        attempt: Some(9),
        next_retry_ms: None,
    });
    health.observe(DependencyObservation {
        dependency: DependencyName::RedisPubSub,
        state: DependencyObservationState::Reconnecting,
        error_kind: Some(DependencyErrorKind::ConnectionClosed),
        attempt: Some(3),
        next_retry_ms: None,
    });

    let snapshot = health.snapshot();
    assert!(snapshot["redisCommand"]["reconnectAttempts"].is_null());
    assert_eq!(snapshot["redisPubSub"]["reconnectAttempts"], 3);
}

#[tokio::test]
async fn a_late_check_result_cannot_delay_the_timeout_response() {
    let health = DependencyHealthRegistry::new();
    let finished = Arc::new(AtomicBool::new(false));
    health
        .register(
            DependencyName::Postgres,
            check({
                let finished = finished.clone();
                move || {
                    let finished = finished.clone();
                    async move {
                        std::future::pending::<()>().await;
                        finished.store(true, Ordering::SeqCst);
                        Ok(())
                    }
                }
            }),
        )
        .unwrap();

    let result = health.check_readiness(Duration::from_millis(10)).await;

    assert!(!result.ok);
    assert!(!finished.load(Ordering::SeqCst));
}
