use async_trait::async_trait;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;

use taskcast_core::types::{
    EventQueryOptions, ShortTermStore, Task, TaskEvent, TaskFilter, Worker, WorkerAssignment,
    WorkerFilter,
};

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

    /// `{prefix}:tasks` -- SET of all task IDs.
    fn tasks_set(&self) -> String {
        format!("{}:tasks", self.prefix)
    }

    /// `{prefix}:worker:{id}` -- stores the full Worker JSON.
    fn worker(&self, id: &str) -> String {
        format!("{}:worker:{}", self.prefix, id)
    }

    /// `{prefix}:workers` -- SET of all worker IDs.
    fn workers_set(&self) -> String {
        format!("{}:workers", self.prefix)
    }

    /// `{prefix}:assignment:{taskId}` -- stores the WorkerAssignment JSON.
    fn assignment(&self, task_id: &str) -> String {
        format!("{}:assignment:{}", self.prefix, task_id)
    }

    /// `{prefix}:workerAssignments:{workerId}` -- SET of task IDs for a worker's assignments.
    fn worker_assignments(&self, worker_id: &str) -> String {
        format!("{}:workerAssignments:{}", self.prefix, worker_id)
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
        let tasks_set_key = self.keys.tasks_set();
        let json = serde_json::to_string(&task)?;
        let mut conn = self.conn.clone();
        conn.set::<_, _, ()>(&key, &json).await?;
        conn.sadd::<_, _, ()>(&tasks_set_key, &task.id).await?;
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

    // ─── Task query ──────────────────────────────────────────────────────

    async fn list_tasks(
        &self,
        filter: TaskFilter,
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let tasks_set_key = self.keys.tasks_set();
        let mut conn = self.conn.clone();

        let task_ids: Vec<String> = conn.smembers(&tasks_set_key).await?;
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build task keys for MGET
        let task_keys: Vec<String> = task_ids.iter().map(|id| self.keys.task(id)).collect();
        let raw: Vec<Option<String>> = conn.mget(&task_keys).await?;

        let mut tasks: Vec<Task> = raw
            .into_iter()
            .filter_map(|opt| opt.and_then(|s| serde_json::from_str(&s).ok()))
            .collect();

        // Apply filters in Rust
        if let Some(ref statuses) = filter.status {
            tasks.retain(|t| statuses.contains(&t.status));
        }
        if let Some(ref types) = filter.types {
            tasks.retain(|t| match &t.r#type {
                Some(task_type) => types.contains(task_type),
                None => false,
            });
        }
        if let Some(ref tag_matcher) = filter.tags {
            tasks.retain(|t| {
                let task_tags = t.tags.as_deref().unwrap_or(&[]);
                // all: every tag in the filter must be present
                if let Some(ref all) = tag_matcher.all {
                    if !all.iter().all(|tag| task_tags.contains(tag)) {
                        return false;
                    }
                }
                // any: at least one tag must be present
                if let Some(ref any) = tag_matcher.any {
                    if !any.iter().any(|tag| task_tags.contains(tag)) {
                        return false;
                    }
                }
                // none: no tag in the filter should be present
                if let Some(ref none) = tag_matcher.none {
                    if none.iter().any(|tag| task_tags.contains(tag)) {
                        return false;
                    }
                }
                true
            });
        }
        if let Some(ref assign_modes) = filter.assign_mode {
            tasks.retain(|t| match &t.assign_mode {
                Some(mode) => assign_modes.contains(mode),
                None => false,
            });
        }
        if let Some(ref exclude_ids) = filter.exclude_task_ids {
            tasks.retain(|t| !exclude_ids.contains(&t.id));
        }
        if let Some(limit) = filter.limit {
            tasks.truncate(limit as usize);
        }

        Ok(tasks)
    }

    // ─── Worker state ────────────────────────────────────────────────────

    async fn save_worker(
        &self,
        worker: Worker,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.worker(&worker.id);
        let workers_set_key = self.keys.workers_set();
        let json = serde_json::to_string(&worker)?;
        let mut conn = self.conn.clone();
        conn.set::<_, _, ()>(&key, &json).await?;
        conn.sadd::<_, _, ()>(&workers_set_key, &worker.id).await?;
        Ok(())
    }

    async fn get_worker(
        &self,
        worker_id: &str,
    ) -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.worker(worker_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = conn.get(&key).await?;
        match result {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    async fn list_workers(
        &self,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        let workers_set_key = self.keys.workers_set();
        let mut conn = self.conn.clone();

        let worker_ids: Vec<String> = conn.smembers(&workers_set_key).await?;
        if worker_ids.is_empty() {
            return Ok(Vec::new());
        }

        let worker_keys: Vec<String> = worker_ids.iter().map(|id| self.keys.worker(id)).collect();
        let raw: Vec<Option<String>> = conn.mget(&worker_keys).await?;

        let mut workers: Vec<Worker> = raw
            .into_iter()
            .filter_map(|opt| opt.and_then(|s| serde_json::from_str(&s).ok()))
            .collect();

        // Apply filter in Rust
        if let Some(ref f) = filter {
            if let Some(ref statuses) = f.status {
                workers.retain(|w| statuses.contains(&w.status));
            }
            if let Some(ref modes) = f.connection_mode {
                workers.retain(|w| modes.contains(&w.connection_mode));
            }
        }

        Ok(workers)
    }

    async fn delete_worker(
        &self,
        worker_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.worker(worker_id);
        let workers_set_key = self.keys.workers_set();
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(&key).await?;
        conn.srem::<_, _, ()>(&workers_set_key, worker_id).await?;
        Ok(())
    }

    // ─── Atomic claim ────────────────────────────────────────────────────

    async fn claim_task(
        &self,
        task_id: &str,
        worker_id: &str,
        cost: u32,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let task_key = self.keys.task(task_id);
        let worker_key = self.keys.worker(worker_id);

        let lua = r#"
            local taskJson = redis.call('GET', KEYS[1])
            if not taskJson then return 0 end
            local task = cjson.decode(taskJson)
            if task.status ~= 'pending' and task.status ~= 'assigned' then return 0 end

            local workerJson = redis.call('GET', KEYS[2])
            if not workerJson then return 0 end
            local worker = cjson.decode(workerJson)
            local cost = tonumber(ARGV[1])
            if worker.usedSlots + cost > worker.capacity then return 0 end

            worker.usedSlots = worker.usedSlots + cost
            redis.call('SET', KEYS[2], cjson.encode(worker))

            task.status = 'assigned'
            task.assignedWorker = ARGV[2]
            task.cost = cost
            task.updatedAt = tonumber(ARGV[3])
            redis.call('SET', KEYS[1], cjson.encode(task))

            return 1
        "#;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs_f64();
        let timestamp_ms = (now * 1000.0) as u64;

        let script = redis::Script::new(lua);
        let mut conn = self.conn.clone();
        let result: i32 = script
            .key(&task_key)
            .key(&worker_key)
            .arg(cost)
            .arg(worker_id)
            .arg(timestamp_ms)
            .invoke_async(&mut conn)
            .await?;

        Ok(result == 1)
    }

    // ─── Worker assignments ──────────────────────────────────────────────

    async fn add_assignment(
        &self,
        assignment: WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let assignment_key = self.keys.assignment(&assignment.task_id);
        let worker_assignments_key = self.keys.worker_assignments(&assignment.worker_id);
        let json = serde_json::to_string(&assignment)?;
        let mut conn = self.conn.clone();
        conn.set::<_, _, ()>(&assignment_key, &json).await?;
        conn.sadd::<_, _, ()>(&worker_assignments_key, &assignment.task_id)
            .await?;
        Ok(())
    }

    async fn remove_assignment(
        &self,
        task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let assignment_key = self.keys.assignment(task_id);
        let mut conn = self.conn.clone();

        // First, get the assignment to find the worker ID
        let result: Option<String> = conn.get(&assignment_key).await?;
        if let Some(json) = result {
            let assignment: WorkerAssignment = serde_json::from_str(&json)?;
            let worker_assignments_key = self.keys.worker_assignments(&assignment.worker_id);
            conn.srem::<_, _, ()>(&worker_assignments_key, task_id)
                .await?;
        }

        conn.del::<_, ()>(&assignment_key).await?;
        Ok(())
    }

    async fn get_worker_assignments(
        &self,
        worker_id: &str,
    ) -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        let worker_assignments_key = self.keys.worker_assignments(worker_id);
        let mut conn = self.conn.clone();

        let task_ids: Vec<String> = conn.smembers(&worker_assignments_key).await?;
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }

        let assignment_keys: Vec<String> =
            task_ids.iter().map(|id| self.keys.assignment(id)).collect();
        let raw: Vec<Option<String>> = conn.mget(&assignment_keys).await?;

        let assignments: Vec<WorkerAssignment> = raw
            .into_iter()
            .filter_map(|opt| opt.and_then(|s| serde_json::from_str(&s).ok()))
            .collect();

        Ok(assignments)
    }

    async fn get_task_assignment(
        &self,
        task_id: &str,
    ) -> Result<Option<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.assignment(task_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = conn.get(&key).await?;
        match result {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
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

    #[test]
    fn key_generation_tasks_set() {
        let keys = Keys::new("taskcast");
        assert_eq!(keys.tasks_set(), "taskcast:tasks");
    }

    #[test]
    fn key_generation_worker() {
        let keys = Keys::new("taskcast");
        assert_eq!(keys.worker("w1"), "taskcast:worker:w1");
    }

    #[test]
    fn key_generation_workers_set() {
        let keys = Keys::new("taskcast");
        assert_eq!(keys.workers_set(), "taskcast:workers");
    }

    #[test]
    fn key_generation_assignment() {
        let keys = Keys::new("taskcast");
        assert_eq!(keys.assignment("t1"), "taskcast:assignment:t1");
    }

    #[test]
    fn key_generation_worker_assignments() {
        let keys = Keys::new("taskcast");
        assert_eq!(
            keys.worker_assignments("w1"),
            "taskcast:workerAssignments:w1"
        );
    }

    #[test]
    fn key_generation_worker_custom_prefix() {
        let keys = Keys::new("myapp");
        assert_eq!(keys.worker("worker_abc"), "myapp:worker:worker_abc");
        assert_eq!(keys.workers_set(), "myapp:workers");
        assert_eq!(keys.assignment("task_123"), "myapp:assignment:task_123");
        assert_eq!(
            keys.worker_assignments("worker_abc"),
            "myapp:workerAssignments:worker_abc"
        );
        assert_eq!(keys.tasks_set(), "myapp:tasks");
    }
}
