use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use taskcast_core::types::{EventQueryOptions, ShortTermStore, Task, TaskEvent};

use crate::row_helpers::{
    json_value_to_string, level_to_string, row_to_event, row_to_task, series_mode_to_string,
    status_to_string, to_json_string,
};

pub struct SqliteShortTermStore {
    pool: SqlitePool,
}

impl SqliteShortTermStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ShortTermStore for SqliteShortTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let status_str = status_to_string(&task.status);
        let params_json = to_json_string(&task.params);
        let result_json = to_json_string(&task.result);
        let error_json = to_json_string(&task.error);
        let metadata_json = to_json_string(&task.metadata);
        let auth_config_json = to_json_string(&task.auth_config);
        let webhooks_json = to_json_string(&task.webhooks);
        let cleanup_json = to_json_string(&task.cleanup);

        let created_at = task.created_at as i64;
        let updated_at = task.updated_at as i64;
        let completed_at = task.completed_at.map(|v| v as i64);
        let ttl = task.ttl.map(|v| v as i32);

        sqlx::query(
            r#"
            INSERT INTO taskcast_tasks (
                id, type, status, params, result, error, metadata,
                auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
            )
            ON CONFLICT (id) DO UPDATE SET
                status = excluded.status,
                result = excluded.result,
                error = excluded.error,
                metadata = excluded.metadata,
                updated_at = excluded.updated_at,
                completed_at = excluded.completed_at
            "#,
        )
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
        let row = sqlx::query("SELECT * FROM taskcast_tasks WHERE id = ?1")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.as_ref().map(row_to_task))
    }

    async fn next_index(
        &self,
        task_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let row = sqlx::query(
            r#"
            INSERT INTO taskcast_index_counters (task_id, counter)
            VALUES (?1, 0)
            ON CONFLICT (task_id) DO UPDATE SET counter = taskcast_index_counters.counter + 1
            RETURNING counter
            "#,
        )
        .bind(task_id)
        .fetch_one(&self.pool)
        .await?;

        let counter: i32 = row.get("counter");
        Ok(counter as u64)
    }

    async fn append_event(
        &self,
        _task_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let level_str = level_to_string(&event.level);
        let series_mode_str: Option<String> =
            event.series_mode.as_ref().and_then(series_mode_to_string);
        let idx = event.index as i32;
        let timestamp = event.timestamp as i64;
        let data_str = json_value_to_string(&event.data);

        sqlx::query(
            r#"
            INSERT INTO taskcast_events (
                id, task_id, idx, timestamp, type, level, data, series_id, series_mode
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
            )
            "#,
        )
        .bind(&event.id)
        .bind(&event.task_id)
        .bind(idx)
        .bind(timestamp)
        .bind(&event.r#type)
        .bind(&level_str)
        .bind(&data_str)
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
        let since = opts.as_ref().and_then(|o| o.since.as_ref());
        let limit = opts.as_ref().and_then(|o| o.limit);

        // When no limit is specified, use a very large value (effectively unlimited).
        let limit_val = limit.map(|l| l as i64).unwrap_or(i64::MAX);

        let rows = if let Some(since) = since {
            if let Some(ref id) = since.id {
                // since.id takes priority: look up anchor's idx, then fetch events after it.
                // COALESCE ensures that if the anchor is not found, we return all events (idx > -1).
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1
                      AND idx > COALESCE(
                          (SELECT idx FROM taskcast_events WHERE id = ?2),
                          -1
                      )
                    ORDER BY idx ASC
                    LIMIT ?3
                    "#,
                )
                .bind(task_id)
                .bind(id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else if let Some(index) = since.index {
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1 AND idx > ?2
                    ORDER BY idx ASC
                    LIMIT ?3
                    "#,
                )
                .bind(task_id)
                .bind(index as i32)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else if let Some(timestamp) = since.timestamp {
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1 AND timestamp > ?2
                    ORDER BY idx ASC
                    LIMIT ?3
                    "#,
                )
                .bind(task_id)
                .bind(timestamp as i64)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            } else {
                // since exists but has no usable cursor fields
                sqlx::query(
                    r#"
                    SELECT * FROM taskcast_events
                    WHERE task_id = ?1
                    ORDER BY idx ASC
                    LIMIT ?2
                    "#,
                )
                .bind(task_id)
                .bind(limit_val)
                .fetch_all(&self.pool)
                .await?
            }
        } else {
            sqlx::query(
                r#"
                SELECT * FROM taskcast_events
                WHERE task_id = ?1
                ORDER BY idx ASC
                LIMIT ?2
                "#,
            )
            .bind(task_id)
            .bind(limit_val)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.iter().map(row_to_event).collect())
    }

    async fn set_ttl(
        &self,
        _task_id: &str,
        _ttl_seconds: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // No-op for SQLite — TTL expiration is handled at the engine level,
        // not via database-level expiry like Redis.
        Ok(())
    }

    async fn get_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let row = sqlx::query(
            "SELECT event_json FROM taskcast_series_latest WHERE task_id = ?1 AND series_id = ?2",
        )
        .bind(task_id)
        .bind(series_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let json_str: String = row.get("event_json");
                let event: TaskEvent = serde_json::from_str(&json_str)?;
                Ok(Some(event))
            }
            None => Ok(None),
        }
    }

    async fn set_series_latest(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event_json = serde_json::to_string(&event)?;

        sqlx::query(
            r#"
            INSERT INTO taskcast_series_latest (task_id, series_id, event_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT (task_id, series_id) DO UPDATE SET
                event_json = excluded.event_json
            "#,
        )
        .bind(task_id)
        .bind(series_id)
        .bind(&event_json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn replace_last_series_event(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Get the previous series latest event
        let prev = self.get_series_latest(task_id, series_id).await?;

        if let Some(prev) = prev {
            // Update only content fields of the previous event in the events table,
            // preserving the original event's id and idx to maintain position ordering.
            let level_str = level_to_string(&event.level);
            let series_mode_str: Option<String> =
                event.series_mode.as_ref().and_then(series_mode_to_string);
            let data_str = json_value_to_string(&event.data);

            sqlx::query(
                r#"
                UPDATE taskcast_events SET
                    type = ?1,
                    level = ?2,
                    data = ?3,
                    series_id = ?4,
                    series_mode = ?5
                WHERE id = ?6
                "#,
            )
            .bind(&event.r#type)
            .bind(&level_str)
            .bind(&data_str)
            .bind(&event.series_id)
            .bind(&series_mode_str)
            .bind(&prev.id)
            .execute(&self.pool)
            .await?;
        } else {
            // No previous series event — append as a new event
            self.append_event(task_id, event.clone()).await?;
        }

        // Always update the series latest entry
        self.set_series_latest(task_id, series_id, event).await?;

        Ok(())
    }
}
