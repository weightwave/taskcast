use std::error::Error;
use std::fmt;
use std::io;

use taskcast_core::{
    find_dependency_unavailable, DependencyErrorKind, DependencyName, DependencyObservation,
    DependencyObservationState, DependencyUnavailableError,
};

#[test]
fn finds_dependency_error_through_source_chain() {
    let unavailable = DependencyUnavailableError::new(
        DependencyName::RedisCommand,
        DependencyErrorKind::ConnectionReset,
        io::Error::new(io::ErrorKind::ConnectionReset, "secret raw error"),
    );
    let outer = io::Error::other(unavailable);
    let found = find_dependency_unavailable(&outer as &(dyn Error + 'static)).unwrap();

    assert_eq!(found.dependency(), DependencyName::RedisCommand);
    assert_eq!(found.kind(), DependencyErrorKind::ConnectionReset);
    assert_eq!(
        found.to_string(),
        "redisCommand unavailable (connection_reset)"
    );
}

#[derive(Debug)]
struct CyclicError;

impl fmt::Display for CyclicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cyclic error")
    }
}

impl Error for CyclicError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self)
    }
}

#[test]
fn does_not_loop_on_a_cyclic_source_chain() {
    let error = CyclicError;

    assert!(find_dependency_unavailable(&error).is_none());
}

#[test]
fn serializes_observation_with_the_public_names() {
    let observation = DependencyObservation {
        dependency: DependencyName::RedisPubSub,
        state: DependencyObservationState::Reconnecting,
        error_kind: Some(DependencyErrorKind::ConnectionClosed),
        attempt: Some(3),
        next_retry_ms: Some(1_750),
    };
    let json = serde_json::to_value(observation).unwrap();

    assert_eq!(json["dependency"], "redisPubSub");
    assert_eq!(json["state"], "reconnecting");
    assert_eq!(json["errorKind"], "connection_closed");
    assert_eq!(json["nextRetryMs"], 1_750);
}

#[test]
fn rejects_starting_observation_state() {
    let result = serde_json::from_value::<DependencyObservation>(serde_json::json!({
        "dependency": "redisCommand",
        "state": "starting",
    }));

    assert!(result.is_err());
}
