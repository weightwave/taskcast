pub mod broadcast;
pub mod connection;
pub mod pubsub;
pub mod short_term;

pub use broadcast::RedisBroadcastProvider;
pub use connection::{command_check, create_connection_manager, RedisCommandConnection};
pub use pubsub::RedisPubSubHandle;
pub use short_term::RedisShortTermStore;

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use redis::aio::MultiplexedConnection;
use taskcast_core::{
    BoxError, DependencyErrorKind, DependencyName, DependencyObserver, DependencyUnavailableError,
};

/// Adapters returned by [`create_redis_adapters`].
pub struct RedisAdapters {
    pub broadcast: RedisBroadcastProvider,
    pub short_term_store: RedisShortTermStore,
}

pub struct ManagedRedisAdapters {
    pub adapters: RedisAdapters,
    pub command_manager: redis::aio::ConnectionManager,
    pub pubsub: RedisPubSubHandle,
}

#[derive(Clone, Copy)]
enum RedisStartupPhase {
    Command,
    PubSub,
}

fn startup_timeout(phase: RedisStartupPhase) -> BoxError {
    let dependency = match phase {
        RedisStartupPhase::Command => DependencyName::RedisCommand,
        RedisStartupPhase::PubSub => DependencyName::RedisPubSub,
    };
    Box::new(DependencyUnavailableError::new(
        dependency,
        DependencyErrorKind::Timeout,
        io::Error::new(io::ErrorKind::TimedOut, "Redis startup timed out"),
    ))
}

async fn within_startup_phase<T>(
    phase: RedisStartupPhase,
    remaining: Duration,
    operation: impl Future<Output = T>,
) -> Result<T, BoxError> {
    tokio::time::timeout(remaining, operation)
        .await
        .map_err(|_| startup_timeout(phase))
}

/// Convenience factory that builds both a [`RedisBroadcastProvider`] and a
/// [`RedisShortTermStore`] from the provided connections.
///
/// - `pub_conn`: multiplexed connection for PUBLISH and general commands.
/// - `sub_conn`: dedicated PubSub connection for SUBSCRIBE.
/// - `store_conn`: multiplexed connection for the short-term store.
/// - `prefix`: optional key/channel prefix (defaults to `"taskcast"`).
pub fn create_redis_adapters(
    pub_conn: MultiplexedConnection,
    sub_conn: redis::aio::PubSub,
    store_conn: MultiplexedConnection,
    prefix: Option<&str>,
) -> RedisAdapters {
    RedisAdapters {
        broadcast: RedisBroadcastProvider::new(pub_conn, sub_conn, prefix),
        short_term_store: RedisShortTermStore::new(store_conn, prefix),
    }
}

pub async fn create_managed_redis_adapters(
    client: redis::Client,
    prefix: Option<&str>,
    observer: Option<Arc<dyn DependencyObserver>>,
) -> Result<ManagedRedisAdapters, BoxError> {
    let prefix = prefix.map(str::to_owned);
    let deadline = Instant::now() + Duration::from_secs(15);
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let command_manager = within_startup_phase(
        RedisStartupPhase::Command,
        remaining(),
        create_connection_manager(client.clone()),
    )
    .await?
    .map_err(|error| {
        let kind =
            connection::classify_redis_error(&error).unwrap_or(DependencyErrorKind::Unavailable);
        Box::new(DependencyUnavailableError::new(
            DependencyName::RedisCommand,
            kind,
            error,
        )) as BoxError
    })?;
    let handlers = broadcast::new_handler_map();
    let channel_prefix = broadcast::channel_prefix(prefix.as_deref());
    let (pubsub, initialized) =
        RedisPubSubHandle::start(client, channel_prefix, handlers.clone(), observer.clone());
    let initialized = within_startup_phase(RedisStartupPhase::PubSub, remaining(), async {
        initialized.await.map_err(|_| {
            Box::new(DependencyUnavailableError::new(
                DependencyName::RedisPubSub,
                DependencyErrorKind::ConnectionClosed,
                pubsub::startup_cancelled_error(),
            )) as BoxError
        })
    })
    .await;
    match initialized {
        Ok(Ok(())) => {}
        Ok(Err(error)) | Err(error) => {
            pubsub.shutdown().await;
            return Err(error);
        }
    }

    let adapters = RedisAdapters {
        broadcast: RedisBroadcastProvider::new_managed(
            command_manager.clone(),
            handlers,
            prefix.as_deref(),
            observer.clone(),
        ),
        short_term_store: RedisShortTermStore::new_managed(
            command_manager.clone(),
            prefix.as_deref(),
            observer,
        ),
    };
    Ok(ManagedRedisAdapters {
        adapters,
        command_manager,
        pubsub,
    })
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::time::Duration;

    use taskcast_core::{DependencyName, DependencyUnavailableError};

    use super::{within_startup_phase, RedisStartupPhase};

    #[tokio::test]
    async fn startup_timeout_is_attributed_to_the_active_phase() {
        for (phase, dependency) in [
            (RedisStartupPhase::Command, DependencyName::RedisCommand),
            (RedisStartupPhase::PubSub, DependencyName::RedisPubSub),
        ] {
            let error = within_startup_phase(phase, Duration::ZERO, pending::<()>())
                .await
                .expect_err("a pending phase must time out");
            let unavailable = error
                .downcast_ref::<DependencyUnavailableError>()
                .expect("phase timeouts use the dependency contract");
            assert_eq!(unavailable.dependency(), dependency);
        }
    }
}
