pub mod broadcast;
pub mod short_term;

pub use broadcast::RedisBroadcastProvider;
pub use short_term::RedisShortTermStore;

use redis::aio::MultiplexedConnection;

/// Adapters returned by [`create_redis_adapters`].
pub struct RedisAdapters {
    pub broadcast: RedisBroadcastProvider,
    pub short_term: RedisShortTermStore,
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
        short_term: RedisShortTermStore::new(store_conn, prefix),
    }
}
