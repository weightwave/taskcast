use async_trait::async_trait;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;

use taskcast_core::types::{EventQueryOptions, ShortTermStore, Task, TaskEvent};

/// Helper to generate Redis key names for a given prefix.
struct Keys {
    prefix: String,
}

impl Keys {
    fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
        }
    }

    /// `{prefix}:task:{id}` -- stores the full Task JSON.
    fn task(&self, id: &str) -> String {
        format!("{}:task:{}", self.prefix, id)
    }

    /// `{prefix}:events:{id}` -- a Redis list of event JSONs.
    fn events(&self, id: &str) -> String {
        format!("{}:events:{}", self.prefix, id)
    }

    /// `{prefix}:idx:{id}` -- atomic index counter (INCR).
    fn idx(&self, id: &str) -> String {
        format!("{}:idx:{}", self.prefix, id)
    }

    /// `{prefix}:series:{taskId}:{seriesId}` -- latest event in a series.
    fn series_latest(&self, task_id: &str, series_id: &str) -> String {
        format!("{}:series:{}:{}", self.prefix, task_id, series_id)
    }

    /// `{prefix}:seriesIds:{taskId}` -- set of series IDs for a task.
    fn series_ids(&self, task_id: &str) -> String {
        format!("{}:seriesIds:{}", self.prefix, task_id)
    }
}

/// Redis-backed short-term store.
///
/// Uses Redis data structures to persist tasks, events, series tracking,
/// and atomic index counters.
pub struct RedisShortTermStore {
    conn: MultiplexedConnection,
    keys: Keys,
}

impl RedisShortTermStore {
    /// Create a new `RedisShortTermStore`.
    ///
    /// - `conn`: a multiplexed Redis connection for all read/write operations.
    /// - `prefix`: key prefix (defaults to `"taskcast"`).
    pub fn new(conn: MultiplexedConnection, prefix: Option<&str>) -> Self {
        let resolved_prefix = prefix.unwrap_or("taskcast");
        Self {
            conn,
            keys: Keys::new(resolved_prefix),
        }
    }

    /// Returns a reference to the key helper for testing or introspection.
    pub fn key_prefix(&self) -> &str {
        &self.keys.prefix
    }
}

