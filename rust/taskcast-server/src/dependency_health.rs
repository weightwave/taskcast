use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{TimeZone, Utc};
use futures::future::join_all;
use serde::Serialize;
use taskcast_core::{
    DependencyErrorKind, DependencyName, DependencyObservation, DependencyObservationState,
    DependencyObserver, DependencyState, DependencyUnavailableError,
};

pub type DependencyCheck = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), DependencyUnavailableError>> + Send>>
        + Send
        + Sync,
>;

pub trait DependencyHealthLogger: Send + Sync + 'static {
    fn log(&self, record: &serde_json::Value);
}

struct StderrDependencyHealthLogger;

impl DependencyHealthLogger for StderrDependencyHealthLogger {
    fn log(&self, record: &serde_json::Value) {
        eprintln!("{record}");
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyReadiness {
    pub state: DependencyState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<DependencyErrorKind>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReadinessResult {
    pub ok: bool,
    pub dependencies: HashMap<DependencyName, DependencyReadiness>,
}

#[derive(Clone, Default)]
pub struct RuntimeHealth {
    pub registry: Option<Arc<DependencyHealthRegistry>>,
}

#[derive(Clone)]
pub struct DependencyHealthRegistry {
    state: Arc<RwLock<RegistryState>>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    logger: Arc<dyn DependencyHealthLogger>,
}

#[derive(Default)]
struct RegistryState {
    checks: HashMap<DependencyName, DependencyCheck>,
    entries: HashMap<DependencyName, DependencyEntry>,
}

struct DependencyEntry {
    state: DependencyState,
    last_transition_at: u64,
    last_error_kind: Option<DependencyErrorKind>,
    consecutive_failures: u64,
    reconnect_attempts: Option<u32>,
    outage_started_at: Option<u64>,
    last_summary_at: Option<u64>,
}

const OUTAGE_SUMMARY_INTERVAL_MS: u64 = 60_000;

impl DependencyHealthRegistry {
    pub fn new() -> Self {
        Self::with_logger(Arc::new(StderrDependencyHealthLogger))
    }

    pub fn with_logger(logger: Arc<dyn DependencyHealthLogger>) -> Self {
        Self {
            state: Arc::new(RwLock::new(RegistryState::default())),
            clock: Arc::new(system_time_millis),
            logger,
        }
    }

    pub fn register(&self, name: DependencyName, check: DependencyCheck) -> Result<(), String> {
        let now = (self.clock)();
        let mut state = self.state.write().expect("dependency health lock poisoned");
        if state.checks.contains_key(&name) {
            return Err(format!(
                "dependency already registered: {}",
                dependency_name(name)
            ));
        }
        state.checks.insert(name, check);
        state.entries.insert(
            name,
            DependencyEntry {
                state: DependencyState::Starting,
                last_transition_at: now,
                last_error_kind: None,
                consecutive_failures: 0,
                reconnect_attempts: (name == DependencyName::RedisPubSub).then_some(0),
                outage_started_at: None,
                last_summary_at: None,
            },
        );
        Ok(())
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let state = self.state.read().expect("dependency health lock poisoned");
        let mut dependencies = serde_json::Map::new();
        for (name, entry) in &state.entries {
            let mut snapshot = serde_json::Map::new();
            snapshot.insert("configured".into(), serde_json::Value::Bool(true));
            snapshot.insert(
                "state".into(),
                serde_json::to_value(entry.state).expect("dependency state serializes"),
            );
            snapshot.insert(
                "lastTransitionAt".into(),
                serde_json::Value::String(timestamp(entry.last_transition_at)),
            );
            snapshot.insert(
                "consecutiveFailures".into(),
                serde_json::json!(entry.consecutive_failures),
            );
            if let Some(kind) = entry.last_error_kind {
                snapshot.insert(
                    "lastErrorKind".into(),
                    serde_json::to_value(kind).expect("dependency error kind serializes"),
                );
            }
            if *name == DependencyName::RedisPubSub {
                snapshot.insert(
                    "reconnectAttempts".into(),
                    serde_json::json!(entry.reconnect_attempts.unwrap_or(0)),
                );
            }
            dependencies.insert(dependency_name(*name).to_string(), snapshot.into());
        }
        dependencies.into()
    }

    pub async fn check_readiness(&self, timeout: Duration) -> ReadinessResult {
        let checks: Vec<_> = {
            let state = self.state.read().expect("dependency health lock poisoned");
            state
                .checks
                .iter()
                .map(|(name, check)| (*name, Arc::clone(check)))
                .collect()
        };
        let pending = Arc::new(std::sync::Mutex::new(
            checks.iter().map(|(name, _)| *name).collect::<HashSet<_>>(),
        ));
        let futures = checks.into_iter().map(|(name, check)| {
            let registry = self.clone();
            let pending = Arc::clone(&pending);
            async move {
                let result = check().await;
                let was_pending = pending
                    .lock()
                    .expect("dependency readiness lock poisoned")
                    .remove(&name);
                if was_pending {
                    registry.observe(match result {
                        Ok(()) => DependencyObservation {
                            dependency: name,
                            state: DependencyObservationState::Healthy,
                            error_kind: None,
                            attempt: None,
                            next_retry_ms: None,
                        },
                        Err(error) => DependencyObservation {
                            dependency: name,
                            state: DependencyObservationState::Unhealthy,
                            error_kind: Some(error.kind()),
                            attempt: None,
                            next_retry_ms: None,
                        },
                    });
                }
            }
        });

        if tokio::time::timeout(timeout, join_all(futures))
            .await
            .is_err()
        {
            let unfinished: Vec<_> = pending
                .lock()
                .expect("dependency readiness lock poisoned")
                .drain()
                .collect();
            for name in unfinished {
                self.observe(DependencyObservation {
                    dependency: name,
                    state: DependencyObservationState::Unhealthy,
                    error_kind: Some(DependencyErrorKind::Timeout),
                    attempt: None,
                    next_retry_ms: None,
                });
            }
        }

        let state = self.state.read().expect("dependency health lock poisoned");
        let dependencies = state
            .entries
            .iter()
            .map(|(name, entry)| {
                (
                    *name,
                    DependencyReadiness {
                        state: entry.state,
                        error_kind: entry.last_error_kind,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let ok = dependencies
            .values()
            .all(|dependency| dependency.state == DependencyState::Healthy);
        ReadinessResult { ok, dependencies }
    }

    pub(crate) fn record_at(&self, observation: DependencyObservation, now_ms: u64) {
        let record = {
            let mut state = self.state.write().expect("dependency health lock poisoned");
            let Some(entry) = state.entries.get_mut(&observation.dependency) else {
                return;
            };
            let previous = entry.state;
            let next = observation_state(observation.state);
            let was_degraded = is_degraded(previous);
            let degraded = is_degraded(next);

            if next == DependencyState::Healthy {
                entry.consecutive_failures = 0;
                entry.last_error_kind = None;
                if observation.dependency == DependencyName::RedisPubSub {
                    entry.reconnect_attempts = Some(0);
                }
            } else {
                entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
                if observation.error_kind.is_some() {
                    entry.last_error_kind = observation.error_kind;
                }
                if observation.dependency == DependencyName::RedisPubSub {
                    if let Some(attempt) = observation.attempt {
                        entry.reconnect_attempts = Some(attempt);
                    }
                }
                if !was_degraded {
                    entry.outage_started_at = Some(now_ms);
                }
            }

            if previous != next {
                entry.state = next;
                entry.last_transition_at = now_ms;
                entry.last_summary_at = degraded.then_some(now_ms);
                let mut record = serde_json::json!({
                    "timestamp": timestamp(now_ms),
                    "level": if degraded { "warn" } else { "info" },
                    "event": "dependency_state_change",
                    "dependency": dependency_name(observation.dependency),
                    "from": dependency_state(previous),
                    "to": dependency_state(next)
                });
                add_observation_fields(&mut record, &observation, entry.last_error_kind);
                if !degraded && was_degraded {
                    if let Some(started_at) = entry.outage_started_at {
                        record["downtimeMs"] = serde_json::json!(now_ms.saturating_sub(started_at));
                    }
                    entry.outage_started_at = None;
                    entry.last_summary_at = None;
                }
                Some(record)
            } else if degraded
                && now_ms.saturating_sub(entry.last_summary_at.unwrap_or(entry.last_transition_at))
                    >= OUTAGE_SUMMARY_INTERVAL_MS
            {
                entry.last_summary_at = Some(now_ms);
                let mut record = serde_json::json!({
                    "timestamp": timestamp(now_ms),
                    "level": "warn",
                    "event": "dependency_outage_summary",
                    "dependency": dependency_name(observation.dependency),
                    "state": dependency_state(next),
                    "consecutiveFailures": entry.consecutive_failures
                });
                add_observation_fields(&mut record, &observation, entry.last_error_kind);
                Some(record)
            } else {
                None
            }
        };
        if let Some(record) = record {
            self.logger.log(&record);
        }
    }
}

impl Default for DependencyHealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DependencyObserver for DependencyHealthRegistry {
    fn observe(&self, observation: DependencyObservation) {
        self.record_at(observation, (self.clock)());
    }
}

fn add_observation_fields(
    record: &mut serde_json::Value,
    observation: &DependencyObservation,
    error_kind: Option<DependencyErrorKind>,
) {
    if let Some(attempt) = observation.attempt {
        record["attempt"] = serde_json::json!(attempt);
    }
    if let Some(next_retry_ms) = observation.next_retry_ms {
        record["nextRetryMs"] = serde_json::json!(next_retry_ms);
    }
    if let Some(kind) = error_kind {
        record["errorKind"] = serde_json::to_value(kind).expect("dependency error kind serializes");
    }
}

fn is_degraded(state: DependencyState) -> bool {
    matches!(
        state,
        DependencyState::Reconnecting | DependencyState::Unhealthy
    )
}

fn observation_state(state: DependencyObservationState) -> DependencyState {
    match state {
        DependencyObservationState::Healthy => DependencyState::Healthy,
        DependencyObservationState::Reconnecting => DependencyState::Reconnecting,
        DependencyObservationState::Unhealthy => DependencyState::Unhealthy,
    }
}

fn dependency_name(name: DependencyName) -> &'static str {
    match name {
        DependencyName::RedisCommand => "redisCommand",
        DependencyName::RedisPubSub => "redisPubSub",
        DependencyName::Postgres => "postgres",
    }
}

fn dependency_state(state: DependencyState) -> &'static str {
    match state {
        DependencyState::Starting => "starting",
        DependencyState::Healthy => "healthy",
        DependencyState::Reconnecting => "reconnecting",
        DependencyState::Unhealthy => "unhealthy",
    }
}

fn timestamp(now_ms: u64) -> String {
    Utc.timestamp_millis_opt(now_ms as i64)
        .single()
        .expect("dependency health timestamp is valid")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn system_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CollectingLogger {
        records: Mutex<Vec<serde_json::Value>>,
    }

    impl DependencyHealthLogger for CollectingLogger {
        fn log(&self, record: &serde_json::Value) {
            self.records.lock().unwrap().push(record.clone());
        }
    }

    fn successful_check() -> DependencyCheck {
        Arc::new(|| Box::pin(async { Ok(()) }))
    }

    #[test]
    fn injected_times_rate_limit_outage_summaries_and_measure_recovery() {
        let logger = Arc::new(CollectingLogger::default());
        let health = DependencyHealthRegistry::with_logger(logger.clone());
        health
            .register(DependencyName::RedisPubSub, successful_check())
            .unwrap();
        let degraded = |attempt| DependencyObservation {
            dependency: DependencyName::RedisPubSub,
            state: DependencyObservationState::Reconnecting,
            error_kind: Some(DependencyErrorKind::ConnectionReset),
            attempt: Some(attempt),
            next_retry_ms: Some(500),
        };

        health.record_at(degraded(1), 1_000);
        health.record_at(degraded(2), 60_999);
        health.record_at(degraded(3), 61_000);
        health.record_at(degraded(4), 120_999);
        health.record_at(degraded(5), 121_000);
        health.record_at(
            DependencyObservation {
                dependency: DependencyName::RedisPubSub,
                state: DependencyObservationState::Healthy,
                error_kind: None,
                attempt: None,
                next_retry_ms: None,
            },
            126_000,
        );

        let records = logger.records.lock().unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record["event"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "dependency_state_change",
                "dependency_outage_summary",
                "dependency_outage_summary",
                "dependency_state_change"
            ]
        );
        assert_eq!(records[0]["attempt"], 1);
        assert_eq!(records[1]["attempt"], 3);
        assert_eq!(records[2]["attempt"], 5);
        assert_eq!(records[3]["downtimeMs"], 125_000);
    }
}
