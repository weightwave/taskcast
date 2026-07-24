use std::error::Error;
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
    let outer = io::Error::new(io::ErrorKind::Other, unavailable);
    let found = find_dependency_unavailable(&outer as &(dyn Error + 'static)).unwrap();

    assert_eq!(found.dependency(), DependencyName::RedisCommand);
    assert_eq!(found.kind(), DependencyErrorKind::ConnectionReset);
    assert_eq!(
        found.to_string(),
        "redisCommand unavailable (connection_reset)"
    );
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