#[async_trait]
impl ShortTermStore for RedisShortTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.task(&task.id);
        let json = serde_json::to_string(&task)?;
        let mut conn = self.conn.clone();
        conn.set::<_, _, ()>(&key, &json).await?;
        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.task(task_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = conn.get(&key).await?;
        match result {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    async fn append_event(
        &self,
        task_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.events(task_id);
        let json = serde_json::to_string(&event)?;
        let mut conn = self.conn.clone();
        conn.rpush::<_, _, ()>(&key, &json).await?;
        Ok(())
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.events(task_id);
        let mut conn = self.conn.clone();
        let raw: Vec<String> = conn.lrange(&key, 0, -1).await?;

        let all: Vec<TaskEvent> = raw
            .into_iter()
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect();

        let mut result = all;

        if let Some(ref opts) = opts {
            if let Some(ref since) = opts.since {
                if let Some(ref id) = since.id {
                    let idx = result.iter().position(|e| &e.id == id);
                    result = match idx {
                        Some(i) => result[i + 1..].to_vec(),
                        None => result,
                    };
                } else if let Some(index) = since.index {
                    result.retain(|e| e.index > index);
                } else if let Some(timestamp) = since.timestamp {
                    result.retain(|e| e.timestamp > timestamp);
                }
            }

            if let Some(limit) = opts.limit {
                result.truncate(limit as usize);
            }
        }

        Ok(result)
    }

    async fn set_ttl(
        &self,
        task_id: &str,
        ttl_seconds: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let ttl_secs = ttl_seconds as i64;

        // Expire task key
        conn.expire::<_, ()>(&self.keys.task(task_id), ttl_secs)
            .await?;

        // Expire events list
        conn.expire::<_, ()>(&self.keys.events(task_id), ttl_secs)
            .await?;

        // Expire index counter
        conn.expire::<_, ()>(&self.keys.idx(task_id), ttl_secs)
            .await?;

        // Expire series IDs set and each series latest key
        let series_ids_key = self.keys.series_ids(task_id);
        let series_ids: Vec<String> = conn.smembers(&series_ids_key).await.unwrap_or_default();
        for sid in &series_ids {
            conn.expire::<_, ()>(&self.keys.series_latest(task_id, sid), ttl_secs)
                .await?;
        }
        conn.expire::<_, ()>(&series_ids_key, ttl_secs).await?;

        Ok(())
    }

    async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.series_latest(task_id, series_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = conn.get(&key).await?;
        match result {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    async fn set_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.series_latest(task_id, series_id);
        let json = serde_json::to_string(&event)?;
        let mut conn = self.conn.clone();
        conn.set::<_, _, ()>(&key, &json).await?;
        // Track series ID
        conn.sadd::<_, _, ()>(&self.keys.series_ids(task_id), series_id)
            .await?;
        Ok(())
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let series_key = self.keys.series_latest(task_id, series_id);
        let events_key = self.keys.events(task_id);
        let mut conn = self.conn.clone();

        // Get the previous series latest
        let prev_json: Option<String> = conn.get(&series_key).await?;

        if let Some(prev_json) = prev_json {
            let prev: TaskEvent = serde_json::from_str(&prev_json)?;

            // Find and replace the event in the list
            let raw: Vec<String> = conn.lrange(&events_key, 0, -1).await?;
            let new_event_json = serde_json::to_string(&event)?;

            // Search from the end (rposition equivalent)
            for (i, item) in raw.iter().enumerate().rev() {
                if let Ok(e) = serde_json::from_str::<TaskEvent>(item) {
                    if e.id == prev.id {
                        conn.lset::<_, _, ()>(&events_key, i as isize, &new_event_json)
                            .await?;
                        break;
                    }
                }
            }
        } else {
            // No previous -- just append
            self.append_event(task_id, event.clone()).await?;
        }

        // Update series latest
        let json = serde_json::to_string(&event)?;
        conn.set::<_, _, ()>(&series_key, &json).await?;
        conn.sadd::<_, _, ()>(&self.keys.series_ids(task_id), series_id)
            .await?;

        Ok(())
    }

    async fn next_index(
        &self,
        task_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.idx(task_id);
        let mut conn = self.conn.clone();
        // INCR is atomic -- safe across multiple instances sharing the same Redis.
        // Returns 1-based, so subtract 1 to get 0-based index.
        let val: i64 = conn.incr(&key, 1).await?;
        Ok((val - 1) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_generation_default_prefix() {
        let keys = Keys::new("taskcast");
        assert_eq!(keys.task("t1"), "taskcast:task:t1");
        assert_eq!(keys.events("t1"), "taskcast:events:t1");
        assert_eq!(keys.idx("t1"), "taskcast:idx:t1");
        assert_eq!(
            keys.series_latest("t1", "s1"),
            "taskcast:series:t1:s1"
        );
        assert_eq!(keys.series_ids("t1"), "taskcast:seriesIds:t1");
    }

    #[test]
    fn key_generation_custom_prefix() {
        let keys = Keys::new("myapp");
        assert_eq!(keys.task("task_123"), "myapp:task:task_123");
        assert_eq!(keys.events("task_123"), "myapp:events:task_123");
        assert_eq!(keys.idx("task_123"), "myapp:idx:task_123");
        assert_eq!(
            keys.series_latest("task_123", "progress"),
            "myapp:series:task_123:progress"
        );
        assert_eq!(keys.series_ids("task_123"), "myapp:seriesIds:task_123");
    }

    #[test]
    fn key_generation_empty_ids() {
        let keys = Keys::new("tc");
        assert_eq!(keys.task(""), "tc:task:");
        assert_eq!(keys.events(""), "tc:events:");
        assert_eq!(keys.idx(""), "tc:idx:");
    }

    #[test]
    fn key_generation_special_characters() {
        let keys = Keys::new("taskcast");
        assert_eq!(keys.task("a:b:c"), "taskcast:task:a:b:c");
        assert_eq!(
            keys.series_latest("task-1", "series/2"),
            "taskcast:series:task-1:series/2"
        );
    }
}
