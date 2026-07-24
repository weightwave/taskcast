use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

pub type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyName {
    RedisCommand,
    RedisPubSub,
    Postgres,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyState {
    Starting,
    Healthy,
    Reconnecting,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyObservationState {
    Healthy,
    Reconnecting,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyErrorKind {
    ConnectionRefused,
    ConnectionReset,
    Timeout,
    Dns,
    Authentication,
    ConnectionClosed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyObservation {
    pub dependency: DependencyName,
    pub state: DependencyObservationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<DependencyErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_ms: Option<u64>,
}

pub trait DependencyObserver: Send + Sync + 'static {
    fn observe(&self, observation: DependencyObservation);
}

#[derive(Debug)]
pub struct DependencyUnavailableError {
    dependency: DependencyName,
    kind: DependencyErrorKind,
    source: BoxError,
}

impl DependencyUnavailableError {
    pub fn new(
        dependency: DependencyName,
        kind: DependencyErrorKind,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            dependency,
            kind,
            source: Box::new(source),
        }
    }

    pub fn dependency(&self) -> DependencyName {
        self.dependency
    }

    pub fn kind(&self) -> DependencyErrorKind {
        self.kind
    }
}

impl fmt::Display for DependencyUnavailableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dependency = serde_json::to_value(self.dependency).expect("DependencyName serializes");
        let kind = serde_json::to_value(self.kind).expect("DependencyErrorKind serializes");
        write!(
            f,
            "{} unavailable ({})",
            dependency.as_str().unwrap(),
            kind.as_str().unwrap()
        )
    }
}

impl Error for DependencyUnavailableError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub fn find_dependency_unavailable<'a>(
    mut error: &'a (dyn Error + 'static),
) -> Option<&'a DependencyUnavailableError> {
    // Error::source chains are conventionally acyclic, but adapters can supply
    // custom errors. A bounded walk keeps a malformed chain from hanging a
    // request-time classification while preserving ordinary nested causes.
    for _ in 0..64 {
        if let Some(found) = error.downcast_ref::<DependencyUnavailableError>() {
            return Some(found);
        }
        // io::Error::source() skips its immediate custom payload.
        if let Some(inner) = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
        {
            error = inner;
            continue;
        }
        error = error.source()?;
    }
    None
}
