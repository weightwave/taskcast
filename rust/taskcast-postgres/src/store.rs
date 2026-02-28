use async_trait::async_trait;
use serde_json::Value as JsonValue;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

use taskcast_core::types::{
    CleanupConfig, EventQueryOptions, Level, LongTermStore, SeriesMode, Task, TaskAuthConfig,
    TaskError, TaskEvent, TaskStatus, WebhookConfig,
};

/// Table names derived from a configurable prefix.
#[derive(Debug, Clone)]
struct TableNames {
    tasks: String,
    events: String,
}

impl TableNames {
    fn new(prefix: &str) -> Self {
        Self {
            tasks: format!("{prefix}_tasks"),
            events: format!("{prefix}_events"),
        }
    }
}

/// PostgreSQL-backed long-term store for tasks and events.
///
/// Uses `sqlx::PgPool` for connection pooling and implements the
/// `LongTermStore` trait from `taskcast-core`.
pub struct PostgresLongTermStore {
    pool: PgPool,
    tables: TableNames,
}

impl PostgresLongTermStore {
    /// Create a new store with the given connection pool and optional table prefix.
    ///
    /// If `prefix` is `None`, falls back to the `TASKCAST_PG_PREFIX` env var,
    /// then to `"taskcast"`.
    pub fn new(pool: PgPool, prefix: Option<&str>) -> Self {
        let resolved = prefix
            .map(|s| s.to_string())
            .or_else(|| std::env::var("TASKCAST_PG_PREFIX").ok())
            .unwrap_or_else(|| "taskcast".to_string());
        Self {
            pool,
            tables: TableNames::new(&resolved),
        }
    }

    /// Run the initial migration to create tables and indexes.
    ///
    /// Uses the configurable table prefix to generate the correct table names.
    pub async fn migrate(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let tasks = &self.tables.tasks;
        let events = &self.tables.events;

        let migration = format!(
            r#"
            CREATE TABLE IF NOT EXISTS {tasks} (
              id TEXT PRIMARY KEY,
              type TEXT,
              status TEXT NOT NULL,
              params JSONB,
              result JSONB,
              error JSONB,
              metadata JSONB,
              auth_config JSONB,
              webhooks JSONB,
              cleanup JSONB,
              created_at BIGINT NOT NULL,
              updated_at BIGINT NOT NULL,
              completed_at BIGINT,
              ttl INTEGER
            );

            CREATE TABLE IF NOT EXISTS {events} (
              id TEXT PRIMARY KEY,
              task_id TEXT NOT NULL REFERENCES {tasks}(id) ON DELETE CASCADE,
              idx INTEGER NOT NULL,
              timestamp BIGINT NOT NULL,
              type TEXT NOT NULL,
              level TEXT NOT NULL,
              data JSONB,
              series_id TEXT,
              series_mode TEXT,
              UNIQUE(task_id, idx)
            );

            CREATE INDEX IF NOT EXISTS {events}_task_id_idx ON {events}(task_id, idx);
            CREATE INDEX IF NOT EXISTS {events}_task_id_timestamp ON {events}(task_id, timestamp);
            "#
        );

        sqlx::query(&migration).execute(&self.pool).await?;
        Ok(())
    }

    /// Convert a database row into a `Task`.
    fn row_to_task(row: &PgRow) -> Task {
        let status_str: String = row.get("status");
        let status: TaskStatus =
            serde_json::from_value(JsonValue::String(status_str)).unwrap_or(TaskStatus::Pending);

        let created_at_i64: i64 = row.get("created_at");
        let updated_at_i64: i64 = row.get("updated_at");
        let completed_at_i64: Option<i64> = row.get("completed_at");
        let ttl_i32: Option<i32> = row.get("ttl");

        let params: Option<JsonValue> = row.get("params");
        let result: Option<JsonValue> = row.get("result");
        let error: Option<JsonValue> = row.get("error");
        let metadata: Option<JsonValue> = row.get("metadata");
        let auth_config: Option<JsonValue> = row.get("auth_config");
        let webhooks: Option<JsonValue> = row.get("webhooks");
        let cleanup: Option<JsonValue> = row.get("cleanup");

        Task {
            id: row.get("id"),
            r#type: row.get("type"),
            status,
            params: params.and_then(|v| serde_json::from_value(v).ok()),
            result: result.and_then(|v| serde_json::from_value(v).ok()),
            error: error.and_then(|v| serde_json::from_value::<TaskError>(v).ok()),
            metadata: metadata.and_then(|v| serde_json::from_value(v).ok()),
            auth_config: auth_config
                .and_then(|v| serde_json::from_value::<TaskAuthConfig>(v).ok()),
            webhooks: webhooks
                .and_then(|v| serde_json::from_value::<Vec<WebhookConfig>>(v).ok()),
            cleanup: cleanup.and_then(|v| serde_json::from_value::<CleanupConfig>(v).ok()),
            created_at: created_at_i64 as f64,
            updated_at: updated_at_i64 as f64,
            completed_at: completed_at_i64.map(|v| v as f64),
            ttl: ttl_i32.map(|v| v as u64),
        }
    }

