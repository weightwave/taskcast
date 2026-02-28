use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use redis::aio::MultiplexedConnection;
use tokio::sync::RwLock;

use taskcast_core::types::{BroadcastProvider, TaskEvent};

type Handler = Arc<dyn Fn(TaskEvent) + Send + Sync>;

/// Redis-backed broadcast provider.
///
/// Uses Redis Pub/Sub for cross-process event distribution. A dedicated
/// subscriber connection listens for messages and fans them out to
/// locally-registered handlers.
pub struct RedisBroadcastProvider {
    pub_conn: MultiplexedConnection,
    handlers: Arc<RwLock<HashMap<String, Vec<Handler>>>>,
    channel_prefix: String,
}

impl RedisBroadcastProvider {
    /// Create a new `RedisBroadcastProvider`.
    ///
    /// - `pub_conn`: connection used for PUBLISH commands.
    /// - `sub_conn`: connection used for SUBSCRIBE (spawns a background listener task).
    /// - `prefix`: key/channel prefix (defaults to `"taskcast"`).
    pub fn new(
        pub_conn: MultiplexedConnection,
        mut sub_conn: redis::aio::PubSub,
        prefix: Option<&str>,
    ) -> Self {
        let resolved_prefix = prefix.unwrap_or("taskcast");
        let channel_prefix = format!("{resolved_prefix}:task:");

        let handlers: Arc<RwLock<HashMap<String, Vec<Handler>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Spawn background listener that reads from the PubSub connection
        // and dispatches to local handlers.
        let handlers_clone = Arc::clone(&handlers);
        let prefix_clone = channel_prefix.clone();
        tokio::spawn(async move {
            let mut stream = sub_conn.on_message();

            while let Some(msg) = stream.next().await {
                let channel: String = match msg.get_channel() {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let task_id = if channel.starts_with(&prefix_clone) {
                    &channel[prefix_clone.len()..]
                } else {
                    &channel
                };

                let event: TaskEvent = match serde_json::from_str(&payload) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let handlers = handlers_clone.read().await;
                if let Some(task_handlers) = handlers.get(task_id) {
                    for handler in task_handlers {
                        handler(event.clone());
                    }
                }
            }
        });

        Self {
            pub_conn,
            handlers,
            channel_prefix,
        }
    }

    /// Returns the channel prefix (e.g. `"taskcast:task:"`).
    pub fn channel_prefix(&self) -> &str {
        &self.channel_prefix
    }
}

#[async_trait]
impl BroadcastProvider for RedisBroadcastProvider {
    async fn publish(
        &self,
        channel: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let full_channel = format!("{}{}", self.channel_prefix, channel);
        let payload = serde_json::to_string(&event)?;
        let mut conn = self.pub_conn.clone();
        redis::cmd("PUBLISH")
            .arg(&full_channel)
            .arg(&payload)
            .query_async::<i64>(&mut conn)
            .await?;
        Ok(())
    }

    async fn subscribe(
        &self,
        channel: &str,
        handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        let handler: Handler = Arc::from(handler);
        {
            let mut handlers = self.handlers.write().await;
            handlers
                .entry(channel.to_string())
                .or_default()
                .push(Arc::clone(&handler));
        }

        let handlers = Arc::clone(&self.handlers);
        let channel = channel.to_string();
        let handler_addr = Arc::as_ptr(&handler) as *const () as usize;

        Box::new(move || {
            let handlers = Arc::clone(&handlers);
            let channel = channel.clone();
            // Spawn a blocking task to clean up the handler.
            // The unsubscribe closure is synchronous per the trait, so we
            // spawn a thread to do the async cleanup.
            let _ = std::thread::spawn(move || {
                let rt = tokio::runtime::Handle::try_current();
                if let Ok(handle) = rt {
                    handle.block_on(async {
                        let mut handlers = handlers.write().await;
                        if let Some(task_handlers) = handlers.get_mut(&channel) {
                            task_handlers.retain(|h| {
                                (Arc::as_ptr(h) as *const () as usize) != handler_addr
                            });
                            if task_handlers.is_empty() {
                                handlers.remove(&channel);
                            }
                        }
                    });
                }
            })
            .join();
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn channel_prefix_default() {
        let prefix = "taskcast";
        let channel_prefix = format!("{prefix}:task:");
        assert_eq!(channel_prefix, "taskcast:task:");
    }

    #[test]
    fn channel_prefix_custom() {
        let prefix = "myapp";
        let channel_prefix = format!("{prefix}:task:");
        assert_eq!(channel_prefix, "myapp:task:");
    }

    #[test]
    fn full_channel_name() {
        let channel_prefix = "taskcast:task:";
        let task_id = "task_01";
        let full = format!("{channel_prefix}{task_id}");
        assert_eq!(full, "taskcast:task:task_01");
    }

    #[test]
    fn strip_prefix_from_channel() {
        let channel_prefix = "taskcast:task:";
        let channel = "taskcast:task:task_01";
        let task_id = if channel.starts_with(channel_prefix) {
            &channel[channel_prefix.len()..]
        } else {
            channel
        };
        assert_eq!(task_id, "task_01");
    }

    #[test]
    fn strip_prefix_passthrough_when_no_match() {
        let channel_prefix = "taskcast:task:";
        let channel = "other:channel";
        let task_id = if channel.starts_with(channel_prefix) {
            &channel[channel_prefix.len()..]
        } else {
            channel
        };
        assert_eq!(task_id, "other:channel");
    }
}
