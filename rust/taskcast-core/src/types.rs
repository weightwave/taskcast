use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Task ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionScope {
    #[serde(rename = "task:create")]
    TaskCreate,
    #[serde(rename = "task:manage")]
    TaskManage,
    #[serde(rename = "event:publish")]
    EventPublish,
    #[serde(rename = "event:subscribe")]
    EventSubscribe,
    #[serde(rename = "event:history")]
    EventHistory,
    #[serde(rename = "webhook:create")]
    WebhookCreate,
    #[serde(rename = "*")]
    All,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAuthRule {
    pub r#match: TaskAuthRuleMatch,
    pub require: TaskAuthRuleRequire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAuthRuleMatch {
    pub scope: Vec<PermissionScope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAuthRuleRequire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claims: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAuthConfig {
    pub rules: Vec<TaskAuthRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookConfig {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<SubscribeFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackoffStrategy {
    Fixed,
    Exponential,
    Linear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryConfig {
    pub retries: u32,
    pub backoff: BackoffStrategy,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeriesMode {
    KeepAll,
    Accumulate,
    Latest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanupTarget {
    All,
    Events,
    Task,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRuleMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<TaskStatus>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupTrigger {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupEventFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub levels: Option<Vec<Level>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_than_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_mode: Option<Vec<SeriesMode>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#match: Option<CleanupRuleMatch>,
    pub trigger: CleanupTrigger,
    pub target: CleanupTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_filter: Option<CleanupEventFilter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupConfig {
    pub rules: Vec<CleanupRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TaskError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub created_at: f64,
    pub updated_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_config: Option<TaskAuthConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhooks: Option<Vec<WebhookConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<CleanupConfig>,
}

// ─── Events ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub id: String,
    pub task_id: String,
    pub index: u64,
    pub timestamp: f64,
    pub r#type: String,
    pub level: Level,
    pub data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_mode: Option<SeriesMode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SSEEnvelope {
    pub filtered_index: u64,
    pub raw_index: u64,
    pub event_id: String,
    pub task_id: String,
    pub r#type: String,
    pub timestamp: f64,
    pub level: Level,
    pub data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_mode: Option<SeriesMode>,
}

// ─── Subscription ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SinceCursor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<SinceCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub levels: Option<Vec<Level>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_status: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventQueryOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<SinceCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

// ─── Storage Interfaces ──────────────────────────────────────────────────────

#[async_trait]
pub trait BroadcastProvider: Send + Sync {
    async fn publish(&self, channel: &str, event: TaskEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn subscribe(
        &self,
        channel: &str,
        handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync>;
}

#[async_trait]
pub trait ShortTermStore: Send + Sync {
    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>>;
    async fn append_event(&self, task_id: &str, event: TaskEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_events(&self, task_id: &str, opts: Option<EventQueryOptions>) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>;
    async fn set_ttl(&self, task_id: &str, ttl_seconds: u64) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_series_latest(&self, task_id: &str, series_id: &str) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>;
    async fn set_series_latest(&self, task_id: &str, series_id: &str, event: TaskEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn replace_last_series_event(&self, task_id: &str, series_id: &str, event: TaskEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

#[async_trait]
pub trait LongTermStore: Send + Sync {
    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>>;
    async fn save_event(&self, event: TaskEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_events(&self, task_id: &str, opts: Option<EventQueryOptions>) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>;
}

// ─── Hooks ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorContext {
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// Hooks for monitoring and reacting to taskcast events.
///
/// All methods have default no-op implementations, so consumers only
/// need to implement the hooks they care about.
pub trait TaskcastHooks: Send + Sync {
    fn on_task_failed(&self, _task: &Task, _error: &TaskError) {}
    fn on_task_timeout(&self, _task: &Task) {}
    fn on_unhandled_error(&self, _err: &(dyn std::error::Error + Send + Sync), _context: &ErrorContext) {}
    fn on_event_dropped(&self, _event: &TaskEvent, _reason: &str) {}
    fn on_webhook_failed(&self, _config: &WebhookConfig, _err: &(dyn std::error::Error + Send + Sync)) {}
    fn on_sse_connect(&self, _task_id: &str, _client_id: &str) {}
    fn on_sse_disconnect(&self, _task_id: &str, _client_id: &str, _duration: f64) {}
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ─── TaskStatus ─────────────────────────────────────────────────────

    #[test]
    fn task_status_serializes_to_camel_case() {
        assert_eq!(serde_json::to_string(&TaskStatus::Pending).unwrap(), "\"pending\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Running).unwrap(), "\"running\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Completed).unwrap(), "\"completed\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Failed).unwrap(), "\"failed\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Timeout).unwrap(), "\"timeout\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Cancelled).unwrap(), "\"cancelled\"");
    }

    #[test]
    fn task_status_deserializes_from_camel_case() {
        assert_eq!(serde_json::from_str::<TaskStatus>("\"pending\"").unwrap(), TaskStatus::Pending);
        assert_eq!(serde_json::from_str::<TaskStatus>("\"cancelled\"").unwrap(), TaskStatus::Cancelled);
    }

    // ─── Level ──────────────────────────────────────────────────────────

    #[test]
    fn level_serializes_correctly() {
        assert_eq!(serde_json::to_string(&Level::Debug).unwrap(), "\"debug\"");
        assert_eq!(serde_json::to_string(&Level::Info).unwrap(), "\"info\"");
        assert_eq!(serde_json::to_string(&Level::Warn).unwrap(), "\"warn\"");
        assert_eq!(serde_json::to_string(&Level::Error).unwrap(), "\"error\"");
    }

    // ─── SeriesMode ─────────────────────────────────────────────────────

    #[test]
    fn series_mode_serializes_to_kebab_case() {
        assert_eq!(serde_json::to_string(&SeriesMode::KeepAll).unwrap(), "\"keep-all\"");
        assert_eq!(serde_json::to_string(&SeriesMode::Accumulate).unwrap(), "\"accumulate\"");
        assert_eq!(serde_json::to_string(&SeriesMode::Latest).unwrap(), "\"latest\"");
    }

    #[test]
    fn series_mode_roundtrip() {
        let mode = SeriesMode::KeepAll;
        let json = serde_json::to_string(&mode).unwrap();
        let back: SeriesMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    // ─── PermissionScope ────────────────────────────────────────────────

    #[test]
    fn permission_scope_serializes_with_colon_notation() {
        assert_eq!(serde_json::to_string(&PermissionScope::TaskCreate).unwrap(), "\"task:create\"");
        assert_eq!(serde_json::to_string(&PermissionScope::TaskManage).unwrap(), "\"task:manage\"");
        assert_eq!(serde_json::to_string(&PermissionScope::EventPublish).unwrap(), "\"event:publish\"");
        assert_eq!(serde_json::to_string(&PermissionScope::EventSubscribe).unwrap(), "\"event:subscribe\"");
        assert_eq!(serde_json::to_string(&PermissionScope::EventHistory).unwrap(), "\"event:history\"");
        assert_eq!(serde_json::to_string(&PermissionScope::WebhookCreate).unwrap(), "\"webhook:create\"");
        assert_eq!(serde_json::to_string(&PermissionScope::All).unwrap(), "\"*\"");
    }

    #[test]
    fn permission_scope_roundtrip() {
        let scope = PermissionScope::All;
        let json = serde_json::to_string(&scope).unwrap();
        let back: PermissionScope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, scope);
    }

    // ─── BackoffStrategy ────────────────────────────────────────────────

    #[test]
    fn backoff_strategy_serializes_correctly() {
        assert_eq!(serde_json::to_string(&BackoffStrategy::Fixed).unwrap(), "\"fixed\"");
        assert_eq!(serde_json::to_string(&BackoffStrategy::Exponential).unwrap(), "\"exponential\"");
        assert_eq!(serde_json::to_string(&BackoffStrategy::Linear).unwrap(), "\"linear\"");
    }

    // ─── CleanupTarget ──────────────────────────────────────────────────

    #[test]
    fn cleanup_target_serializes_correctly() {
        assert_eq!(serde_json::to_string(&CleanupTarget::All).unwrap(), "\"all\"");
        assert_eq!(serde_json::to_string(&CleanupTarget::Events).unwrap(), "\"events\"");
        assert_eq!(serde_json::to_string(&CleanupTarget::Task).unwrap(), "\"task\"");
    }

    // ─── TaskError ──────────────────────────────────────────────────────

    #[test]
    fn task_error_minimal_serializes_correctly() {
        let err = TaskError {
            code: None,
            message: "something broke".to_string(),
            details: None,
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json, json!({ "message": "something broke" }));
    }

    #[test]
    fn task_error_full_serializes_correctly() {
        let mut details = HashMap::new();
        details.insert("key".to_string(), json!("value"));
        let err = TaskError {
            code: Some("ERR_001".to_string()),
            message: "something broke".to_string(),
            details: Some(details),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json, json!({
            "code": "ERR_001",
            "message": "something broke",
            "details": { "key": "value" }
        }));
    }

    // ─── Task (minimal) ─────────────────────────────────────────────────

    #[test]
    fn task_minimal_serializes_with_correct_field_names() {
        let task = Task {
            id: "task_01".to_string(),
            r#type: None,
            status: TaskStatus::Pending,
            params: None,
            result: None,
            error: None,
            metadata: None,
            created_at: 1700000000000.0,
            updated_at: 1700000000000.0,
            completed_at: None,
            ttl: None,
            auth_config: None,
            webhooks: None,
            cleanup: None,
        };
        let json = serde_json::to_value(&task).unwrap();
        // Check camelCase field names
        assert_eq!(json["id"], "task_01");
        assert_eq!(json["status"], "pending");
        assert_eq!(json["createdAt"], 1700000000000.0_f64);
        assert_eq!(json["updatedAt"], 1700000000000.0_f64);
        // Optional fields should be absent
        assert!(json.get("type").is_none());
        assert!(json.get("params").is_none());
        assert!(json.get("result").is_none());
        assert!(json.get("error").is_none());
        assert!(json.get("metadata").is_none());
        assert!(json.get("completedAt").is_none());
        assert!(json.get("ttl").is_none());
        assert!(json.get("authConfig").is_none());
        assert!(json.get("webhooks").is_none());
        assert!(json.get("cleanup").is_none());
    }

    // ─── Task (full) ────────────────────────────────────────────────────

    #[test]
    fn task_full_serializes_with_all_fields() {
        let mut params = HashMap::new();
        params.insert("url".to_string(), json!("https://example.com"));

        let task = Task {
            id: "task_02".to_string(),
            r#type: Some("crawl".to_string()),
            status: TaskStatus::Completed,
            params: Some(params),
            result: Some(HashMap::new()),
            error: Some(TaskError {
                code: Some("ERR".to_string()),
                message: "fail".to_string(),
                details: None,
            }),
            metadata: Some(HashMap::new()),
            created_at: 1700000000000.0,
            updated_at: 1700000001000.0,
            completed_at: Some(1700000001000.0),
            ttl: Some(3600),
            auth_config: Some(TaskAuthConfig {
                rules: vec![TaskAuthRule {
                    r#match: TaskAuthRuleMatch {
                        scope: vec![PermissionScope::TaskCreate],
                    },
                    require: TaskAuthRuleRequire {
                        claims: None,
                        sub: Some(vec!["user1".to_string()]),
                    },
                }],
            }),
            webhooks: Some(vec![WebhookConfig {
                url: "https://hook.example.com".to_string(),
                filter: None,
                secret: Some("s3cret".to_string()),
                wrap: Some(true),
                retry: Some(RetryConfig {
                    retries: 3,
                    backoff: BackoffStrategy::Exponential,
                    initial_delay_ms: 1000,
                    max_delay_ms: 30000,
                    timeout_ms: 5000,
                }),
            }]),
            cleanup: Some(CleanupConfig {
                rules: vec![CleanupRule {
                    name: Some("cleanup-old".to_string()),
                    r#match: Some(CleanupRuleMatch {
                        task_types: Some(vec!["crawl".to_string()]),
                        status: Some(vec![TaskStatus::Completed]),
                    }),
                    trigger: CleanupTrigger {
                        after_ms: Some(86400000),
                    },
                    target: CleanupTarget::All,
                    event_filter: None,
                }],
            }),
        };

        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["id"], "task_02");
        assert_eq!(json["type"], "crawl");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["params"]["url"], "https://example.com");
        assert_eq!(json["createdAt"], 1700000000000.0_f64);
        assert_eq!(json["updatedAt"], 1700000001000.0_f64);
        assert_eq!(json["completedAt"], 1700000001000.0_f64);
        assert_eq!(json["ttl"], 3600);
        assert_eq!(json["error"]["code"], "ERR");
        assert_eq!(json["error"]["message"], "fail");

        // authConfig
        assert_eq!(json["authConfig"]["rules"][0]["match"]["scope"][0], "task:create");
        assert_eq!(json["authConfig"]["rules"][0]["require"]["sub"][0], "user1");

        // webhooks
        assert_eq!(json["webhooks"][0]["url"], "https://hook.example.com");
        assert_eq!(json["webhooks"][0]["secret"], "s3cret");
        assert_eq!(json["webhooks"][0]["wrap"], true);
        assert_eq!(json["webhooks"][0]["retry"]["retries"], 3);
        assert_eq!(json["webhooks"][0]["retry"]["backoff"], "exponential");
        assert_eq!(json["webhooks"][0]["retry"]["initialDelayMs"], 1000);
        assert_eq!(json["webhooks"][0]["retry"]["maxDelayMs"], 30000);
        assert_eq!(json["webhooks"][0]["retry"]["timeoutMs"], 5000);

        // cleanup
        assert_eq!(json["cleanup"]["rules"][0]["name"], "cleanup-old");
        assert_eq!(json["cleanup"]["rules"][0]["match"]["taskTypes"][0], "crawl");
        assert_eq!(json["cleanup"]["rules"][0]["match"]["status"][0], "completed");
        assert_eq!(json["cleanup"]["rules"][0]["trigger"]["afterMs"], 86400000);
        assert_eq!(json["cleanup"]["rules"][0]["target"], "all");
    }

    // ─── Task roundtrip ─────────────────────────────────────────────────

    #[test]
    fn task_roundtrip_serialization() {
        let task = Task {
            id: "task_rt".to_string(),
            r#type: Some("test".to_string()),
            status: TaskStatus::Running,
            params: None,
            result: None,
            error: None,
            metadata: None,
            created_at: 1700000000000.0,
            updated_at: 1700000000000.0,
            completed_at: None,
            ttl: None,
            auth_config: None,
            webhooks: None,
            cleanup: None,
        };
        let json_str = serde_json::to_string(&task).unwrap();
        let back: Task = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back.id, task.id);
        assert_eq!(back.r#type, task.r#type);
        assert_eq!(back.status, task.status);
        assert_eq!(back.created_at, task.created_at);
    }

    // ─── TaskEvent ──────────────────────────────────────────────────────

    #[test]
    fn task_event_serializes_with_correct_field_names() {
        let event = TaskEvent {
            id: "evt_01".to_string(),
            task_id: "task_01".to_string(),
            index: 0,
            timestamp: 1700000000000.0,
            r#type: "progress".to_string(),
            level: Level::Info,
            data: json!({ "percent": 50 }),
            series_id: None,
            series_mode: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["id"], "evt_01");
        assert_eq!(json["taskId"], "task_01");
        assert_eq!(json["index"], 0);
        assert_eq!(json["timestamp"], 1700000000000.0_f64);
        assert_eq!(json["type"], "progress");
        assert_eq!(json["level"], "info");
        assert_eq!(json["data"]["percent"], 50);
        assert!(json.get("seriesId").is_none());
        assert!(json.get("seriesMode").is_none());
    }

    #[test]
    fn task_event_with_series_serializes_correctly() {
        let event = TaskEvent {
            id: "evt_02".to_string(),
            task_id: "task_01".to_string(),
            index: 1,
            timestamp: 1700000001000.0,
            r#type: "log".to_string(),
            level: Level::Debug,
            data: json!("hello"),
            series_id: Some("series_01".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["seriesId"], "series_01");
        assert_eq!(json["seriesMode"], "accumulate");
    }

    #[test]
    fn task_event_roundtrip() {
        let event = TaskEvent {
            id: "evt_rt".to_string(),
            task_id: "task_rt".to_string(),
            index: 5,
            timestamp: 1700000000000.0,
            r#type: "status".to_string(),
            level: Level::Warn,
            data: json!(null),
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Latest),
        };
        let json_str = serde_json::to_string(&event).unwrap();
        let back: TaskEvent = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back, event);
    }

    // ─── SSEEnvelope ────────────────────────────────────────────────────

    #[test]
    fn sse_envelope_serializes_with_correct_field_names() {
        let envelope = SSEEnvelope {
            filtered_index: 3,
            raw_index: 7,
            event_id: "evt_01".to_string(),
            task_id: "task_01".to_string(),
            r#type: "progress".to_string(),
            timestamp: 1700000000000.0,
            level: Level::Info,
            data: json!({ "done": true }),
            series_id: None,
            series_mode: None,
        };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["filteredIndex"], 3);
        assert_eq!(json["rawIndex"], 7);
        assert_eq!(json["eventId"], "evt_01");
        assert_eq!(json["taskId"], "task_01");
        assert_eq!(json["type"], "progress");
        assert_eq!(json["timestamp"], 1700000000000.0_f64);
        assert_eq!(json["level"], "info");
        assert_eq!(json["data"]["done"], true);
        assert!(json.get("seriesId").is_none());
        assert!(json.get("seriesMode").is_none());
    }

    #[test]
    fn sse_envelope_with_series_serializes_correctly() {
        let envelope = SSEEnvelope {
            filtered_index: 0,
            raw_index: 0,
            event_id: "evt_03".to_string(),
            task_id: "task_01".to_string(),
            r#type: "data".to_string(),
            timestamp: 1700000000000.0,
            level: Level::Error,
            data: json!(42),
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::KeepAll),
        };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["seriesId"], "s1");
        assert_eq!(json["seriesMode"], "keep-all");
    }

    // ─── SinceCursor ────────────────────────────────────────────────────

    #[test]
    fn since_cursor_empty_serializes_to_empty_object() {
        let cursor = SinceCursor {
            id: None,
            index: None,
            timestamp: None,
        };
        let json = serde_json::to_value(&cursor).unwrap();
        assert_eq!(json, json!({}));
    }

    #[test]
    fn since_cursor_full_serializes_correctly() {
        let cursor = SinceCursor {
            id: Some("evt_01".to_string()),
            index: Some(5),
            timestamp: Some(1700000000000.0),
        };
        let json = serde_json::to_value(&cursor).unwrap();
        assert_eq!(json["id"], "evt_01");
        assert_eq!(json["index"], 5);
        assert_eq!(json["timestamp"], 1700000000000.0_f64);
    }

    // ─── SubscribeFilter ────────────────────────────────────────────────

    #[test]
    fn subscribe_filter_serializes_correctly() {
        let filter = SubscribeFilter {
            since: Some(SinceCursor {
                id: None,
                index: Some(10),
                timestamp: None,
            }),
            types: Some(vec!["progress".to_string(), "log".to_string()]),
            levels: Some(vec![Level::Info, Level::Error]),
            include_status: Some(true),
            wrap: Some(false),
        };
        let json = serde_json::to_value(&filter).unwrap();
        assert_eq!(json["since"]["index"], 10);
        assert_eq!(json["types"][0], "progress");
        assert_eq!(json["types"][1], "log");
        assert_eq!(json["levels"][0], "info");
        assert_eq!(json["levels"][1], "error");
        assert_eq!(json["includeStatus"], true);
        assert_eq!(json["wrap"], false);
    }

    // ─── EventQueryOptions ──────────────────────────────────────────────

    #[test]
    fn event_query_options_serializes_correctly() {
        let opts = EventQueryOptions {
            since: None,
            limit: Some(100),
        };
        let json = serde_json::to_value(&opts).unwrap();
        assert!(json.get("since").is_none());
        assert_eq!(json["limit"], 100);
    }

    // ─── RetryConfig ────────────────────────────────────────────────────

    #[test]
    fn retry_config_serializes_correctly() {
        let cfg = RetryConfig {
            retries: 5,
            backoff: BackoffStrategy::Linear,
            initial_delay_ms: 500,
            max_delay_ms: 10000,
            timeout_ms: 30000,
        };
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["retries"], 5);
        assert_eq!(json["backoff"], "linear");
        assert_eq!(json["initialDelayMs"], 500);
        assert_eq!(json["maxDelayMs"], 10000);
        assert_eq!(json["timeoutMs"], 30000);
    }

    // ─── WebhookConfig ──────────────────────────────────────────────────

    #[test]
    fn webhook_config_minimal_serializes_correctly() {
        let cfg = WebhookConfig {
            url: "https://example.com/hook".to_string(),
            filter: None,
            secret: None,
            wrap: None,
            retry: None,
        };
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json, json!({ "url": "https://example.com/hook" }));
    }

    // ─── CleanupRule ────────────────────────────────────────────────────

    #[test]
    fn cleanup_rule_serializes_correctly() {
        let rule = CleanupRule {
            name: Some("cleanup-events".to_string()),
            r#match: Some(CleanupRuleMatch {
                task_types: Some(vec!["download".to_string()]),
                status: Some(vec![TaskStatus::Completed, TaskStatus::Failed]),
            }),
            trigger: CleanupTrigger {
                after_ms: Some(3600000),
            },
            target: CleanupTarget::Events,
            event_filter: Some(CleanupEventFilter {
                types: Some(vec!["log".to_string()]),
                levels: Some(vec![Level::Debug]),
                older_than_ms: Some(86400000),
                series_mode: Some(vec![SeriesMode::KeepAll]),
            }),
        };
        let json = serde_json::to_value(&rule).unwrap();
        assert_eq!(json["name"], "cleanup-events");
        assert_eq!(json["match"]["taskTypes"][0], "download");
        assert_eq!(json["match"]["status"][0], "completed");
        assert_eq!(json["match"]["status"][1], "failed");
        assert_eq!(json["trigger"]["afterMs"], 3600000);
        assert_eq!(json["target"], "events");
        assert_eq!(json["eventFilter"]["types"][0], "log");
        assert_eq!(json["eventFilter"]["levels"][0], "debug");
        assert_eq!(json["eventFilter"]["olderThanMs"], 86400000);
        assert_eq!(json["eventFilter"]["seriesMode"][0], "keep-all");
    }

    // ─── TaskAuthConfig ─────────────────────────────────────────────────

    #[test]
    fn task_auth_config_serializes_correctly() {
        let mut claims = HashMap::new();
        claims.insert("role".to_string(), json!("admin"));

        let config = TaskAuthConfig {
            rules: vec![TaskAuthRule {
                r#match: TaskAuthRuleMatch {
                    scope: vec![PermissionScope::TaskCreate, PermissionScope::All],
                },
                require: TaskAuthRuleRequire {
                    claims: Some(claims),
                    sub: Some(vec!["user-abc".to_string()]),
                },
            }],
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["rules"][0]["match"]["scope"][0], "task:create");
        assert_eq!(json["rules"][0]["match"]["scope"][1], "*");
        assert_eq!(json["rules"][0]["require"]["claims"]["role"], "admin");
        assert_eq!(json["rules"][0]["require"]["sub"][0], "user-abc");
    }

    // ─── ErrorContext ───────────────────────────────────────────────────

    #[test]
    fn error_context_serializes_correctly() {
        let ctx = ErrorContext {
            operation: "saveTask".to_string(),
            task_id: Some("task_01".to_string()),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["operation"], "saveTask");
        assert_eq!(json["taskId"], "task_01");
    }

    #[test]
    fn error_context_without_task_id() {
        let ctx = ErrorContext {
            operation: "startup".to_string(),
            task_id: None,
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["operation"], "startup");
        assert!(json.get("taskId").is_none());
    }

    // ─── Deserialization from TypeScript-shaped JSON ─────────────────────

    #[test]
    fn task_deserializes_from_typescript_json() {
        let ts_json = json!({
            "id": "task_from_ts",
            "type": "render",
            "status": "running",
            "params": { "width": 1920, "height": 1080 },
            "createdAt": 1700000000000.0,
            "updatedAt": 1700000000500.0,
            "ttl": 7200
        });
        let task: Task = serde_json::from_value(ts_json).unwrap();
        assert_eq!(task.id, "task_from_ts");
        assert_eq!(task.r#type, Some("render".to_string()));
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.created_at, 1700000000000.0);
        assert_eq!(task.updated_at, 1700000000500.0);
        assert_eq!(task.ttl, Some(7200));
        assert!(task.params.is_some());
        let params = task.params.unwrap();
        assert_eq!(params["width"], json!(1920));
    }

    #[test]
    fn task_event_deserializes_from_typescript_json() {
        let ts_json = json!({
            "id": "evt_from_ts",
            "taskId": "task_01",
            "index": 3,
            "timestamp": 1700000001000.0,
            "type": "log",
            "level": "warn",
            "data": { "message": "something happened" },
            "seriesId": "s1",
            "seriesMode": "keep-all"
        });
        let event: TaskEvent = serde_json::from_value(ts_json).unwrap();
        assert_eq!(event.id, "evt_from_ts");
        assert_eq!(event.task_id, "task_01");
        assert_eq!(event.index, 3);
        assert_eq!(event.r#type, "log");
        assert_eq!(event.level, Level::Warn);
        assert_eq!(event.series_id, Some("s1".to_string()));
        assert_eq!(event.series_mode, Some(SeriesMode::KeepAll));
    }

    #[test]
    fn sse_envelope_deserializes_from_typescript_json() {
        let ts_json = json!({
            "filteredIndex": 2,
            "rawIndex": 5,
            "eventId": "evt_01",
            "taskId": "task_01",
            "type": "status",
            "timestamp": 1700000000000.0,
            "level": "error",
            "data": null
        });
        let envelope: SSEEnvelope = serde_json::from_value(ts_json).unwrap();
        assert_eq!(envelope.filtered_index, 2);
        assert_eq!(envelope.raw_index, 5);
        assert_eq!(envelope.event_id, "evt_01");
        assert_eq!(envelope.task_id, "task_01");
        assert_eq!(envelope.level, Level::Error);
        assert_eq!(envelope.data, json!(null));
        assert_eq!(envelope.series_id, None);
        assert_eq!(envelope.series_mode, None);
    }

    // ─── JSON key absence checks ────────────────────────────────────────

    #[test]
    fn optional_fields_are_absent_not_null_in_json() {
        // This is critical: TypeScript omits undefined fields, so Rust must too
        let task = Task {
            id: "t".to_string(),
            r#type: None,
            status: TaskStatus::Pending,
            params: None,
            result: None,
            error: None,
            metadata: None,
            created_at: 0.0,
            updated_at: 0.0,
            completed_at: None,
            ttl: None,
            auth_config: None,
            webhooks: None,
            cleanup: None,
        };
        let json_str = serde_json::to_string(&task).unwrap();
        // These keys must NOT appear at all
        assert!(!json_str.contains("\"type\""));
        assert!(!json_str.contains("\"params\""));
        assert!(!json_str.contains("\"result\""));
        assert!(!json_str.contains("\"error\""));
        assert!(!json_str.contains("\"metadata\""));
        assert!(!json_str.contains("\"completedAt\""));
        assert!(!json_str.contains("\"ttl\""));
        assert!(!json_str.contains("\"authConfig\""));
        assert!(!json_str.contains("\"webhooks\""));
        assert!(!json_str.contains("\"cleanup\""));
    }

    #[test]
    fn task_event_optional_fields_are_absent_not_null() {
        let event = TaskEvent {
            id: "e".to_string(),
            task_id: "t".to_string(),
            index: 0,
            timestamp: 0.0,
            r#type: "x".to_string(),
            level: Level::Info,
            data: json!(null),
            series_id: None,
            series_mode: None,
        };
        let json_str = serde_json::to_string(&event).unwrap();
        assert!(!json_str.contains("\"seriesId\""));
        assert!(!json_str.contains("\"seriesMode\""));
    }

    #[test]
    fn sse_envelope_optional_fields_are_absent_not_null() {
        let envelope = SSEEnvelope {
            filtered_index: 0,
            raw_index: 0,
            event_id: "e".to_string(),
            task_id: "t".to_string(),
            r#type: "x".to_string(),
            timestamp: 0.0,
            level: Level::Info,
            data: json!(null),
            series_id: None,
            series_mode: None,
        };
        let json_str = serde_json::to_string(&envelope).unwrap();
        assert!(!json_str.contains("\"seriesId\""));
        assert!(!json_str.contains("\"seriesMode\""));
    }

    // ─── CleanupConfig nested in Task ───────────────────────────────────

    #[test]
    fn cleanup_config_nested_serializes_correctly() {
        let task = Task {
            id: "t".to_string(),
            r#type: None,
            status: TaskStatus::Pending,
            params: None,
            result: None,
            error: None,
            metadata: None,
            created_at: 0.0,
            updated_at: 0.0,
            completed_at: None,
            ttl: None,
            auth_config: None,
            webhooks: None,
            cleanup: Some(CleanupConfig {
                rules: vec![CleanupRule {
                    name: None,
                    r#match: None,
                    trigger: CleanupTrigger { after_ms: Some(1000) },
                    target: CleanupTarget::Task,
                    event_filter: None,
                }],
            }),
        };
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["cleanup"]["rules"][0]["trigger"]["afterMs"], 1000);
        assert_eq!(json["cleanup"]["rules"][0]["target"], "task");
    }

    // ─── WebhookConfig with filter ──────────────────────────────────────

    #[test]
    fn webhook_config_with_filter_serializes_correctly() {
        let cfg = WebhookConfig {
            url: "https://example.com".to_string(),
            filter: Some(SubscribeFilter {
                since: None,
                types: Some(vec!["status".to_string()]),
                levels: None,
                include_status: Some(true),
                wrap: None,
            }),
            secret: None,
            wrap: None,
            retry: None,
        };
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["url"], "https://example.com");
        assert_eq!(json["filter"]["types"][0], "status");
        assert_eq!(json["filter"]["includeStatus"], true);
    }
}