    /// Convert a database row into a `TaskEvent`.
    fn row_to_event(row: &PgRow) -> TaskEvent {
        let level_str: String = row.get("level");
        let level: Level =
            serde_json::from_value(JsonValue::String(level_str)).unwrap_or(Level::Info);

        let idx: i32 = row.get("idx");
        let timestamp_i64: i64 = row.get("timestamp");
        let data: Option<JsonValue> = row.get("data");

        let series_mode_str: Option<String> = row.get("series_mode");
        let series_mode: Option<SeriesMode> = series_mode_str
            .and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());

        TaskEvent {
            id: row.get("id"),
            task_id: row.get("task_id"),
            index: idx as u64,
            timestamp: timestamp_i64 as f64,
            r#type: row.get("type"),
            level,
            data: data.unwrap_or(JsonValue::Null),
            series_id: row.get("series_id"),
            series_mode,
        }
    }
}

#[async_trait]
impl LongTermStore for PostgresLongTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let tasks_table = &self.tables.tasks;

        let params_json: Option<JsonValue> =
            task.params.as_ref().map(|p| serde_json::to_value(p).unwrap_or(JsonValue::Null));
        let result_json: Option<JsonValue> =
            task.result.as_ref().map(|r| serde_json::to_value(r).unwrap_or(JsonValue::Null));
        let error_json: Option<JsonValue> =
            task.error.as_ref().map(|e| serde_json::to_value(e).unwrap_or(JsonValue::Null));
        let metadata_json: Option<JsonValue> =
            task.metadata.as_ref().map(|m| serde_json::to_value(m).unwrap_or(JsonValue::Null));
        let auth_config_json: Option<JsonValue> = task
            .auth_config
            .as_ref()
            .map(|a| serde_json::to_value(a).unwrap_or(JsonValue::Null));
        let webhooks_json: Option<JsonValue> = task
            .webhooks
            .as_ref()
            .map(|w| serde_json::to_value(w).unwrap_or(JsonValue::Null));
        let cleanup_json: Option<JsonValue> =
            task.cleanup.as_ref().map(|c| serde_json::to_value(c).unwrap_or(JsonValue::Null));

        let created_at = task.created_at as i64;
        let updated_at = task.updated_at as i64;
        let completed_at = task.completed_at.map(|v| v as i64);
        let ttl = task.ttl.map(|v| v as i32);

        let sql = format!(
            r#"
            INSERT INTO {tasks_table} (
                id, type, status, params, result, error, metadata,
                auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14
            )
            ON CONFLICT (id) DO UPDATE SET
                status = EXCLUDED.status,
                result = EXCLUDED.result,
                error = EXCLUDED.error,
                metadata = EXCLUDED.metadata,
                updated_at = EXCLUDED.updated_at,
                completed_at = EXCLUDED.completed_at
            "#
        );

        let status_str =
            serde_json::to_value(&task.status).map(|v| v.as_str().unwrap_or("pending").to_string())?;

        sqlx::query(&sql)
            .bind(&task.id)
            .bind(&task.r#type)
            .bind(&status_str)
            .bind(&params_json)
            .bind(&result_json)
            .bind(&error_json)
            .bind(&metadata_json)
            .bind(&auth_config_json)
            .bind(&webhooks_json)
            .bind(&cleanup_json)
            .bind(created_at)
            .bind(updated_at)
            .bind(completed_at)
            .bind(ttl)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        let tasks_table = &self.tables.tasks;
        let sql = format!("SELECT * FROM {tasks_table} WHERE id = $1");

        let row = sqlx::query(&sql)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(Self::row_to_task))
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let events_table = &self.tables.events;

        let sql = format!(
            r#"
            INSERT INTO {events_table} (
                id, task_id, idx, timestamp, type, level, data, series_id, series_mode
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9
            )
            ON CONFLICT (id) DO NOTHING
            "#
        );

        let level_str =
            serde_json::to_value(&event.level).map(|v| v.as_str().unwrap_or("info").to_string())?;
        let series_mode_str: Option<String> = event.series_mode.as_ref().and_then(|sm| {
            serde_json::to_value(sm)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        });

        let idx = event.index as i32;
        let timestamp = event.timestamp as i64;
        let data_json: Option<JsonValue> = if event.data.is_null() {
            None
        } else {
            Some(event.data.clone())
        };

        sqlx::query(&sql)
            .bind(&event.id)
            .bind(&event.task_id)
            .bind(idx)
            .bind(timestamp)
            .bind(&event.r#type)
            .bind(&level_str)
            .bind(&data_json)
            .bind(&event.series_id)
            .bind(&series_mode_str)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let events_table = &self.tables.events;
        let since = opts.as_ref().and_then(|o| o.since.as_ref());
        let limit = opts.as_ref().and_then(|o| o.limit);

        let limit_clause = limit
            .map(|l| format!("LIMIT {l}"))
            .unwrap_or_default();

        let rows = if let Some(since) = since {
            if let Some(index) = since.index {
                let sql = format!(
                    "SELECT * FROM {events_table} WHERE task_id = $1 AND idx > $2 ORDER BY idx ASC {limit_clause}"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(index as i32)
                    .fetch_all(&self.pool)
                    .await?
            } else if let Some(timestamp) = since.timestamp {
                let sql = format!(
                    "SELECT * FROM {events_table} WHERE task_id = $1 AND timestamp > $2 ORDER BY idx ASC {limit_clause}"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(timestamp as i64)
                    .fetch_all(&self.pool)
                    .await?
            } else if let Some(ref id) = since.id {
                // Look up the anchor event's idx, then fetch events after it
                let anchor_sql =
                    format!("SELECT idx FROM {events_table} WHERE id = $1");
                let anchor_row = sqlx::query(&anchor_sql)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;
                let anchor_idx: i32 = anchor_row
                    .as_ref()
                    .map(|r| r.get("idx"))
                    .unwrap_or(-1);

                let sql = format!(
                    "SELECT * FROM {events_table} WHERE task_id = $1 AND idx > $2 ORDER BY idx ASC {limit_clause}"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .bind(anchor_idx)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                // since exists but has no usable cursor fields
                let sql = format!(
                    "SELECT * FROM {events_table} WHERE task_id = $1 ORDER BY idx ASC {limit_clause}"
                );
                sqlx::query(&sql)
                    .bind(task_id)
                    .fetch_all(&self.pool)
                    .await?
            }
        } else {
            let sql = format!(
                "SELECT * FROM {events_table} WHERE task_id = $1 ORDER BY idx ASC {limit_clause}"
            );
            sqlx::query(&sql)
                .bind(task_id)
                .fetch_all(&self.pool)
                .await?
        };

        Ok(rows.iter().map(Self::row_to_event).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn table_names_with_default_prefix() {
        let tables = TableNames::new("taskcast");
        assert_eq!(tables.tasks, "taskcast_tasks");
        assert_eq!(tables.events, "taskcast_events");
    }

    #[test]
    fn table_names_with_custom_prefix() {
        let tables = TableNames::new("myapp");
        assert_eq!(tables.tasks, "myapp_tasks");
        assert_eq!(tables.events, "myapp_events");
    }

    #[test]
    fn table_names_with_empty_prefix() {
        let tables = TableNames::new("");
        assert_eq!(tables.tasks, "_tasks");
        assert_eq!(tables.events, "_events");
    }

    #[test]
    fn status_serializes_for_db() {
        let status = TaskStatus::Running;
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v.as_str().unwrap(), "running");
    }

    #[test]
    fn status_deserializes_from_db_string() {
        let status: TaskStatus =
            serde_json::from_value(JsonValue::String("completed".to_string())).unwrap();
        assert_eq!(status, TaskStatus::Completed);
    }

    #[test]
    fn level_serializes_for_db() {
        let level = Level::Warn;
        let v = serde_json::to_value(&level).unwrap();
        assert_eq!(v.as_str().unwrap(), "warn");
    }

    #[test]
    fn level_deserializes_from_db_string() {
        let level: Level =
            serde_json::from_value(JsonValue::String("error".to_string())).unwrap();
        assert_eq!(level, Level::Error);
    }

    #[test]
    fn series_mode_roundtrip_through_string() {
        let mode = SeriesMode::Accumulate;
        let v = serde_json::to_value(&mode).unwrap();
        let s = v.as_str().unwrap().to_string();
        let back: SeriesMode =
            serde_json::from_value(JsonValue::String(s)).unwrap();
        assert_eq!(back, SeriesMode::Accumulate);
    }

    #[test]
    fn task_params_to_json_value() {
        let mut params = HashMap::new();
        params.insert("url".to_string(), serde_json::json!("https://example.com"));
        params.insert("depth".to_string(), serde_json::json!(3));
        let v = serde_json::to_value(&params).unwrap();
        assert_eq!(v["url"], "https://example.com");
        assert_eq!(v["depth"], 3);
    }

    #[test]
    fn task_error_to_json_value() {
        let err = TaskError {
            code: Some("TIMEOUT".to_string()),
            message: "Request timed out".to_string(),
            details: None,
        };
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["code"], "TIMEOUT");
        assert_eq!(v["message"], "Request timed out");
    }

    #[test]
    fn optional_json_none_stays_none() {
        let params: Option<HashMap<String, JsonValue>> = None;
        let json: Option<JsonValue> =
            params.as_ref().map(|p| serde_json::to_value(p).unwrap_or(JsonValue::Null));
        assert!(json.is_none());
    }

    #[test]
    fn timestamp_f64_to_i64_conversion() {
        let ts: f64 = 1700000000000.0;
        let as_i64 = ts as i64;
        assert_eq!(as_i64, 1700000000000_i64);
        let back = as_i64 as f64;
        assert!((back - ts).abs() < f64::EPSILON);
    }

    #[test]
    fn ttl_u64_to_i32_conversion() {
        let ttl: u64 = 3600;
        let as_i32 = ttl as i32;
        assert_eq!(as_i32, 3600);
        let back = as_i32 as u64;
        assert_eq!(back, ttl);
    }
}
