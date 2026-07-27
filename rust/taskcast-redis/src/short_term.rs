use async_trait::async_trait;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;
use std::sync::Arc;

use taskcast_core::types::{
    ArchiveSourcePage, ClosedWriteFence, EventQueryOptions, HotWriteToken, RehydrateSnapshot,
    SeriesMode, SeriesResult, ShortTermStore, StorageFenceConflictError, StorageIntegrityError,
    StorageLease, StorageWriterRegistration, Task, TaskEvent, TaskFilter, TaskMutationSnapshot,
    TaskStoragePresence, TaskWriteFence, TerminalProjection, TerminalProjectionResult, Worker,
    WorkerAssignment, WorkerFilter,
};
use taskcast_core::{BoxError, DependencyObserver};

use crate::connection::RedisCommandConnection;

macro_rules! redis_call {
    ($connection:ident, $operation:expr) => {{
        let result = $operation.await;
        $connection.observe_result(result)?
    }};
}

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

    fn task_status(&self, id: &str) -> String {
        format!("{}:taskStatus:{}", self.prefix, id)
    }

    /// `{prefix}:events:{id}` -- a Redis list of event JSONs.
    fn events(&self, id: &str) -> String {
        format!("{}:events:{}", self.prefix, id)
    }

    /// `{prefix}:idx:{id}` -- atomic index counter (INCR).
    fn idx(&self, id: &str) -> String {
        format!("{}:idx:{}", self.prefix, id)
    }

    fn series_state(&self, task_id: &str) -> String {
        format!("{}:seriesState:{}", self.prefix, task_id)
    }

    fn series_list_entries(&self, task_id: &str) -> String {
        format!("{}:seriesListEntries:{}", self.prefix, task_id)
    }

    /// Legacy `{prefix}:series:{taskId}:{seriesId}` latest-event key.
    fn legacy_series_latest(&self, task_id: &str, series_id: &str) -> String {
        format!("{}:series:{}:{}", self.prefix, task_id, series_id)
    }

    /// Legacy `{prefix}:seriesIds:{taskId}` series-ID set.
    fn legacy_series_ids(&self, task_id: &str) -> String {
        format!("{}:seriesIds:{}", self.prefix, task_id)
    }

    fn write_fence(&self, task_id: &str) -> String {
        format!("{}:writeFence:{}", self.prefix, task_id)
    }

    fn storage_lock(&self, task_id: &str) -> String {
        format!("{}:storageLock:{}", self.prefix, task_id)
    }

    fn hot_window(&self, task_id: &str) -> String {
        format!("{}:hotWindow:{}", self.prefix, task_id)
    }

    fn storage_writers(&self) -> String {
        format!("{}:storageWriters", self.prefix)
    }

    fn storage_writer(&self, instance_id: &str) -> String {
        format!("{}:storageWriter:{}", self.prefix, instance_id)
    }

    fn series_prefix(&self, task_id: &str) -> String {
        format!("{}:series:{}:", self.prefix, task_id)
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

    fn terminal_projection(&self, projection_id: &str) -> String {
        format!("{}:terminalProjection:{}", self.prefix, projection_id)
    }
}

/// Redis-backed short-term store.
///
/// Uses Redis data structures to persist tasks, events, series tracking,
/// and atomic index counters.
pub struct RedisShortTermStore {
    conn: RedisCommandConnection,
    keys: Keys,
    legacy_series_writes: bool,
}

impl RedisShortTermStore {
    const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

    /// Create a new `RedisShortTermStore`.
    ///
    /// - `conn`: a multiplexed Redis connection for all read/write operations.
    /// - `prefix`: key prefix (defaults to `"taskcast"`).
    pub fn new(conn: MultiplexedConnection, prefix: Option<&str>) -> Self {
        Self::with_command_connection(conn.into(), prefix)
    }

    #[allow(dead_code)] // Used by the managed adapter composition added in Task 4.
    pub(crate) fn new_managed(
        manager: redis::aio::ConnectionManager,
        prefix: Option<&str>,
        observer: Option<Arc<dyn DependencyObserver>>,
    ) -> Self {
        Self::with_command_connection(RedisCommandConnection::managed(manager, observer), prefix)
    }

