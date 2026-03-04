use serde_json::Value as JsonValue;
use sqlx::sqlite::SqliteRow;
use sqlx::Row;
use std::collections::HashMap;

use taskcast_core::types::{
    CleanupConfig, Level, SeriesMode, Task, TaskAuthConfig, TaskError, TaskEvent, TaskStatus,
    WebhookConfig,
};

/// Convert a SQLite row from the tasks table into a `Task`.
pub fn row_to_task(row: &SqliteRow) -> Task {
    let status_str: String = row.get("status");
    let status: TaskStatus =
        serde_json::from_value(JsonValue::String(status_str)).unwrap_or(TaskStatus::Pending);

    let created_at_i64: i64 = row.get("created_at");
    let updated_at_i64: i64 = row.get("updated_at");
    let completed_at_i64: Option<i64> = row.get("completed_at");
    let ttl_i32: Option<i32> = row.get("ttl");

    let params_str: Option<String> = row.get("params");
    let result_str: Option<String> = row.get("result");
    let error_str: Option<String> = row.get("error");
    let metadata_str: Option<String> = row.get("metadata");
    let auth_config_str: Option<String> = row.get("auth_config");
    let webhooks_str: Option<String> = row.get("webhooks");
    let cleanup_str: Option<String> = row.get("cleanup");

    Task {
        id: row.get("id"),
        r#type: row.get("type"),
        status,
        params: params_str
            .and_then(|s| serde_json::from_str::<HashMap<String, JsonValue>>(&s).ok()),
        result: result_str
            .and_then(|s| serde_json::from_str::<HashMap<String, JsonValue>>(&s).ok()),
        error: error_str.and_then(|s| serde_json::from_str::<TaskError>(&s).ok()),
        metadata: metadata_str
            .and_then(|s| serde_json::from_str::<HashMap<String, JsonValue>>(&s).ok()),
        auth_config: auth_config_str
            .and_then(|s| serde_json::from_str::<TaskAuthConfig>(&s).ok()),
        webhooks: webhooks_str
            .and_then(|s| serde_json::from_str::<Vec<WebhookConfig>>(&s).ok()),
        cleanup: cleanup_str.and_then(|s| serde_json::from_str::<CleanupConfig>(&s).ok()),
        created_at: created_at_i64 as f64,
        updated_at: updated_at_i64 as f64,
        completed_at: completed_at_i64.map(|v| v as f64),
        ttl: ttl_i32.map(|v| v as u64),
    }
}

/// Convert a SQLite row from the events table into a `TaskEvent`.
pub fn row_to_event(row: &SqliteRow) -> TaskEvent {
    let level_str: String = row.get("level");
    let level: Level =
        serde_json::from_value(JsonValue::String(level_str)).unwrap_or(Level::Info);

    let idx: i32 = row.get("idx");
    let timestamp_i64: i64 = row.get("timestamp");
    let data_str: Option<String> = row.get("data");

    let series_mode_str: Option<String> = row.get("series_mode");
    let series_mode: Option<SeriesMode> =
        series_mode_str.and_then(|s| serde_json::from_value(JsonValue::String(s)).ok());

    let data: JsonValue = data_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(JsonValue::Null);

    TaskEvent {
        id: row.get("id"),
        task_id: row.get("task_id"),
        index: idx as u64,
        timestamp: timestamp_i64 as f64,
        r#type: row.get("type"),
        level,
        data,
        series_id: row.get("series_id"),
        series_mode,
    }
}

/// Serialize a `TaskStatus` to its string representation for DB storage.
pub fn status_to_string(status: &TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "pending".to_string())
}

/// Serialize a `Level` to its string representation for DB storage.
pub fn level_to_string(level: &Level) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "info".to_string())
}

/// Serialize a `SeriesMode` to its string representation for DB storage.
pub fn series_mode_to_string(mode: &SeriesMode) -> Option<String> {
    serde_json::to_value(mode)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

/// Serialize an optional JSON-serializable value to an optional JSON string for TEXT storage.
pub fn to_json_string<T: serde::Serialize>(value: &Option<T>) -> Option<String> {
    value
        .as_ref()
        .and_then(|v| serde_json::to_string(v).ok())
}

/// Serialize a `serde_json::Value` to an optional string (None if null).
pub fn json_value_to_string(value: &JsonValue) -> Option<String> {
    if value.is_null() {
        None
    } else {
        Some(value.to_string())
    }
}
