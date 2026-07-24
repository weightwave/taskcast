use std::error::Error;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use redis::aio::{ConnectionLike, ConnectionManager, MultiplexedConnection};
use redis::{Cmd, ErrorKind, Pipeline, RedisError, RedisFuture, RedisResult, Value};
use taskcast_core::{
    BoxError, DependencyErrorKind, DependencyName, DependencyObservation,
    DependencyObservationState, DependencyObserver, DependencyUnavailableError,
};

#[derive(Clone)]
pub enum RedisCommandConnection {
    Raw(MultiplexedConnection),
    Managed {
        manager: ConnectionManager,
        observer: Option<Arc<dyn DependencyObserver>>,
    },
}

impl RedisCommandConnection {
    #[allow(dead_code)] // Used by the managed adapter composition added in Task 4.
    pub(crate) fn managed(
        manager: ConnectionManager,
        observer: Option<Arc<dyn DependencyObserver>>,
    ) -> Self {
        Self::Managed { manager, observer }
    }

    pub(crate) fn observe_result<T>(&self, result: RedisResult<T>) -> Result<T, BoxError> {
        match result {
            Ok(value) => {
                self.observe(DependencyObservationState::Healthy, None);
                Ok(value)
            }
            Err(error) => {
                let Some(kind) = classify_redis_error(&error) else {
                    return Err(Box::new(error));
                };
                self.observe(DependencyObservationState::Reconnecting, Some(kind));
                Err(Box::new(DependencyUnavailableError::new(
                    DependencyName::RedisCommand,
                    kind,
                    error,
                )))
            }
        }
    }

    pub(crate) fn is_managed(&self) -> bool {
        matches!(self, Self::Managed { .. })
    }

    fn observe(&self, state: DependencyObservationState, error_kind: Option<DependencyErrorKind>) {
        let Self::Managed {
            observer: Some(observer),
            ..
        } = self
        else {
            return;
        };
        observer.observe(DependencyObservation {
            dependency: DependencyName::RedisCommand,
            state,
            error_kind,
            attempt: None,
            next_retry_ms: None,
        });
    }
}

impl From<MultiplexedConnection> for RedisCommandConnection {
    fn from(connection: MultiplexedConnection) -> Self {
        Self::Raw(connection)
    }
}

impl ConnectionLike for RedisCommandConnection {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        match self {
            Self::Raw(connection) => connection.req_packed_command(cmd),
            Self::Managed { manager, .. } => manager.req_packed_command(cmd),
        }
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        match self {
            Self::Raw(connection) => connection.req_packed_commands(cmd, offset, count),
            Self::Managed { manager, .. } => manager.req_packed_commands(cmd, offset, count),
        }
    }

    fn get_db(&self) -> i64 {
        match self {
            Self::Raw(connection) => connection.get_db(),
            Self::Managed { manager, .. } => manager.get_db(),
        }
    }
}

pub async fn create_connection_manager(client: redis::Client) -> RedisResult<ConnectionManager> {
    let startup = async {
        // redis 0.27.6 leaves backon's 2x default exponent in place; max_delay
        // caps the nominal delay before jitter is added.
        let config = redis::aio::ConnectionManagerConfig::new()
            .set_exponent_base(2)
            .set_factor(2)
            .set_number_of_retries(2)
            .set_max_delay(2_000)
            .set_connection_timeout(Duration::from_secs(2))
            .set_response_timeout(Duration::from_secs(10));
        let mut manager = ConnectionManager::new_with_config(client, config).await?;
        redis::cmd("PING")
            .query_async::<String>(&mut manager)
            .await?;
        Ok(manager)
    };

    tokio::time::timeout(Duration::from_secs(15), startup)
        .await
        .map_err(|_| {
            RedisError::from((
                ErrorKind::IoError,
                "Redis startup timed out",
                "startup deadline exceeded".to_string(),
            ))
        })?
}

pub async fn command_check(manager: &ConnectionManager) -> Result<(), DependencyUnavailableError> {
    let mut manager = manager.clone();
    redis::cmd("PING")
        .query_async::<String>(&mut manager)
        .await
        .map(|_| ())
        .map_err(|error| {
            let kind = classify_redis_error(&error).unwrap_or(DependencyErrorKind::Unavailable);
            DependencyUnavailableError::new(DependencyName::RedisCommand, kind, error)
        })
}

pub(crate) fn classify_redis_error(error: &RedisError) -> Option<DependencyErrorKind> {
    if error.kind() == ErrorKind::AuthenticationFailed {
        return Some(DependencyErrorKind::Authentication);
    }
    if error.is_connection_refusal() {
        return Some(DependencyErrorKind::ConnectionRefused);
    }
    if error.is_timeout() {
        return Some(DependencyErrorKind::Timeout);
    }

    let io_error = error
        .source()
        .and_then(|source| source.downcast_ref::<io::Error>());
    if let Some(io_error) = io_error {
        if matches!(
            io_error.raw_os_error(),
            Some(11_001 | 11_002 | 11_003 | 11_004)
        ) {
            return Some(DependencyErrorKind::Dns);
        }
        return Some(match io_error.kind() {
            io::ErrorKind::ConnectionReset => DependencyErrorKind::ConnectionReset,
            io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::NotConnected => DependencyErrorKind::ConnectionClosed,
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => DependencyErrorKind::Timeout,
            io::ErrorKind::NotFound => DependencyErrorKind::Dns,
            _ => {
                let detail = io_error.to_string().to_ascii_lowercase();
                if detail.contains("dns")
                    || detail.contains("name resolution")
                    || detail.contains("lookup address")
                    || detail.contains("no such host")
                    || detail.contains("nodename nor servname")
                    || detail.contains("name or service not known")
                    || detail.contains("temporary failure in name resolution")
                {
                    DependencyErrorKind::Dns
                } else {
                    DependencyErrorKind::Unavailable
                }
            }
        });
    }
    if error.is_connection_dropped() {
        return Some(DependencyErrorKind::ConnectionClosed);
    }
    if error.is_io_error() {
        return Some(DependencyErrorKind::Unavailable);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_connectivity_and_authentication_errors() {
        let refused = RedisError::from(io::Error::from(io::ErrorKind::ConnectionRefused));
        assert_eq!(
            classify_redis_error(&refused),
            Some(DependencyErrorKind::ConnectionRefused)
        );

        let timed_out = RedisError::from(io::Error::from(io::ErrorKind::TimedOut));
        assert_eq!(
            classify_redis_error(&timed_out),
            Some(DependencyErrorKind::Timeout)
        );

        let authentication =
            RedisError::from((ErrorKind::AuthenticationFailed, "authentication failed"));
        assert_eq!(
            classify_redis_error(&authentication),
            Some(DependencyErrorKind::Authentication)
        );
    }

    #[test]
    fn does_not_classify_redis_command_errors_as_dependency_failures() {
        let command_error = RedisError::from((ErrorKind::TypeError, "wrong Redis data type"));
        assert_eq!(classify_redis_error(&command_error), None);
    }
}