    fn with_command_connection(conn: RedisCommandConnection, prefix: Option<&str>) -> Self {
        let resolved_prefix = prefix.unwrap_or("taskcast");
        Self {
            conn,
            keys: Keys::new(resolved_prefix),
            legacy_series_writes: std::env::var("TASKCAST_REDIS_LEGACY_SERIES_WRITES")
                .map(|value| value.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }

    pub fn with_legacy_series_writes(mut self, enabled: bool) -> Self {
        self.legacy_series_writes = enabled;
        self
    }

    /// Returns a reference to the key helper for testing or introspection.
    pub fn key_prefix(&self) -> &str {
        &self.keys.prefix
    }

    fn map_fence_error(error: redis::RedisError) -> Box<dyn std::error::Error + Send + Sync> {
        if error.to_string().contains("STORAGE_FENCE_CONFLICT") {
            Box::new(StorageFenceConflictError::default())
        } else if error.to_string().contains("STORAGE_INTEGRITY_ERROR") {
            Box::new(StorageIntegrityError::new(
                "Redis storage state failed integrity validation",
            ))
        } else {
            Box::new(error)
        }
    }

    fn observe_fence_result<T>(
        connection: &RedisCommandConnection,
        result: redis::RedisResult<T>,
    ) -> Result<T, BoxError> {
        match connection.observe_result(result) {
            Ok(value) => Ok(value),
            Err(error) => match error.downcast::<redis::RedisError>() {
                Ok(error) => Err(Self::map_fence_error(*error)),
                Err(error) => Err(error),
            },
        }
    }

    fn encode_archive_cursor(watermark: i64, offset: usize, last_index: i64) -> String {
        format!("tc1|{watermark}|{offset}|{last_index}")
    }

    fn decode_archive_cursor(
        cursor: Option<&str>,
        watermark: i64,
    ) -> Result<(usize, i64), Box<dyn std::error::Error + Send + Sync>> {
        let Some(cursor) = cursor else {
            return Ok((0, -1));
        };
        let parts = cursor.split('|').collect::<Vec<_>>();
        if parts.len() != 4 || parts[0] != "tc1" {
            return Err(Box::new(StorageIntegrityError::new(
                "Invalid archive source cursor",
            )));
        }
        let cursor_watermark = parts[1].parse::<i64>().map_err(|_| {
            Box::new(StorageIntegrityError::new("Invalid archive source cursor"))
                as Box<dyn std::error::Error + Send + Sync>
        })?;
        let offset = parts[2].parse::<usize>().map_err(|_| {
            Box::new(StorageIntegrityError::new("Invalid archive source cursor"))
                as Box<dyn std::error::Error + Send + Sync>
        })?;
        let last_index = parts[3].parse::<i64>().map_err(|_| {
            Box::new(StorageIntegrityError::new("Invalid archive source cursor"))
                as Box<dyn std::error::Error + Send + Sync>
        })?;
        if cursor_watermark != watermark {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive source cursor does not match the request",
            )));
        }
        Ok((offset, last_index))
    }

    fn validate_rehydrate_snapshot(
        snapshot: &RehydrateSnapshot,
        lease: &StorageLease,
        next_epoch: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if snapshot.task.id != lease.task_id || next_epoch <= snapshot.storage_epoch {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        if snapshot.max_event_index < -1
            || snapshot.max_event_index >= Self::MAX_SAFE_INTEGER
            || snapshot.archive_watermark > snapshot.max_event_index
        {
            return Err(Box::new(StorageIntegrityError::new(
                "Invalid durable event bounds for rehydration",
            )));
        }
        let mut previous_index = -1;
        for event in &snapshot.replay_events {
            let event_index = i64::try_from(event.index).map_err(|_| {
                Box::new(StorageIntegrityError::new(
                    "Rehydrate replay event index exceeds safe bounds",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
            if event.task_id != lease.task_id
                || event_index <= previous_index
                || event_index > snapshot.max_event_index
            {
                return Err(Box::new(StorageIntegrityError::new(
                    "Rehydrate replay events are not strictly ordered",
                )));
            }
            previous_index = event_index;
        }
        let mut series_ids = std::collections::HashSet::new();
        for entry in &snapshot.series_latest {
            let event_index = i64::try_from(entry.event.index).map_err(|_| {
                Box::new(StorageIntegrityError::new(
                    "Durable series event index exceeds safe bounds",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
            let through_index = i64::try_from(entry.through_index).map_err(|_| {
                Box::new(StorageIntegrityError::new(
                    "Durable series index exceeds safe bounds",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
            if entry.task_id != lease.task_id
                || entry.event.task_id != lease.task_id
                || entry.event.series_id.as_deref() != Some(entry.series_id.as_str())
                || entry.event.series_mode.as_ref() != Some(&entry.mode)
                || event_index > through_index
                || through_index > snapshot.max_event_index
                || !series_ids.insert(entry.series_id.as_str())
            {
                return Err(Box::new(StorageIntegrityError::new(
                    "Invalid durable series state for rehydration",
                )));
            }
        }
        Ok(())
    }

    fn make_indexed_event_template(
        event: &TaskEvent,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        let mut template = event.clone();
        template.index = 0;
        let encoded = serde_json::to_string(&template)?;
        let marker = "\"index\":0";
        let marker_offset = encoded.find(marker).ok_or_else(|| {
            Box::new(StorageIntegrityError::new(
                "Unable to build an opaque indexed event",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        let number_offset = marker_offset + marker.len() - 1;
        Ok((
            encoded[..number_offset].to_string(),
            encoded[number_offset + 1..].to_string(),
        ))
    }

    fn parse_series_state(
        raw: &str,
        task_id: &str,
        series_id: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        let event: TaskEvent = serde_json::from_str(raw).map_err(|_| {
            Box::new(StorageIntegrityError::new(
                "Series state contains invalid event JSON",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        if event.task_id != task_id
            || event
                .series_id
                .as_deref()
                .is_some_and(|stored| stored != series_id)
            || event.index > Self::MAX_SAFE_INTEGER as u64
        {
            return Err(Box::new(StorageIntegrityError::new(
                "Series state does not match its task and series",
            )));
        }
        Ok(event)
    }

    fn select_series_state(
        hash_state_raw: &str,
        legacy_candidate_raw: &str,
        task_id: &str,
        series_id: &str,
    ) -> Result<(String, Option<TaskEvent>, String), Box<dyn std::error::Error + Send + Sync>> {
        let hash_event = (!hash_state_raw.is_empty())
            .then(|| Self::parse_series_state(hash_state_raw, task_id, series_id))
            .transpose()?;
        let mut legacy_state_raw = String::new();
        let mut legacy_event = None;
        if !legacy_candidate_raw.is_empty() {
            let candidate: TaskEvent =
                serde_json::from_str(legacy_candidate_raw).map_err(|_| {
                    Box::new(StorageIntegrityError::new(
                        "Legacy series state contains invalid event JSON",
                    )) as Box<dyn std::error::Error + Send + Sync>
                })?;
            if candidate.task_id == task_id
                && candidate
                    .series_id
                    .as_deref()
                    .is_none_or(|stored| stored == series_id)
                && candidate.index <= Self::MAX_SAFE_INTEGER as u64
            {
                legacy_state_raw = legacy_candidate_raw.to_string();
                legacy_event = Some(candidate);
            }
        }

        match (hash_event, legacy_event) {
            (None, legacy) => Ok((legacy_state_raw.clone(), legacy, legacy_state_raw)),
            (Some(hash), None) => Ok((hash_state_raw.to_string(), Some(hash), legacy_state_raw)),
            (Some(hash), Some(legacy)) if hash.index == legacy.index => {
                if hash_state_raw != legacy_state_raw {
                    return Err(Box::new(StorageIntegrityError::new(
                        "Hash and legacy series state conflict at the same index",
                    )));
                }
                Ok((hash_state_raw.to_string(), Some(hash), legacy_state_raw))
            }
            // New writers update both representations atomically in
            // compatibility mode (or delete legacy in fixed mode). A
            // differing legacy value can therefore only be a later old-writer
            // update, even if it reserved its event index earlier.
            (Some(_), Some(legacy)) => {
                Ok((legacy_state_raw.clone(), Some(legacy), legacy_state_raw))
            }
        }
    }

    fn parse_event_list_head(
        raw: &str,
        task_id: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        let event: TaskEvent = serde_json::from_str(raw).map_err(|_| {
            Box::new(StorageIntegrityError::new(
                "Event list contains invalid event JSON",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        if event.task_id != task_id || event.index > Self::MAX_SAFE_INTEGER as u64 {
            return Err(Box::new(StorageIntegrityError::new(
                "Event list head does not match its task",
            )));
        }
        Ok(event)
    }

    fn accumulate_event(previous: Option<&TaskEvent>, event: &TaskEvent, field: &str) -> TaskEvent {
        let Some(previous_data) = previous.and_then(|value| value.data.as_object()) else {
            return event.clone();
        };
        let Some(next_data) = event.data.as_object() else {
            return event.clone();
        };
        let (
            Some(serde_json::Value::String(previous_value)),
            Some(serde_json::Value::String(next_value)),
        ) = (previous_data.get(field), next_data.get(field))
        else {
            return event.clone();
        };
        let mut accumulated = event.clone();
        accumulated
            .data
            .as_object_mut()
            .expect("object checked above")
            .insert(
                field.to_string(),
                serde_json::Value::String(format!("{previous_value}{next_value}")),
            );
        accumulated
    }

    async fn scan_series_keys(
        &self,
        task_id: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let mut escaped_prefix = String::new();
        for character in self.keys.series_prefix(task_id).chars() {
            if matches!(character, '\\' | '*' | '?' | '[' | ']') {
                escaped_prefix.push('\\');
            }
            escaped_prefix.push(character);
        }
        let pattern = format!("{escaped_prefix}*");
        let mut cursor = 0_u64;
        let mut keys = std::collections::HashSet::new();
        loop {
            let (next_cursor, page): (u64, Vec<String>) = redis_call!(
                conn,
                redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(1000)
                    .query_async(&mut conn)
            );
            if !page.is_empty() {
                let values: Vec<Option<String>> = redis_call!(conn, conn.mget(&page));
                for (key, raw) in page.into_iter().zip(values) {
                    let Some(raw) = raw else {
                        continue;
                    };
                    let event: TaskEvent = serde_json::from_str(&raw).map_err(|_| {
                        Box::new(StorageIntegrityError::new(
                            "Legacy series state contains invalid event JSON",
                        )) as Box<dyn std::error::Error + Send + Sync>
                    })?;
                    if event.task_id == task_id {
                        keys.insert(key);
                    }
                    if keys.len() > 1000 {
                        return Err(Box::new(StorageIntegrityError::new(
                            "Legacy series state exceeds the bounded migration limit",
                        )));
                    }
                }
            }
            cursor = next_cursor;
            if cursor == 0 {
                return Ok(keys.into_iter().collect());
            }
        }
    }

    fn now_ms() -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs_f64()
            * 1000.0)
    }
}

const SAVE_TASK_LUA: &str = r#"
    redis.call('SET', KEYS[1], ARGV[1])
    redis.call('SADD', KEYS[2], ARGV[2])
    redis.call('SETNX', KEYS[3], ARGV[3])
    redis.call('SET', KEYS[4], ARGV[4])
    return 1
"#;

const ACQUIRE_STORAGE_LOCK_LUA: &str = r#"
    local currentJson = redis.call('GET', KEYS[1])
    if currentJson then
      local current = cjson.decode(currentJson)
      if current.taskId == ARGV[1]
         and current.lockToken == ARGV[2]
         and current.generation == ARGV[3] then
        redis.call('PEXPIRE', KEYS[1], tonumber(ARGV[4]))
        return currentJson
      end
      return false
    end

    local epoch = 1
    local fenceJson = redis.call('GET', KEYS[2])
    if fenceJson then
      local fence = cjson.decode(fenceJson)
      epoch = fence.storageEpoch
    end
    local lease = {
      taskId = ARGV[1],
      lockToken = ARGV[2],
      generation = ARGV[3],
      storageEpoch = epoch
    }
    local encoded = cjson.encode(lease)
    redis.call('SET', KEYS[1], encoded, 'PX', tonumber(ARGV[4]))
    return encoded
"#;

const RENEW_STORAGE_LOCK_LUA: &str = r#"
    local currentJson = redis.call('GET', KEYS[1])
    if not currentJson then return 0 end
    local current = cjson.decode(currentJson)
    if current.taskId ~= ARGV[1]
       or current.lockToken ~= ARGV[2]
       or current.generation ~= ARGV[3]
       or current.storageEpoch ~= tonumber(ARGV[4]) then
      return 0
    end
    redis.call('PEXPIRE', KEYS[1], tonumber(ARGV[5]))
    return 1
"#;

const RELEASE_STORAGE_LOCK_LUA: &str = r#"
    local currentJson = redis.call('GET', KEYS[1])
    if not currentJson then return 0 end
    local current = cjson.decode(currentJson)
    if current.taskId ~= ARGV[1]
       or current.lockToken ~= ARGV[2]
       or current.generation ~= ARGV[3]
       or current.storageEpoch ~= tonumber(ARGV[4]) then
      return 0
    end
    redis.call('DEL', KEYS[1])
    return 1
"#;

const CLOSE_WRITE_FENCE_LUA: &str = r#"
    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.taskId ~= ARGV[1]
       or (fence.acceptingWrites ~= true and fence.acceptingWrites ~= false)
       or fence.storageEpoch ~= tonumber(ARGV[5]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local nextIndexJson = redis.call('GET', KEYS[3])
    local highWatermarkJson = '-1'
    if nextIndexJson then
      if not string.match(nextIndexJson, '^[0-9]+$')
         or (#nextIndexJson > 1 and string.sub(nextIndexJson, 1, 1) == '0')
         or #nextIndexJson > 16
         or (#nextIndexJson == 16 and nextIndexJson > '9007199254740991') then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
      redis.call('DECR', KEYS[3])
      highWatermarkJson = redis.call('GET', KEYS[3])
      redis.call('INCR', KEYS[3])
    end
    local closed = {
      taskId = ARGV[1],
      acceptingWrites = false,
      storageEpoch = tonumber(ARGV[5]),
      activeReleaseGeneration = ARGV[3]
    }
    local encoded = cjson.encode(closed)
    redis.call('SET', KEYS[2], encoded)
    return { encoded, highWatermarkJson }
"#;

const REOPEN_WRITE_FENCE_LUA: &str = r#"
    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.acceptingWrites ~= false
       or fence.storageEpoch ~= tonumber(ARGV[5])
       or fence.activeReleaseGeneration ~= ARGV[3] then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local nextEpoch = tonumber(ARGV[5]) + 1
    local reopened = {
      taskId = ARGV[1],
      acceptingWrites = true,
      storageEpoch = nextEpoch,
      activeReleaseGeneration = cjson.null
    }
    redis.call('SET', KEYS[2], cjson.encode(reopened))
    return cjson.encode({ taskId = ARGV[1], storageEpoch = nextEpoch })
"#;

const COMMIT_EVENT_FENCED_LUA: &str = r#"
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    local function validIndex(value, maximum)
      return string.match(value, '^[0-9]+$')
        and (#value == 1 or string.sub(value, 1, 1) ~= '0')
        and (#value < #maximum or (#value == #maximum and value <= maximum))
    end

    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'list')
       or not validType(KEYS[4], 'hash')
       or not validType(KEYS[5], 'hash')
       or not validType(KEYS[6], 'string')
       or not validType(KEYS[7], 'set')
       or not validType(KEYS[8], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local fenceJson = redis.call('GET', KEYS[1])
    if not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local fence = cjson.decode(fenceJson)
    if fence.acceptingWrites ~= true or fence.storageEpoch ~= tonumber(ARGV[1]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local indexJson = redis.call('GET', KEYS[2]) or '0'
    if not validIndex(indexJson, '9007199254740990') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local currentHashState = redis.call('HGET', KEYS[4], ARGV[4])
    local currentLegacyState = redis.call('GET', KEYS[6])
    if (ARGV[5] == 'latest' or ARGV[5] == 'accumulate')
       and ARGV[4] ~= ''
       and (
         (currentHashState or '') ~= ARGV[15]
         or (currentLegacyState or '') ~= ARGV[16]
       ) then
      return { 'RETRY', '' }
    end
    local currentState = ARGV[6] ~= '' and ARGV[6] or nil

    local currentFirst = redis.call('LINDEX', KEYS[3], 0)
    local currentSecond = redis.call('LINDEX', KEYS[3], 1)
    if (currentFirst or '') ~= ARGV[9]
       or (currentSecond or '') ~= ARGV[10] then
      return { 'RETRY', '' }
    end
    if currentFirst and not validIndex(ARGV[11], '9007199254740991') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    if currentSecond and not validIndex(ARGV[12], '9007199254740991') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local eventJson = ARGV[2] .. indexJson .. ARGV[3]
    local removeHead = false
    local seriesWriteJson = ''

    if ARGV[5] == 'latest' and ARGV[4] ~= '' then
      local previousListJson = redis.call('HGET', KEYS[5], ARGV[4])
      if ARGV[13] == '1' and currentState and ARGV[6] ~= ARGV[15] then
        previousListJson = currentState
      elseif not previousListJson and ARGV[13] == '1' then
        previousListJson = currentState
      end
      if previousListJson then
        removeHead = currentFirst == previousListJson
        redis.call('LREM', KEYS[3], -1, previousListJson)
      end
      redis.call('RPUSH', KEYS[3], eventJson)
      redis.call('HSET', KEYS[4], ARGV[4], eventJson)
      redis.call('HSET', KEYS[5], ARGV[4], eventJson)
      seriesWriteJson = eventJson
    elseif ARGV[5] == 'accumulate' and ARGV[4] ~= '' then
      redis.call('RPUSH', KEYS[3], eventJson)
      local accumulatedJson = ARGV[7] .. indexJson .. ARGV[8]
      redis.call('HSET', KEYS[4], ARGV[4], accumulatedJson)
      redis.call('HDEL', KEYS[5], ARGV[4])
      seriesWriteJson = accumulatedJson
    else
      redis.call('RPUSH', KEYS[3], eventJson)
    end

    if seriesWriteJson ~= '' then
      if ARGV[17] == '1' then
        redis.call('SET', KEYS[6], seriesWriteJson)
        redis.call('SADD', KEYS[7], ARGV[4])
      elseif ARGV[14] == '1' then
        redis.call('DEL', KEYS[6])
        redis.call('SREM', KEYS[7], ARGV[4])
      end
    end
    redis.call('INCR', KEYS[2])
    local firstIndexJson = indexJson
    if currentFirst and not removeHead then
      firstIndexJson = ARGV[11]
    elseif currentSecond then
      firstIndexJson = ARGV[12]
    end
    redis.call(
      'SET',
      KEYS[8],
      '{"firstIndex":' .. firstIndexJson
        .. ',"lastIndex":' .. indexJson .. '}'
    )

    return { 'COMMITTED', indexJson }
"#;

const SAVE_TASK_FENCED_LUA: &str = r#"
    local fenceJson = redis.call('GET', KEYS[1])
    if not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local fence = cjson.decode(fenceJson)
    if fence.acceptingWrites ~= true or fence.storageEpoch ~= tonumber(ARGV[1]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    redis.call('SET', KEYS[2], ARGV[2])
    redis.call('SET', KEYS[3], ARGV[3])
    return 1
"#;

const COMMIT_TASK_EVENTS_FENCED_LUA: &str = r#"
local function validType(key, expected)
  local actual = redis.call('TYPE', key).ok
  return actual == 'none' or actual == expected
end
local function validIndex(value, maximum)
  return string.match(value, '^[0-9]+$')
    and (#value == 1 or string.sub(value, 1, 1) ~= '0')
    and (#value < #maximum or (#value == #maximum and value <= maximum))
end
local function increment(value)
  local carry = 1
  local result = ''
  for position = #value, 1, -1 do
    local digit = tonumber(string.sub(value, position, position)) + carry
    if digit >= 10 then
      digit = digit - 10
      carry = 1
    else
      carry = 0
    end
    result = tostring(digit) .. result
  end
  if carry == 1 then result = '1' .. result end
  return result
end

if not validType(KEYS[1], 'string')
   or not validType(KEYS[2], 'string')
   or not validType(KEYS[3], 'string')
   or not validType(KEYS[4], 'list')
   or not validType(KEYS[5], 'string')
   or not validType(KEYS[6], 'string') then
  return redis.error_reply('STORAGE_INTEGRITY_ERROR')
end
local fenceJson = redis.call('GET', KEYS[1])
if not fenceJson then
  return redis.error_reply('STORAGE_FENCE_CONFLICT')
end
local fence = cjson.decode(fenceJson)
if fence.acceptingWrites ~= true or fence.storageEpoch ~= tonumber(ARGV[1]) then
  return redis.error_reply('STORAGE_FENCE_CONFLICT')
end
local currentTaskJson = redis.call('GET', KEYS[2])
if not currentTaskJson then
  return redis.error_reply('STORAGE_INTEGRITY_ERROR')
end
if currentTaskJson ~= ARGV[3] then
  return { 'TASK_CONFLICT' }
end
local eventCount = tonumber(ARGV[5])
if not eventCount or eventCount < 1 or eventCount > 16
   or eventCount ~= math.floor(eventCount)
   or #ARGV ~= 5 + eventCount * 2 then
  return redis.error_reply('STORAGE_INTEGRITY_ERROR')
end
local indexJson = redis.call('GET', KEYS[3]) or '0'
if not validIndex(indexJson, '9007199254740990') then
  return redis.error_reply('STORAGE_INTEGRITY_ERROR')
end
local finalIndex = indexJson
for _ = 1, eventCount do
  finalIndex = increment(finalIndex)
end
if not validIndex(finalIndex, '9007199254740991') then
  return redis.error_reply('STORAGE_INTEGRITY_ERROR')
end
local originalIndex = indexJson
local committed = { 'COMMITTED', '' }
redis.call('SET', KEYS[2], ARGV[2])
redis.call('SET', KEYS[6], ARGV[4])
for ordinal = 0, eventCount - 1 do
  if not validIndex(indexJson, '9007199254740991') then
    return redis.error_reply('STORAGE_INTEGRITY_ERROR')
  end
  local eventJson = ARGV[6 + ordinal * 2] .. indexJson
    .. ARGV[7 + ordinal * 2]
  redis.call('RPUSH', KEYS[4], eventJson)
  table.insert(committed, eventJson)
  indexJson = increment(indexJson)
end
redis.call('SET', KEYS[3], indexJson)
committed[2] = indexJson

local firstIndex = originalIndex
local existingWindow = redis.call('GET', KEYS[5])
if existingWindow then
  local decoded = cjson.decode(existingWindow)
  if decoded.firstIndex ~= cjson.null then
    firstIndex = tostring(decoded.firstIndex)
  end
end
local lastIndex = tostring(tonumber(indexJson) - 1)
redis.call(
  'SET',
  KEYS[5],
  '{"firstIndex":' .. firstIndex .. ',"lastIndex":' .. lastIndex .. '}'
)
return committed
"#;

const DELETE_TASK_STORAGE_LUA: &str = r#"
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'string')
       or not validType(KEYS[4], 'string')
       or not validType(KEYS[5], 'list')
       or not validType(KEYS[6], 'string')
       or not validType(KEYS[7], 'hash')
       or not validType(KEYS[8], 'hash')
       or not validType(KEYS[9], 'set')
       or not validType(KEYS[10], 'set')
       or not validType(KEYS[11], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    for index = 12, #KEYS do
      if not validType(KEYS[index], 'string') then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    end

    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.acceptingWrites ~= false
       or fence.storageEpoch ~= tonumber(ARGV[5])
       or fence.activeReleaseGeneration ~= ARGV[3] then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    for index = 12, #KEYS do
      redis.call('UNLINK', KEYS[index])
    end
    redis.call(
      'UNLINK',
      KEYS[2],
      KEYS[3],
      KEYS[4],
      KEYS[5],
      KEYS[6],
      KEYS[7],
      KEYS[8],
      KEYS[9],
      KEYS[11]
    )
    redis.call('SREM', KEYS[10], ARGV[1])
    return 1
"#;

const RESTORE_HOT_TASK_LUA: &str = r#"
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'string')
       or not validType(KEYS[4], 'list')
       or not validType(KEYS[5], 'string')
       or not validType(KEYS[6], 'hash')
       or not validType(KEYS[7], 'hash')
       or not validType(KEYS[8], 'set')
       or not validType(KEYS[9], 'string')
       or not validType(KEYS[10], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local leaseJson = redis.call('GET', KEYS[1])
    if not leaseJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or tonumber(ARGV[6]) <= tonumber(ARGV[5]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local existingFenceJson = redis.call('GET', KEYS[2])
    if existingFenceJson then
      local existingFence = cjson.decode(existingFenceJson)
      if existingFence.acceptingWrites == true
         and existingFence.storageEpoch == tonumber(ARGV[6])
         and redis.call('EXISTS', KEYS[3]) == 1 then
        redis.call('SET', KEYS[10], ARGV[13])
        return 2
      end
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local replay = cjson.decode(ARGV[8])
    local series = cjson.decode(ARGV[9])
    if type(replay) ~= 'table' or type(series) ~= 'table' then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    for _, eventJson in ipairs(replay) do
      if type(eventJson) ~= 'string' then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    end
    for _, entry in ipairs(series) do
      if type(entry) ~= 'table'
         or type(entry.seriesId) ~= 'string'
         or type(entry.eventJson) ~= 'string'
         or type(entry.listEventJson) ~= 'string' then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    end
    redis.call('DEL', KEYS[4], KEYS[6], KEYS[7])
    for _, eventJson in ipairs(replay) do
      redis.call('RPUSH', KEYS[4], eventJson)
    end
    for _, entry in ipairs(series) do
      redis.call('HSET', KEYS[6], entry.seriesId, entry.eventJson)
      if entry.listEventJson ~= '' then
        redis.call('HSET', KEYS[7], entry.seriesId, entry.listEventJson)
      end
    end

    redis.call('SET', KEYS[3], ARGV[7])
    redis.call('SADD', KEYS[8], ARGV[1])
    redis.call('SET', KEYS[5], ARGV[10])
    redis.call('SET', KEYS[9], ARGV[11])
    redis.call('SET', KEYS[2], ARGV[12])
    redis.call('SET', KEYS[10], ARGV[13])
    return 1
"#;

const PROJECT_TERMINAL_FENCED_LUA: &str = r#"
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'string')
       or not validType(KEYS[4], 'string')
       or not validType(KEYS[5], 'list')
       or not validType(KEYS[6], 'string')
       or not validType(KEYS[7], 'string')
       or not validType(KEYS[8], 'string')
       or not validType(KEYS[9], 'set')
       or not validType(KEYS[10], 'string')
       or not validType(KEYS[11], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.taskId ~= ARGV[1]
       or fence.acceptingWrites ~= false
       or fence.storageEpoch ~= tonumber(ARGV[5])
       or fence.activeReleaseGeneration ~= ARGV[3]
       or tonumber(ARGV[6]) ~= tonumber(ARGV[5]) + 1 then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local eventIndex = tonumber(ARGV[9])
    local nextIndex = tonumber(redis.call('GET', KEYS[6]) or '0')
    if not eventIndex or eventIndex < 0 or not nextIndex then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    local projected = 0
    if nextIndex == eventIndex then
      redis.call('RPUSH', KEYS[5], ARGV[8])
      redis.call('SET', KEYS[6], tostring(eventIndex + 1))
      projected = 1
    elseif nextIndex > eventIndex then
      local found = false
      for _, candidateJson in ipairs(redis.call('LRANGE', KEYS[5], 0, -1)) do
        local candidate = cjson.decode(candidateJson)
        if candidate.index == eventIndex then
          if candidateJson ~= ARGV[8] then
            return redis.error_reply('STORAGE_INTEGRITY_ERROR')
          end
          found = true
          break
        end
      end
      if not found then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    else
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    redis.call('SET', KEYS[3], ARGV[7])
    redis.call('SET', KEYS[4], 'timeout')
    local windowJson = redis.call('GET', KEYS[7])
    local firstIndex = eventIndex
    if windowJson then
      local window = cjson.decode(windowJson)
      if window.firstIndex ~= cjson.null then firstIndex = window.firstIndex end
    end
    redis.call(
      'SET',
      KEYS[7],
      cjson.encode({ firstIndex = firstIndex, lastIndex = eventIndex })
    )

    if ARGV[10] ~= '' and redis.call('EXISTS', KEYS[11]) == 0 then
      local assignmentJson = redis.call('GET', KEYS[8])
      if assignmentJson and assignmentJson ~= ARGV[10] then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
      if assignmentJson then
        redis.call('DEL', KEYS[8])
        redis.call('SREM', KEYS[9], ARGV[1])
        local workerJson = redis.call('GET', KEYS[10])
        if workerJson then
          local worker = cjson.decode(workerJson)
          worker.usedSlots = math.max(0, worker.usedSlots - tonumber(ARGV[12]))
          if worker.status ~= 'offline' and worker.status ~= 'draining' then
            if worker.usedSlots >= worker.capacity then
              worker.status = 'busy'
            else
              worker.status = 'idle'
            end
          end
          redis.call('SET', KEYS[10], cjson.encode(worker))
        end
      end
      redis.call('SET', KEYS[11], '1', 'PX', tonumber(ARGV[14]))
    end

    redis.call('SET', KEYS[2], ARGV[13])
    return { tostring(projected), ARGV[6] }
"#;

const REGISTER_STORAGE_WRITER_LUA: &str = r#"
    redis.call('SET', KEYS[1], ARGV[2], 'PX', tonumber(ARGV[3]))
    redis.call('SADD', KEYS[2], ARGV[1])
    return 1
"#;

const SET_SERIES_LATEST_LUA: &str = r#"
    redis.call('HSET', KEYS[1], ARGV[2], ARGV[1])
    redis.call('HDEL', KEYS[2], ARGV[2])
    if ARGV[4] == '1' then
      redis.call('SET', KEYS[3], ARGV[1])
      redis.call('SADD', KEYS[4], ARGV[2])
    elseif ARGV[3] == '1' then
      redis.call('DEL', KEYS[3])
      redis.call('SREM', KEYS[4], ARGV[2])
    end
    return 1
"#;

const ACCUMULATE_LUA: &str = r#"
    local currentHash = redis.call('HGET', KEYS[1], ARGV[1])
    local currentLegacy = redis.call('GET', KEYS[3])
    if (currentHash or '') ~= ARGV[4]
       or (currentLegacy or '') ~= ARGV[5] then
      return 'RETRY'
    end
    redis.call('HSET', KEYS[1], ARGV[1], ARGV[2])
    redis.call('HDEL', KEYS[2], ARGV[1])
    if ARGV[6] == '1' then
      redis.call('SET', KEYS[3], ARGV[2])
      redis.call('SADD', KEYS[4], ARGV[1])
    elseif ARGV[3] == '1' then
      redis.call('DEL', KEYS[3])
      redis.call('SREM', KEYS[4], ARGV[1])
    end
    return 'COMMITTED'
"#;

#[async_trait]
impl ShortTermStore for RedisShortTermStore {
    fn supports_hot_cold_release(&self) -> bool {
        true
    }

    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let task_id = task.id.clone();
        let json = serde_json::to_string(&task)?;
        let fence = serde_json::to_string(&TaskWriteFence {
            task_id: task_id.clone(),
            accepting_writes: true,
            storage_epoch: 1,
            active_release_generation: None,
        })?;
        let mut conn = self.conn.clone();
        redis_call!(
            conn,
            redis::Script::new(SAVE_TASK_LUA)
                .key(self.keys.task(&task_id))
                .key(self.keys.tasks_set())
                .key(self.keys.write_fence(&task_id))
                .key(self.keys.task_status(&task_id))
                .arg(json)
                .arg(&task_id)
                .arg(fence)
                .arg(
                    serde_json::to_value(&task.status)?
                        .as_str()
                        .unwrap_or("pending"),
                )
                .invoke_async::<()>(&mut conn)
        );
        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.task(task_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = redis_call!(conn, conn.get(&key));
        match result {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    async fn get_task_mutation_snapshot(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskMutationSnapshot>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.task(task_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = redis_call!(conn, conn.get(&key));
        match result {
            Some(json) => Ok(Some(TaskMutationSnapshot {
                task: serde_json::from_str(&json)?,
                revision: json,
            })),
            None => Ok(None),
        }
    }

    async fn acquire_storage_lock(
        &self,
        task_id: &str,
        lock_token: &str,
        generation: &str,
        ttl_ms: u64,
    ) -> Result<Option<StorageLease>, Box<dyn std::error::Error + Send + Sync>> {
        if ttl_ms == 0 {
            return Err(Box::new(StorageIntegrityError::new(
                "Storage lock TTL must be positive",
            )));
        }
        let mut conn = self.conn.clone();
        let raw: Option<String> = redis_call!(
            conn,
            redis::Script::new(ACQUIRE_STORAGE_LOCK_LUA)
                .key(self.keys.storage_lock(task_id))
                .key(self.keys.write_fence(task_id))
                .arg(task_id)
                .arg(lock_token)
                .arg(generation)
                .arg(ttl_ms)
                .invoke_async(&mut conn)
        );
        raw.map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(Into::into)
    }

    async fn renew_storage_lock(
        &self,
        lease: &StorageLease,
        ttl_ms: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if ttl_ms == 0 {
            return Ok(false);
        }
        let mut conn = self.conn.clone();
        let renewed: i32 = redis_call!(
            conn,
            redis::Script::new(RENEW_STORAGE_LOCK_LUA)
                .key(self.keys.storage_lock(&lease.task_id))
                .arg(&lease.task_id)
                .arg(&lease.lock_token)
                .arg(&lease.generation)
                .arg(lease.storage_epoch)
                .arg(ttl_ms)
                .invoke_async(&mut conn)
        );
        Ok(renewed == 1)
    }

    async fn release_storage_lock(
        &self,
        lease: &StorageLease,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let released: i32 = redis_call!(
            conn,
            redis::Script::new(RELEASE_STORAGE_LOCK_LUA)
                .key(self.keys.storage_lock(&lease.task_id))
                .arg(&lease.task_id)
                .arg(&lease.lock_token)
                .arg(&lease.generation)
                .arg(lease.storage_epoch)
                .invoke_async(&mut conn)
        );
        Ok(released == 1)
    }

    async fn get_write_fence(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskWriteFence>, Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let raw: Option<String> = redis_call!(conn, conn.get(self.keys.write_fence(task_id)));
        raw.map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(Into::into)
    }

    async fn close_write_fence(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<ClosedWriteFence, Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let result = redis::Script::new(CLOSE_WRITE_FENCE_LUA)
            .key(self.keys.storage_lock(&lease.task_id))
            .key(self.keys.write_fence(&lease.task_id))
            .key(self.keys.idx(&lease.task_id))
            .arg(&lease.task_id)
            .arg(&lease.lock_token)
            .arg(&lease.generation)
            .arg(lease.storage_epoch)
            .arg(expected_epoch)
            .invoke_async(&mut conn)
            .await;
        let (fence_json, high_watermark_json): (String, String) =
            Self::observe_fence_result(&conn, result)?;
        let fence: TaskWriteFence = serde_json::from_str(&fence_json)?;
        let high_watermark = high_watermark_json.parse::<i64>().map_err(|_| {
            Box::new(StorageIntegrityError::new(
                "Redis returned an invalid high watermark",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        if !(-1..=Self::MAX_SAFE_INTEGER).contains(&high_watermark) {
            return Err(Box::new(StorageIntegrityError::new(
                "Redis returned an invalid high watermark",
            )));
        }
        Ok(ClosedWriteFence {
            task_id: fence.task_id,
            accepting_writes: fence.accepting_writes,
            storage_epoch: fence.storage_epoch,
            active_release_generation: fence.active_release_generation,
            high_watermark,
        })
    }

    async fn reopen_write_fence(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<HotWriteToken, Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let result = redis::Script::new(REOPEN_WRITE_FENCE_LUA)
            .key(self.keys.storage_lock(&lease.task_id))
            .key(self.keys.write_fence(&lease.task_id))
            .arg(&lease.task_id)
            .arg(&lease.lock_token)
            .arg(&lease.generation)
            .arg(lease.storage_epoch)
            .arg(expected_epoch)
            .invoke_async(&mut conn)
            .await;
        let raw: String = Self::observe_fence_result(&conn, result)?;
        Ok(serde_json::from_str(&raw)?)
    }

    async fn commit_event_fenced(
        &self,
        task_id: &str,
        event: TaskEvent,
        token: &HotWriteToken,
    ) -> Result<SeriesResult, Box<dyn std::error::Error + Send + Sync>> {
        if token.task_id != task_id || event.task_id != task_id {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        let series_id = event.series_id.as_deref().unwrap_or("");
        let series_mode = match event.series_mode.as_ref() {
            Some(SeriesMode::Latest) => "latest",
            Some(SeriesMode::Accumulate) => "accumulate",
            _ => "",
        };
        let field = event.series_acc_field.as_deref().unwrap_or("delta");
        let (event_prefix, event_suffix) = Self::make_indexed_event_template(&event)?;

        loop {
            let mut conn = self.conn.clone();
            let (hash_state_raw, legacy_candidate_raw, first_raw, second_raw): (
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            ) = redis_call!(
                conn,
                redis::pipe()
                    .cmd("HGET")
                    .arg(self.keys.series_state(task_id))
                    .arg(series_id)
                    .cmd("GET")
                    .arg(self.keys.legacy_series_latest(task_id, series_id))
                    .cmd("LINDEX")
                    .arg(self.keys.events(task_id))
                    .arg(0)
                    .cmd("LINDEX")
                    .arg(self.keys.events(task_id))
                    .arg(1)
                    .query_async(&mut conn)
            );

            let hash_state_raw = hash_state_raw.unwrap_or_default();
            let legacy_candidate_raw = legacy_candidate_raw.unwrap_or_default();
            let (state_raw, previous, legacy_state_raw) = Self::select_series_state(
                &hash_state_raw,
                &legacy_candidate_raw,
                task_id,
                series_id,
            )?;
            if self.legacy_series_writes
                && !legacy_candidate_raw.is_empty()
                && legacy_state_raw.is_empty()
            {
                return Err(Box::new(StorageIntegrityError::new(
                    "Legacy series key collides with another task or series",
                )));
            }
            let first = first_raw
                .as_deref()
                .map(|raw| Self::parse_event_list_head(raw, task_id))
                .transpose()?;
            let second = second_raw
                .as_deref()
                .map(|raw| Self::parse_event_list_head(raw, task_id))
                .transpose()?;
            let accumulated = if series_mode == "accumulate" {
                Self::accumulate_event(previous.as_ref(), &event, field)
            } else {
                event.clone()
            };
            let (accumulated_prefix, accumulated_suffix) =
                Self::make_indexed_event_template(&accumulated)?;
            let result = redis::Script::new(COMMIT_EVENT_FENCED_LUA)
                .key(self.keys.write_fence(task_id))
                .key(self.keys.idx(task_id))
                .key(self.keys.events(task_id))
                .key(self.keys.series_state(task_id))
                .key(self.keys.series_list_entries(task_id))
                .key(self.keys.legacy_series_latest(task_id, series_id))
                .key(self.keys.legacy_series_ids(task_id))
                .key(self.keys.hot_window(task_id))
                .arg(token.storage_epoch)
                .arg(&event_prefix)
                .arg(&event_suffix)
                .arg(series_id)
                .arg(series_mode)
                .arg(&state_raw)
                .arg(&accumulated_prefix)
                .arg(&accumulated_suffix)
                .arg(first_raw.as_deref().unwrap_or(""))
                .arg(second_raw.as_deref().unwrap_or(""))
                .arg(
                    first
                        .as_ref()
                        .map(|value| value.index.to_string())
                        .unwrap_or_default(),
                )
                .arg(
                    second
                        .as_ref()
                        .map(|value| value.index.to_string())
                        .unwrap_or_default(),
                )
                .arg(
                    (previous
                        .as_ref()
                        .and_then(|value| value.series_mode.as_ref())
                        == Some(&SeriesMode::Latest)) as u8,
                )
                .arg((!legacy_state_raw.is_empty()) as u8)
                .arg(&hash_state_raw)
                .arg(&legacy_candidate_raw)
                .arg(self.legacy_series_writes as u8)
                .invoke_async(&mut conn)
                .await;
            let (status, index_raw): (String, String) = Self::observe_fence_result(&conn, result)?;
            if status == "RETRY" {
                continue;
            }
            if status != "COMMITTED" {
                return Err(Box::new(StorageIntegrityError::new(
                    "Redis returned an invalid fenced commit result",
                )));
            }
            let index = index_raw.parse::<u64>().map_err(|_| {
                Box::new(StorageIntegrityError::new(
                    "Redis returned an invalid event index",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
            if index > Self::MAX_SAFE_INTEGER as u64 {
                return Err(Box::new(StorageIntegrityError::new(
                    "Redis returned an invalid event index",
                )));
            }
            let mut committed = event.clone();
            committed.index = index;
            let accumulated_event = (series_mode == "accumulate").then(|| {
                let mut value = accumulated;
                value.index = index;
                value
            });
            return Ok(SeriesResult {
                event: committed,
                accumulated_event,
                stored: true,
            });
        }
    }

    async fn save_task_fenced(
        &self,
        task: Task,
        token: &HotWriteToken,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if token.task_id != task.id {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        let mut conn = self.conn.clone();
        let result = redis::Script::new(SAVE_TASK_FENCED_LUA)
            .key(self.keys.write_fence(&task.id))
            .key(self.keys.task(&task.id))
            .key(self.keys.task_status(&task.id))
            .arg(token.storage_epoch)
            .arg(serde_json::to_string(&task)?)
            .arg(
                serde_json::to_value(&task.status)?
                    .as_str()
                    .unwrap_or("pending"),
            )
            .invoke_async::<()>(&mut conn)
            .await;
        Self::observe_fence_result(&conn, result)?;
        Ok(())
    }

    async fn commit_task_events_fenced(
        &self,
        task: Task,
        expected_revision: &str,
        events: Vec<TaskEvent>,
        token: &HotWriteToken,
    ) -> Result<Option<Vec<TaskEvent>>, Box<dyn std::error::Error + Send + Sync>> {
        if token.task_id != task.id
            || events.is_empty()
            || events.iter().any(|event| {
                event.task_id != task.id || event.series_id.is_some() || event.series_mode.is_some()
            })
        {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        let templates = events
            .iter()
            .map(Self::make_indexed_event_template)
            .collect::<Result<Vec<_>, _>>()?;
        let script = redis::Script::new(COMMIT_TASK_EVENTS_FENCED_LUA);
        let mut invocation = script.prepare_invoke();
        invocation
            .key(self.keys.write_fence(&task.id))
            .key(self.keys.task(&task.id))
            .key(self.keys.idx(&task.id))
            .key(self.keys.events(&task.id))
            .key(self.keys.hot_window(&task.id))
            .key(self.keys.task_status(&task.id))
            .arg(token.storage_epoch)
            .arg(serde_json::to_string(&task)?)
            .arg(expected_revision)
            .arg(
                serde_json::to_value(&task.status)?
                    .as_str()
                    .unwrap_or("pending"),
            )
            .arg(events.len());
        for (prefix, suffix) in &templates {
            invocation.arg(prefix).arg(suffix);
        }
        let mut conn = self.conn.clone();
        let result = invocation.invoke_async::<Vec<String>>(&mut conn).await;
        let raw = Self::observe_fence_result(&conn, result)?;
        if raw.first().map(String::as_str) == Some("TASK_CONFLICT") {
            return Ok(None);
        }
        if raw.first().map(String::as_str) != Some("COMMITTED") || raw.len() != events.len() + 2 {
            return Err(Box::new(StorageIntegrityError::new(
                "Redis returned an invalid fenced task-event commit result",
            )));
        }
        let committed = raw
            .into_iter()
            .skip(2)
            .map(|event| serde_json::from_str(&event).map_err(Into::into))
            .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync>>>()?;
        Ok(Some(committed))
    }

    async fn read_archive_source_page(
        &self,
        task_id: &str,
        watermark: i64,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<ArchiveSourcePage, Box<dyn std::error::Error + Send + Sync>> {
        if limit == 0 || !(-1..=Self::MAX_SAFE_INTEGER).contains(&watermark) {
            return Err(Box::new(StorageIntegrityError::new(
                "Invalid archive source bounds",
            )));
        }
        let (offset, mut last_index) = Self::decode_archive_cursor(cursor, watermark)?;
        let bounded_limit = limit.min(isize::MAX as u64) as usize;
        let end = offset
            .checked_add(bounded_limit - 1)
            .filter(|end| *end <= isize::MAX as usize)
            .ok_or_else(|| {
                Box::new(StorageIntegrityError::new(
                    "Archive source cursor exceeds safe bounds",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
        if offset > isize::MAX as usize {
            return Err(Box::new(StorageIntegrityError::new(
                "Archive source cursor exceeds safe bounds",
            )));
        }
        let mut conn = self.conn.clone();
        let raw: Vec<String> = redis_call!(
            conn,
            conn.lrange(self.keys.events(task_id), offset as isize, end as isize)
        );
        let length: usize = redis_call!(conn, conn.llen(self.keys.events(task_id)));
        let mut events = Vec::new();
        let mut beyond_watermark = false;
        for encoded in &raw {
            let event: TaskEvent = serde_json::from_str(encoded).map_err(|_| {
                Box::new(StorageIntegrityError::new(
                    "Archive source contains invalid event JSON",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
            let event_index = i64::try_from(event.index).map_err(|_| {
                Box::new(StorageIntegrityError::new(
                    "Archive source event index exceeds safe bounds",
                )) as Box<dyn std::error::Error + Send + Sync>
            })?;
            if event.task_id != task_id || event_index <= last_index {
                return Err(Box::new(StorageIntegrityError::new(
                    "Archive source indexes are not strictly increasing",
                )));
            }
            if event_index > watermark {
                beyond_watermark = true;
                break;
            }
            last_index = event_index;
            events.push(event);
        }
        let next_offset = offset + raw.len();
        let done = beyond_watermark || next_offset >= length;
        Ok(ArchiveSourcePage {
            task_id: task_id.to_string(),
            watermark,
            cursor: cursor.map(str::to_string),
            next_cursor: (!done)
                .then(|| Self::encode_archive_cursor(watermark, next_offset, last_index)),
            events,
            done,
        })
    }

    async fn delete_task_storage_fenced(
        &self,
        lease: &StorageLease,
        expected_epoch: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let series_keys = self.scan_series_keys(&lease.task_id).await?;
        let mut conn = self.conn.clone();
        let script = redis::Script::new(DELETE_TASK_STORAGE_LUA);
        let mut invocation = script.prepare_invoke();
        invocation
            .key(self.keys.storage_lock(&lease.task_id))
            .key(self.keys.write_fence(&lease.task_id))
            .key(self.keys.task(&lease.task_id))
            .key(self.keys.task_status(&lease.task_id))
            .key(self.keys.events(&lease.task_id))
            .key(self.keys.idx(&lease.task_id))
            .key(self.keys.series_state(&lease.task_id))
            .key(self.keys.series_list_entries(&lease.task_id))
            .key(self.keys.legacy_series_ids(&lease.task_id))
            .key(self.keys.tasks_set())
            .key(self.keys.hot_window(&lease.task_id));
        for key in series_keys {
            invocation.key(key);
        }
        let result = invocation
            .arg(&lease.task_id)
            .arg(&lease.lock_token)
            .arg(&lease.generation)
            .arg(lease.storage_epoch)
            .arg(expected_epoch)
            .invoke_async::<()>(&mut conn)
            .await;
        Self::observe_fence_result(&conn, result)?;
        Ok(())
    }

    async fn restore_hot_task_fenced(
        &self,
        snapshot: RehydrateSnapshot,
        lease: &StorageLease,
        next_epoch: u64,
    ) -> Result<HotWriteToken, Box<dyn std::error::Error + Send + Sync>> {
        Self::validate_rehydrate_snapshot(&snapshot, lease, next_epoch)?;
        let task_id = snapshot.task.id.clone();
        let hot_window = serde_json::json!({
            "firstIndex": snapshot.replay_events.first().map(|event| event.index),
            "lastIndex": snapshot.replay_events.last().map(|event| event.index),
        });
        let fence = TaskWriteFence {
            task_id: task_id.clone(),
            accepting_writes: true,
            storage_epoch: next_epoch,
            active_release_generation: None,
        };
        let replay_event_json = snapshot
            .replay_events
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;
        let series_event_json = snapshot
            .series_latest
            .iter()
            .map(|entry| {
                let event_json = serde_json::to_string(&entry.event)?;
                let list_event_json = if entry.mode == SeriesMode::Latest
                    && snapshot.replay_events.iter().any(|event| {
                        event.index == entry.event.index
                            && event.id == entry.event.id
                            && serde_json::to_string(event).ok().as_deref()
                                == Some(event_json.as_str())
                    }) {
                    event_json.clone()
                } else {
                    String::new()
                };
                Ok(serde_json::json!({
                    "seriesId": entry.series_id,
                    "eventJson": event_json,
                    "listEventJson": list_event_json,
                }))
            })
            .collect::<Result<Vec<serde_json::Value>, serde_json::Error>>()?;
        let mut conn = self.conn.clone();
        let result = redis::Script::new(RESTORE_HOT_TASK_LUA)
            .key(self.keys.storage_lock(&task_id))
            .key(self.keys.write_fence(&task_id))
            .key(self.keys.task(&task_id))
            .key(self.keys.events(&task_id))
            .key(self.keys.idx(&task_id))
            .key(self.keys.series_state(&task_id))
            .key(self.keys.series_list_entries(&task_id))
            .key(self.keys.tasks_set())
            .key(self.keys.hot_window(&task_id))
            .key(self.keys.task_status(&task_id))
            .arg(&task_id)
            .arg(&lease.lock_token)
            .arg(&lease.generation)
            .arg(lease.storage_epoch)
            .arg(snapshot.storage_epoch)
            .arg(next_epoch)
            .arg(serde_json::to_string(&snapshot.task)?)
            .arg(serde_json::to_string(&replay_event_json)?)
            .arg(serde_json::to_string(&series_event_json)?)
            .arg(snapshot.max_event_index + 1)
            .arg(serde_json::to_string(&hot_window)?)
            .arg(serde_json::to_string(&fence)?)
            .arg(
                serde_json::to_value(&snapshot.task.status)?
                    .as_str()
                    .unwrap_or("pending"),
            )
            .invoke_async::<()>(&mut conn)
            .await;
        Self::observe_fence_result(&conn, result)?;
        Ok(HotWriteToken {
            task_id,
            storage_epoch: next_epoch,
        })
    }

    async fn project_terminal_fenced(
        &self,
        projection: &TerminalProjection,
        lease: &StorageLease,
        expected_epoch: u64,
        next_epoch: u64,
    ) -> Result<TerminalProjectionResult, Box<dyn std::error::Error + Send + Sync>> {
        let task_id = &projection.task.id;
        if task_id != &lease.task_id
            || projection.task.status != taskcast_core::types::TaskStatus::Timeout
            || projection.event.task_id != *task_id
            || projection
                .assignment
                .as_ref()
                .is_some_and(|assignment| assignment.task_id != *task_id)
            || next_epoch != expected_epoch + 1
        {
            return Err(Box::new(StorageFenceConflictError::default()));
        }
        let assignment = projection.assignment.as_ref();
        let assignment_json = assignment
            .map(serde_json::to_string)
            .transpose()?
            .unwrap_or_default();
        let worker_id = assignment
            .map(|assignment| assignment.worker_id.as_str())
            .unwrap_or("");
        let cost = assignment.map(|assignment| assignment.cost).unwrap_or(0);
        let fence = TaskWriteFence {
            task_id: task_id.clone(),
            accepting_writes: true,
            storage_epoch: next_epoch,
            active_release_generation: None,
        };
        let mut conn = self.conn.clone();
        let result = redis::Script::new(PROJECT_TERMINAL_FENCED_LUA)
            .key(self.keys.storage_lock(task_id))
            .key(self.keys.write_fence(task_id))
            .key(self.keys.task(task_id))
            .key(self.keys.task_status(task_id))
            .key(self.keys.events(task_id))
            .key(self.keys.idx(task_id))
            .key(self.keys.hot_window(task_id))
            .key(self.keys.assignment(task_id))
            .key(self.keys.worker_assignments(worker_id))
            .key(self.keys.worker(worker_id))
            .key(self.keys.terminal_projection(&projection.projection_id))
            .arg(task_id)
            .arg(&lease.lock_token)
            .arg(&lease.generation)
            .arg(lease.storage_epoch)
            .arg(expected_epoch)
            .arg(next_epoch)
            .arg(serde_json::to_string(&projection.task)?)
            .arg(serde_json::to_string(&projection.event)?)
            .arg(projection.event.index)
            .arg(assignment_json)
            .arg(worker_id)
            .arg(cost)
            .arg(serde_json::to_string(&fence)?)
            .arg(7_u64 * 24 * 60 * 60 * 1_000)
            .invoke_async(&mut conn)
            .await;
        let result: (u64, u64) = Self::observe_fence_result(&conn, result)?;
        if result.0 > 1 || result.1 != next_epoch {
            return Err(Box::new(StorageIntegrityError::new(
                "Redis returned an invalid terminal projection result",
            )));
        }
        Ok(TerminalProjectionResult {
            token: HotWriteToken {
                task_id: task_id.clone(),
                storage_epoch: next_epoch,
            },
            projected: result.0 == 1,
        })
    }

    async fn get_task_storage_presence(
        &self,
        task_id: &str,
    ) -> Result<TaskStoragePresence, Box<dyn std::error::Error + Send + Sync>> {
        let legacy_series_count = self.scan_series_keys(task_id).await?.len() as u64;
        let mut conn = self.conn.clone();
        let (task, event_count, next_index, series_state_count, write_fence): (
            u64,
            u64,
            u64,
            u64,
            u64,
        ) = redis_call!(
            conn,
            redis::pipe()
                .cmd("EXISTS")
                .arg(self.keys.task(task_id))
                .arg(self.keys.task_status(task_id))
                .cmd("LLEN")
                .arg(self.keys.events(task_id))
                .cmd("EXISTS")
                .arg(self.keys.idx(task_id))
                .cmd("HLEN")
                .arg(self.keys.series_state(task_id))
                .cmd("EXISTS")
                .arg(self.keys.write_fence(task_id))
                .query_async(&mut conn)
        );
        Ok(TaskStoragePresence {
            task: task > 0,
            event_count,
            next_index: next_index == 1,
            series_state_count: series_state_count + legacy_series_count,
            write_fence: write_fence == 1,
        })
    }

    async fn register_storage_writer(
        &self,
        mut registration: StorageWriterRegistration,
        ttl_ms: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if ttl_ms == 0 {
            return Err(Box::new(StorageIntegrityError::new(
                "Writer readiness TTL must be positive",
            )));
        }
        registration.expires_at = Self::now_ms()? + ttl_ms as f64;
        let mut conn = self.conn.clone();
        redis_call!(
            conn,
            redis::Script::new(REGISTER_STORAGE_WRITER_LUA)
                .key(self.keys.storage_writer(&registration.instance_id))
                .key(self.keys.storage_writers())
                .arg(&registration.instance_id)
                .arg(serde_json::to_string(&registration)?)
                .arg(ttl_ms)
                .invoke_async::<()>(&mut conn)
        );
        Ok(())
    }

    async fn list_storage_writers(
        &self,
    ) -> Result<Vec<StorageWriterRegistration>, Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let instance_ids: Vec<String> =
            redis_call!(conn, conn.smembers(self.keys.storage_writers()));
        if instance_ids.is_empty() {
            return Ok(Vec::new());
        }
        let writer_keys = instance_ids
            .iter()
            .map(|instance_id| self.keys.storage_writer(instance_id))
            .collect::<Vec<_>>();
        let values: Vec<Option<String>> = redis_call!(conn, conn.mget(writer_keys));
        let mut stale = Vec::new();
        let mut registrations = Vec::new();
        for (instance_id, raw) in instance_ids.iter().zip(values) {
            if let Some(raw) = raw {
                registrations.push(serde_json::from_str(&raw)?);
            } else {
                stale.push(instance_id.as_str());
            }
        }
        if !stale.is_empty() {
            redis_call!(
                conn,
                conn.srem::<_, _, ()>(self.keys.storage_writers(), stale)
            );
        }
        Ok(registrations)
    }

    async fn append_event(
        &self,
        task_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.events(task_id);
        let json = serde_json::to_string(&event)?;
        let mut conn = self.conn.clone();
        redis_call!(conn, conn.rpush::<_, _, ()>(&key, &json));
        Ok(())
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.events(task_id);
        let mut conn = self.conn.clone();
        let raw: Vec<String> = redis_call!(conn, conn.lrange(&key, 0, -1));

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
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.task(task_id), ttl_secs)
        );
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.task_status(task_id), ttl_secs)
        );

        // Expire events list
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.events(task_id), ttl_secs)
        );

        // Expire index counter
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.idx(task_id), ttl_secs)
        );
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.write_fence(task_id), ttl_secs)
        );
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.hot_window(task_id), ttl_secs)
        );
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.series_state(task_id), ttl_secs)
        );
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.series_list_entries(task_id), ttl_secs)
        );
        let legacy_series_keys = self.scan_series_keys(task_id).await?;
        for key in legacy_series_keys {
            redis_call!(conn, conn.expire::<_, ()>(key, ttl_secs));
        }
        redis_call!(
            conn,
            conn.expire::<_, ()>(&self.keys.legacy_series_ids(task_id), ttl_secs)
        );

        Ok(())
    }

    async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let mut conn = self.conn.clone();
        let (hash_state_raw, legacy_candidate_raw): (Option<String>, Option<String>) = redis_call!(
            conn,
            redis::pipe()
                .cmd("HGET")
                .arg(self.keys.series_state(task_id))
                .arg(series_id)
                .cmd("GET")
                .arg(self.keys.legacy_series_latest(task_id, series_id))
                .query_async(&mut conn)
        );
        let (_, selected, _) = Self::select_series_state(
            hash_state_raw.as_deref().unwrap_or(""),
            legacy_candidate_raw.as_deref().unwrap_or(""),
            task_id,
            series_id,
        )?;
        Ok(selected)
    }

    async fn set_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let json = serde_json::to_string(&event)?;
        let mut conn = self.conn.clone();
        let legacy_candidate_raw: Option<String> = redis_call!(
            conn,
            conn.get(self.keys.legacy_series_latest(task_id, series_id))
        );
        let legacy_candidate_raw = legacy_candidate_raw.unwrap_or_default();
        let (_, _, legacy_state_raw) =
            Self::select_series_state("", &legacy_candidate_raw, task_id, series_id)?;
        if self.legacy_series_writes
            && !legacy_candidate_raw.is_empty()
            && legacy_state_raw.is_empty()
        {
            return Err(Box::new(StorageIntegrityError::new(
                "Legacy series key collides with another task or series",
            )));
        }
        redis_call!(
            conn,
            redis::Script::new(SET_SERIES_LATEST_LUA)
                .key(self.keys.series_state(task_id))
                .key(self.keys.series_list_entries(task_id))
                .key(self.keys.legacy_series_latest(task_id, series_id))
                .key(self.keys.legacy_series_ids(task_id))
                .arg(json)
                .arg(series_id)
                .arg((!legacy_state_raw.is_empty()) as u8)
                .arg(self.legacy_series_writes as u8)
                .invoke_async::<()>(&mut conn)
        );
        Ok(())
    }

    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let mut conn = self.conn.clone();
            let (hash_raw, legacy_candidate): (Option<String>, Option<String>) = redis_call!(
                conn,
                redis::pipe()
                    .cmd("HGET")
                    .arg(self.keys.series_state(task_id))
                    .arg(series_id)
                    .cmd("GET")
                    .arg(self.keys.legacy_series_latest(task_id, series_id))
                    .query_async(&mut conn)
            );
            let hash_raw = hash_raw.unwrap_or_default();
            let legacy_candidate = legacy_candidate.unwrap_or_default();
            let (_, previous, legacy_raw) =
                Self::select_series_state(&hash_raw, &legacy_candidate, task_id, series_id)?;
            if self.legacy_series_writes && !legacy_candidate.is_empty() && legacy_raw.is_empty() {
                return Err(Box::new(StorageIntegrityError::new(
                    "Legacy series key collides with another task or series",
                )));
            }
            let accumulated = Self::accumulate_event(previous.as_ref(), &event, field);
            let result: String = redis_call!(
                conn,
                redis::Script::new(ACCUMULATE_LUA)
                    .key(self.keys.series_state(task_id))
                    .key(self.keys.series_list_entries(task_id))
                    .key(self.keys.legacy_series_latest(task_id, series_id))
                    .key(self.keys.legacy_series_ids(task_id))
                    .arg(series_id)
                    .arg(serde_json::to_string(&accumulated)?)
                    .arg((!legacy_raw.is_empty()) as u8)
                    .arg(&hash_raw)
                    .arg(&legacy_candidate)
                    .arg(self.legacy_series_writes as u8)
                    .invoke_async(&mut conn)
            );
            if result == "RETRY" {
                continue;
            }
            return Ok(accumulated);
        }
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let events_key = self.keys.events(task_id);
        let mut conn = self.conn.clone();

        // Get the previous series latest
        let previous = self.get_series_latest(task_id, series_id).await?;

        if let Some(prev) = previous {
            // Find and replace the event in the list
            let raw: Vec<String> = redis_call!(conn, conn.lrange(&events_key, 0, -1));
            let new_event_json = serde_json::to_string(&event)?;

            // Search from the end (rposition equivalent)
            for (i, item) in raw.iter().enumerate().rev() {
                if let Ok(e) = serde_json::from_str::<TaskEvent>(item) {
                    if e.id == prev.id {
                        redis_call!(
                            conn,
                            conn.lset::<_, _, ()>(&events_key, i as isize, &new_event_json)
                        );
                        break;
                    }
                }
            }
        } else {
            // No previous -- just append
            self.append_event(task_id, event.clone()).await?;
        }

        // Update series latest
        self.set_series_latest(task_id, series_id, event.clone())
            .await?;
        redis_call!(
            conn,
            conn.hset::<_, _, _, ()>(
                self.keys.series_list_entries(task_id),
                series_id,
                serde_json::to_string(&event)?,
            )
        );

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
        let val: i64 = redis_call!(conn, conn.incr(&key, 1));
        Ok((val - 1) as u64)
    }

    // ─── Task query ──────────────────────────────────────────────────────

    async fn list_tasks(
        &self,
        filter: TaskFilter,
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let tasks_set_key = self.keys.tasks_set();
        let mut conn = self.conn.clone();

        let task_ids: Vec<String> = redis_call!(conn, conn.smembers(&tasks_set_key));
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build task keys for MGET
        let task_keys: Vec<String> = task_ids.iter().map(|id| self.keys.task(id)).collect();
        let raw: Vec<Option<String>> = redis_call!(conn, conn.mget(&task_keys));

        // Collect stale IDs (task expired but ID still in SET) for passive cleanup
        let stale_ids: Vec<&str> = raw
            .iter()
            .enumerate()
            .filter_map(|(i, opt)| {
                if opt.is_none() {
                    Some(task_ids[i].as_str())
                } else {
                    None
                }
            })
            .collect();
        if !stale_ids.is_empty() {
            redis_call!(conn, conn.srem::<_, _, ()>(&tasks_set_key, &stale_ids));
        }

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
        redis_call!(conn, conn.set::<_, _, ()>(&key, &json));
        redis_call!(conn, conn.sadd::<_, _, ()>(&workers_set_key, &worker.id));
        Ok(())
    }

    async fn get_worker(
        &self,
        worker_id: &str,
    ) -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.keys.worker(worker_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = redis_call!(conn, conn.get(&key));
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

        let worker_ids: Vec<String> = redis_call!(conn, conn.smembers(&workers_set_key));
        if worker_ids.is_empty() {
            return Ok(Vec::new());
        }

        let worker_keys: Vec<String> = worker_ids.iter().map(|id| self.keys.worker(id)).collect();
        let raw: Vec<Option<String>> = redis_call!(conn, conn.mget(&worker_keys));

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
        redis_call!(conn, conn.del::<_, ()>(&key));
        redis_call!(conn, conn.srem::<_, _, ()>(&workers_set_key, worker_id));
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
            redis.call('SET', KEYS[3], 'assigned')

            return 1
        "#;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs_f64();
        let timestamp_ms = (now * 1000.0) as u64;

        let script = redis::Script::new(lua);
        let mut conn = self.conn.clone();
        let result: i32 = redis_call!(
            conn,
            script
                .key(&task_key)
                .key(&worker_key)
                .key(self.keys.task_status(task_id))
                .arg(cost)
                .arg(worker_id)
                .arg(timestamp_ms)
                .invoke_async(&mut conn)
        );

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
        redis_call!(conn, conn.set::<_, _, ()>(&assignment_key, &json));
        redis_call!(
            conn,
            conn.sadd::<_, _, ()>(&worker_assignments_key, &assignment.task_id)
        );
        Ok(())
    }

    async fn remove_assignment(
        &self,
        task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let assignment_key = self.keys.assignment(task_id);
        let mut conn = self.conn.clone();

        // First, get the assignment to find the worker ID
        let result: Option<String> = redis_call!(conn, conn.get(&assignment_key));
        if let Some(json) = result {
            let assignment: WorkerAssignment = serde_json::from_str(&json)?;
            let worker_assignments_key = self.keys.worker_assignments(&assignment.worker_id);
            redis_call!(
                conn,
                conn.srem::<_, _, ()>(&worker_assignments_key, task_id)
            );
        }

        redis_call!(conn, conn.del::<_, ()>(&assignment_key));
        Ok(())
    }

    async fn get_worker_assignments(
        &self,
        worker_id: &str,
    ) -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>> {
        let worker_assignments_key = self.keys.worker_assignments(worker_id);
        let mut conn = self.conn.clone();

        let task_ids: Vec<String> = redis_call!(conn, conn.smembers(&worker_assignments_key));
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }

        let assignment_keys: Vec<String> =
            task_ids.iter().map(|id| self.keys.assignment(id)).collect();
        let raw: Vec<Option<String>> = redis_call!(conn, conn.mget(&assignment_keys));

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
        let result: Option<String> = redis_call!(conn, conn.get(&key));
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
        assert_eq!(keys.series_state("t1"), "taskcast:seriesState:t1");
        assert_eq!(
            keys.series_list_entries("t1"),
            "taskcast:seriesListEntries:t1"
        );
        assert_eq!(
            keys.legacy_series_latest("t1", "s1"),
            "taskcast:series:t1:s1"
        );
        assert_eq!(keys.legacy_series_ids("t1"), "taskcast:seriesIds:t1");
        assert_eq!(keys.write_fence("t1"), "taskcast:writeFence:t1");
        assert_eq!(keys.storage_lock("t1"), "taskcast:storageLock:t1");
        assert_eq!(keys.hot_window("t1"), "taskcast:hotWindow:t1");
        assert_eq!(keys.storage_writers(), "taskcast:storageWriters");
        assert_eq!(
            keys.storage_writer("writer-1"),
            "taskcast:storageWriter:writer-1"
        );
    }

    #[test]
    fn key_generation_custom_prefix() {
        let keys = Keys::new("myapp");
        assert_eq!(keys.task("task_123"), "myapp:task:task_123");
        assert_eq!(keys.events("task_123"), "myapp:events:task_123");
        assert_eq!(keys.idx("task_123"), "myapp:idx:task_123");
        assert_eq!(
            keys.legacy_series_latest("task_123", "progress"),
            "myapp:series:task_123:progress"
        );
        assert_eq!(
            keys.legacy_series_ids("task_123"),
            "myapp:seriesIds:task_123"
        );
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
            keys.legacy_series_latest("task-1", "series/2"),
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
