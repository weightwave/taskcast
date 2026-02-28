# Taskcast Rust Server Rewrite — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rewrite the Taskcast server-side packages (core, server, cli, postgres, redis) in Rust while maintaining 100% HTTP API compatibility with the existing TypeScript implementation.

**Architecture:** Cargo workspace with 5 crates: `taskcast-core` (types, state machine, engine, filter, series, memory adapters), `taskcast-server` (Axum HTTP + SSE + auth + webhook), `taskcast-postgres` (sqlx long-term store), `taskcast-redis` (redis broadcast + short-term store), `taskcast-cli` (clap entry point). All crates use async/await on Tokio runtime.

**Tech Stack:** Rust, Axum, Tokio, sqlx (Postgres), redis crate, clap, serde/serde_json, serde_yaml, jsonwebtoken, reqwest, hmac+sha2, ulid

---

## Task 1: Cargo Workspace + Core Types

**Files:**
- Create: `rust/Cargo.toml`
- Create: `rust/taskcast-core/Cargo.toml`
- Create: `rust/taskcast-core/src/lib.rs`
- Create: `rust/taskcast-core/src/types.rs`

**Step 1: Create workspace root Cargo.toml**

```toml
# rust/Cargo.toml
[workspace]
resolver = "2"
members = [
    "taskcast-core",
    "taskcast-server",
    "taskcast-postgres",
    "taskcast-redis",
    "taskcast-cli",
]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
thiserror = "2"
ulid = { version = "1", features = ["serde"] }
```

**Step 2: Create taskcast-core Cargo.toml**

```toml
# rust/taskcast-core/Cargo.toml
[package]
name = "taskcast-core"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
serde_yaml = "0.9"
tokio = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
ulid = { workspace = true }
```

**Step 3: Write types.rs — direct port of packages/core/src/types.ts**

Reference: `packages/core/src/types.ts`

```rust
// rust/taskcast-core/src/types.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeriesMode {
    KeepAll,
    Accumulate,
    Latest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAuthConfig {
    pub rules: Vec<AuthRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRule {
    #[serde(rename = "match")]
    pub match_: AuthRuleMatch,
    pub require: AuthRuleRequire,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRuleMatch {
    pub scope: Vec<PermissionScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRuleRequire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claims: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryConfig {
    pub retries: u32,
    pub backoff: BackoffStrategy,
    #[serde(rename = "initialDelayMs")]
    pub initial_delay_ms: u64,
    #[serde(rename = "maxDelayMs")]
    pub max_delay_ms: u64,
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackoffStrategy {
    Fixed,
    Exponential,
    Linear,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<SinceCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub levels: Option<Vec<Level>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "match")]
    pub match_: Option<CleanupMatch>,
    pub trigger: CleanupTrigger,
    pub target: CleanupTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_filter: Option<CleanupEventFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<TaskStatus>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupTrigger {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanupTarget {
    All,
    Events,
    Task,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none", rename = "completedAt")]
    pub completed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "authConfig")]
    pub auth_config: Option<TaskAuthConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhooks: Option<Vec<WebhookConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<CleanupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupConfig {
    pub rules: Vec<CleanupRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub id: String,
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub index: u64,
    pub timestamp: u64,
    pub r#type: String,
    pub level: Level,
    pub data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", rename = "seriesId")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "seriesMode")]
    pub series_mode: Option<SeriesMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SSEEnvelope {
    #[serde(rename = "filteredIndex")]
    pub filtered_index: u64,
    #[serde(rename = "rawIndex")]
    pub raw_index: u64,
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub r#type: String,
    pub timestamp: u64,
    pub level: Level,
    pub data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", rename = "seriesId")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "seriesMode")]
    pub series_mode: Option<SeriesMode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SinceCursor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct EventQueryOptions {
    pub since: Option<SinceCursor>,
    pub limit: Option<usize>,
}
```

**Step 4: Write lib.rs to expose types**

```rust
// rust/taskcast-core/src/lib.rs
pub mod types;

pub use types::*;
```

**Step 5: Verify it compiles**

Run: `cd rust && cargo check -p taskcast-core`
Expected: compiles with zero errors

**Step 6: Commit**

```bash
git add rust/
git commit -m "feat(rust): initialize Cargo workspace with taskcast-core types"
```

---

## Task 2: State Machine

**Files:**
- Create: `rust/taskcast-core/src/state_machine.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

Reference: `packages/core/src/state-machine.ts`

**Step 1: Write state_machine.rs tests**

```rust
// rust/taskcast-core/src/state_machine.rs
use crate::types::TaskStatus;

// ... (implementation follows)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_can_transition_to_running() {
        assert!(can_transition(TaskStatus::Pending, TaskStatus::Running));
    }

    #[test]
    fn pending_can_transition_to_cancelled() {
        assert!(can_transition(TaskStatus::Pending, TaskStatus::Cancelled));
    }

    #[test]
    fn pending_cannot_transition_to_completed() {
        assert!(!can_transition(TaskStatus::Pending, TaskStatus::Completed));
    }

    #[test]
    fn running_can_transition_to_completed() {
        assert!(can_transition(TaskStatus::Running, TaskStatus::Completed));
    }

    #[test]
    fn running_can_transition_to_failed() {
        assert!(can_transition(TaskStatus::Running, TaskStatus::Failed));
    }

    #[test]
    fn running_can_transition_to_timeout() {
        assert!(can_transition(TaskStatus::Running, TaskStatus::Timeout));
    }

    #[test]
    fn running_can_transition_to_cancelled() {
        assert!(can_transition(TaskStatus::Running, TaskStatus::Cancelled));
    }

    #[test]
    fn terminal_states_cannot_transition() {
        for terminal in [TaskStatus::Completed, TaskStatus::Failed, TaskStatus::Timeout, TaskStatus::Cancelled] {
            for target in [TaskStatus::Pending, TaskStatus::Running, TaskStatus::Completed, TaskStatus::Failed] {
                assert!(!can_transition(terminal, target));
            }
        }
    }

    #[test]
    fn same_status_cannot_transition() {
        assert!(!can_transition(TaskStatus::Running, TaskStatus::Running));
    }

    #[test]
    fn is_terminal_works() {
        assert!(!is_terminal(TaskStatus::Pending));
        assert!(!is_terminal(TaskStatus::Running));
        assert!(is_terminal(TaskStatus::Completed));
        assert!(is_terminal(TaskStatus::Failed));
        assert!(is_terminal(TaskStatus::Timeout));
        assert!(is_terminal(TaskStatus::Cancelled));
    }

    #[test]
    fn apply_transition_success() {
        assert_eq!(apply_transition(TaskStatus::Pending, TaskStatus::Running).unwrap(), TaskStatus::Running);
    }

    #[test]
    fn apply_transition_invalid() {
        assert!(apply_transition(TaskStatus::Pending, TaskStatus::Completed).is_err());
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cd rust && cargo test -p taskcast-core state_machine`
Expected: FAIL — functions not defined yet

**Step 3: Write implementation**

```rust
// rust/taskcast-core/src/state_machine.rs (top of file, before tests)
use crate::types::TaskStatus;

const TERMINAL_STATUSES: &[TaskStatus] = &[
    TaskStatus::Completed,
    TaskStatus::Failed,
    TaskStatus::Timeout,
    TaskStatus::Cancelled,
];

pub fn allowed_transitions(from: TaskStatus) -> &'static [TaskStatus] {
    match from {
        TaskStatus::Pending => &[TaskStatus::Running, TaskStatus::Cancelled],
        TaskStatus::Running => &[TaskStatus::Completed, TaskStatus::Failed, TaskStatus::Timeout, TaskStatus::Cancelled],
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Timeout | TaskStatus::Cancelled => &[],
    }
}

pub fn can_transition(from: TaskStatus, to: TaskStatus) -> bool {
    if from == to {
        return false;
    }
    allowed_transitions(from).contains(&to)
}

pub fn apply_transition(from: TaskStatus, to: TaskStatus) -> Result<TaskStatus, String> {
    if !can_transition(from, to) {
        return Err(format!("Invalid transition: {:?} → {:?}", from, to));
    }
    Ok(to)
}

pub fn is_terminal(status: TaskStatus) -> bool {
    TERMINAL_STATUSES.contains(&status)
}
```

**Step 4: Update lib.rs**

```rust
// rust/taskcast-core/src/lib.rs
pub mod types;
pub mod state_machine;

pub use types::*;
pub use state_machine::*;
```

**Step 5: Run tests**

Run: `cd rust && cargo test -p taskcast-core state_machine`
Expected: all tests PASS

**Step 6: Commit**

```bash
git add rust/taskcast-core/
git commit -m "feat(rust): add state machine with transition validation"
```

---

## Task 3: Filter Module

**Files:**
- Create: `rust/taskcast-core/src/filter.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

Reference: `packages/core/src/filter.ts`

**Step 1: Write tests**

```rust
// rust/taskcast-core/src/filter.rs
// ... (implementation follows)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn make_event(index: u64, event_type: &str, level: Level) -> TaskEvent {
        TaskEvent {
            id: format!("evt-{index}"),
            task_id: "task-1".into(),
            index,
            timestamp: 1000 + index,
            r#type: event_type.into(),
            level,
            data: serde_json::Value::Null,
            series_id: None,
            series_mode: None,
        }
    }

    #[test]
    fn matches_type_none_returns_true() {
        assert!(matches_type("anything", None));
    }

    #[test]
    fn matches_type_empty_returns_false() {
        assert!(!matches_type("anything", Some(&vec![])));
    }

    #[test]
    fn matches_type_wildcard() {
        assert!(matches_type("anything", Some(&vec!["*".into()])));
    }

    #[test]
    fn matches_type_exact() {
        assert!(matches_type("llm.delta", Some(&vec!["llm.delta".into()])));
        assert!(!matches_type("llm.done", Some(&vec!["llm.delta".into()])));
    }

    #[test]
    fn matches_type_prefix_wildcard() {
        // 'llm.*' matches 'llm.delta' but NOT 'llm'
        assert!(matches_type("llm.delta", Some(&vec!["llm.*".into()])));
        assert!(matches_type("llm.delta.chunk", Some(&vec!["llm.*".into()])));
        assert!(!matches_type("llm", Some(&vec!["llm.*".into()])));
    }

    #[test]
    fn matches_filter_excludes_status_when_disabled() {
        let event = make_event(0, "taskcast:status", Level::Info);
        let filter = SubscribeFilter {
            include_status: Some(false),
            ..Default::default()
        };
        assert!(!matches_filter(&event, &filter));
    }

    #[test]
    fn matches_filter_includes_status_by_default() {
        let event = make_event(0, "taskcast:status", Level::Info);
        let filter = SubscribeFilter::default();
        assert!(matches_filter(&event, &filter));
    }

    #[test]
    fn apply_filtered_index_basic() {
        let events = vec![
            make_event(0, "llm.delta", Level::Info),
            make_event(1, "taskcast:status", Level::Info),
            make_event(2, "llm.done", Level::Info),
        ];
        let filter = SubscribeFilter {
            types: Some(vec!["llm.*".into()]),
            ..Default::default()
        };
        let result = apply_filtered_index(&events, &filter);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filtered_index, 0);
        assert_eq!(result[0].event.index, 0);
        assert_eq!(result[1].filtered_index, 1);
        assert_eq!(result[1].event.index, 2);
    }

    #[test]
    fn apply_filtered_index_since() {
        let events = vec![
            make_event(0, "a", Level::Info),
            make_event(1, "b", Level::Info),
            make_event(2, "c", Level::Info),
        ];
        let filter = SubscribeFilter {
            since: Some(SinceCursor { index: Some(0), ..Default::default() }),
            ..Default::default()
        };
        let result = apply_filtered_index(&events, &filter);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filtered_index, 1);
    }
}
```

**Step 2: Run tests to verify failure**

Run: `cd rust && cargo test -p taskcast-core filter`
Expected: FAIL

**Step 3: Write implementation**

```rust
// rust/taskcast-core/src/filter.rs (top, before tests)
use crate::types::*;

/// Internal filter struct matching TS SubscribeFilter but with Rust naming
/// for internal use. The HTTP layer maps query params to this.
#[derive(Debug, Clone, Default)]
pub struct SubscribeFilter {
    pub since: Option<SinceCursor>,
    pub types: Option<Vec<String>>,
    pub levels: Option<Vec<Level>>,
    pub include_status: Option<bool>,
    pub wrap: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct FilteredEvent {
    pub filtered_index: u64,
    pub raw_index: u64,
    pub event: TaskEvent,
}

pub fn matches_type(event_type: &str, patterns: Option<&Vec<String>>) -> bool {
    let patterns = match patterns {
        None => return true,
        Some(p) => p,
    };
    if patterns.is_empty() {
        return false;
    }
    patterns.iter().any(|pattern| {
        if pattern == "*" {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix(".*") {
            return event_type.starts_with(&format!("{prefix}."));
        }
        event_type == pattern
    })
}

pub fn matches_filter(event: &TaskEvent, filter: &SubscribeFilter) -> bool {
    let include_status = filter.include_status.unwrap_or(true);

    if !include_status && event.r#type == "taskcast:status" {
        return false;
    }

    if let Some(ref types) = filter.types {
        if !matches_type(&event.r#type, Some(types)) {
            return false;
        }
    }

    if let Some(ref levels) = filter.levels {
        if !levels.contains(&event.level) {
            return false;
        }
    }

    true
}

pub fn apply_filtered_index(events: &[TaskEvent], filter: &SubscribeFilter) -> Vec<FilteredEvent> {
    let since = filter.since.as_ref();
    let mut filtered_counter: u64 = 0;
    let mut result = Vec::new();

    for event in events {
        if !matches_filter(event, filter) {
            continue;
        }

        let current = filtered_counter;
        filtered_counter += 1;

        // since.index: skip events where filteredIndex <= since.index
        if let Some(since) = since {
            if let Some(since_idx) = since.index {
                if current <= since_idx {
                    continue;
                }
            }
        }

        result.push(FilteredEvent {
            filtered_index: current,
            raw_index: event.index,
            event: event.clone(),
        });
    }

    result
}
```

**Step 4: Update lib.rs**

Add `pub mod filter;` and re-export.

**Step 5: Run tests**

Run: `cd rust && cargo test -p taskcast-core filter`
Expected: all PASS

**Step 6: Commit**

```bash
git add rust/taskcast-core/
git commit -m "feat(rust): add event filter with type wildcards and since cursor"
```

---

## Task 4: Series Processing

**Files:**
- Create: `rust/taskcast-core/src/series.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

Reference: `packages/core/src/series.ts`

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn make_series_event(series_id: &str, mode: SeriesMode, data: serde_json::Value) -> TaskEvent {
        TaskEvent {
            id: ulid::Ulid::new().to_string(),
            task_id: "task-1".into(),
            index: 0,
            timestamp: 1000,
            r#type: "llm.delta".into(),
            level: Level::Info,
            data,
            series_id: Some(series_id.into()),
            series_mode: Some(mode),
        }
    }

    #[tokio::test]
    async fn keep_all_returns_unchanged() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event("s1", SeriesMode::KeepAll, serde_json::json!({"text": "hello"}));
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.data, event.data);
    }

    #[tokio::test]
    async fn accumulate_concatenates_text() {
        let store = MemoryShortTermStore::new();
        let e1 = make_series_event("s1", SeriesMode::Accumulate, serde_json::json!({"text": "hello"}));
        let r1 = process_series(e1, &store).await.unwrap();
        assert_eq!(r1.data["text"], "hello");

        let mut e2 = make_series_event("s1", SeriesMode::Accumulate, serde_json::json!({"text": " world"}));
        e2.index = 1;
        let r2 = process_series(e2, &store).await.unwrap();
        assert_eq!(r2.data["text"], "hello world");
    }

    #[tokio::test]
    async fn no_series_returns_unchanged() {
        let store = MemoryShortTermStore::new();
        let event = TaskEvent {
            id: "e1".into(),
            task_id: "task-1".into(),
            index: 0,
            timestamp: 1000,
            r#type: "basic".into(),
            level: Level::Info,
            data: serde_json::json!({"key": "value"}),
            series_id: None,
            series_mode: None,
        };
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.data, event.data);
    }
}
```

**Step 2: Run tests to verify failure**

Run: `cd rust && cargo test -p taskcast-core series`
Expected: FAIL

**Step 3: Write implementation**

```rust
// rust/taskcast-core/src/series.rs
use crate::adapters::ShortTermStore;
use crate::types::*;

pub async fn process_series(
    event: TaskEvent,
    store: &dyn ShortTermStore,
) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
    let (series_id, series_mode) = match (&event.series_id, &event.series_mode) {
        (Some(id), Some(mode)) => (id.clone(), *mode),
        _ => return Ok(event),
    };

    match series_mode {
        SeriesMode::KeepAll => Ok(event),
        SeriesMode::Accumulate => {
            let prev = store.get_series_latest(&event.task_id, &series_id).await?;
            let merged = if let Some(prev) = prev {
                let prev_text = prev.data.get("text").and_then(|v| v.as_str());
                let new_text = event.data.get("text").and_then(|v| v.as_str());
                if let (Some(pt), Some(nt)) = (prev_text, new_text) {
                    let mut new_data = event.data.clone();
                    if let Some(obj) = new_data.as_object_mut() {
                        obj.insert("text".into(), serde_json::Value::String(format!("{pt}{nt}")));
                    }
                    TaskEvent { data: new_data, ..event }
                } else {
                    event
                }
            } else {
                event
            };
            store.set_series_latest(&merged.task_id, &series_id, &merged).await?;
            Ok(merged)
        }
        SeriesMode::Latest => {
            store.replace_last_series_event(&event.task_id, &series_id, &event).await?;
            Ok(event)
        }
    }
}
```

**Step 4: Run tests**

Run: `cd rust && cargo test -p taskcast-core series`
Expected: all PASS

**Step 5: Commit**

```bash
git add rust/taskcast-core/
git commit -m "feat(rust): add series processing (keep-all, accumulate, latest)"
```

---

## Task 5: Storage Adapter Traits + Memory Adapters

**Files:**
- Create: `rust/taskcast-core/src/adapters.rs`
- Create: `rust/taskcast-core/src/memory_adapters.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

Reference: `packages/core/src/types.ts` (interfaces), `packages/core/src/memory-adapters.ts`

**Step 1: Write adapter traits**

```rust
// rust/taskcast-core/src/adapters.rs
use async_trait::async_trait;
use crate::types::*;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait BroadcastProvider: Send + Sync {
    async fn publish(&self, channel: &str, event: &TaskEvent) -> Result<(), BoxError>;
    fn subscribe(&self, channel: &str, handler: Box<dyn Fn(TaskEvent) + Send + Sync>) -> Box<dyn FnOnce() + Send>;
}

#[async_trait]
pub trait ShortTermStore: Send + Sync {
    async fn save_task(&self, task: &Task) -> Result<(), BoxError>;
    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, BoxError>;
    async fn append_event(&self, task_id: &str, event: &TaskEvent) -> Result<(), BoxError>;
    async fn get_events(&self, task_id: &str, opts: Option<&EventQueryOptions>) -> Result<Vec<TaskEvent>, BoxError>;
    async fn set_ttl(&self, task_id: &str, ttl_seconds: u64) -> Result<(), BoxError>;
    async fn get_series_latest(&self, task_id: &str, series_id: &str) -> Result<Option<TaskEvent>, BoxError>;
    async fn set_series_latest(&self, task_id: &str, series_id: &str, event: &TaskEvent) -> Result<(), BoxError>;
    async fn replace_last_series_event(&self, task_id: &str, series_id: &str, event: &TaskEvent) -> Result<(), BoxError>;
}

#[async_trait]
pub trait LongTermStore: Send + Sync {
    async fn save_task(&self, task: &Task) -> Result<(), BoxError>;
    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, BoxError>;
    async fn save_event(&self, event: &TaskEvent) -> Result<(), BoxError>;
    async fn get_events(&self, task_id: &str, opts: Option<&EventQueryOptions>) -> Result<Vec<TaskEvent>, BoxError>;
}
```

**Step 2: Write MemoryBroadcastProvider and MemoryShortTermStore with tests**

Full implementation mirroring `packages/core/src/memory-adapters.ts`, using `Arc<RwLock<...>>` for interior mutability.

```rust
// rust/taskcast-core/src/memory_adapters.rs
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use async_trait::async_trait;
use crate::adapters::*;
use crate::types::*;

pub struct MemoryBroadcastProvider {
    listeners: Arc<RwLock<HashMap<String, Vec<Arc<dyn Fn(TaskEvent) + Send + Sync>>>>>,
}

impl MemoryBroadcastProvider {
    pub fn new() -> Self {
        Self { listeners: Arc::new(RwLock::new(HashMap::new())) }
    }
}

#[async_trait]
impl BroadcastProvider for MemoryBroadcastProvider {
    async fn publish(&self, channel: &str, event: &TaskEvent) -> Result<(), BoxError> {
        let listeners = self.listeners.read().unwrap();
        if let Some(handlers) = listeners.get(channel) {
            for handler in handlers {
                handler(event.clone());
            }
        }
        Ok(())
    }

    fn subscribe(&self, channel: &str, handler: Box<dyn Fn(TaskEvent) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        let handler = Arc::from(handler);
        let ptr = Arc::as_ptr(&handler) as usize;
        {
            let mut listeners = self.listeners.write().unwrap();
            listeners.entry(channel.to_string()).or_default().push(handler);
        }
        let listeners = Arc::clone(&self.listeners);
        let channel = channel.to_string();
        Box::new(move || {
            let mut listeners = listeners.write().unwrap();
            if let Some(handlers) = listeners.get_mut(&channel) {
                handlers.retain(|h| Arc::as_ptr(h) as usize != ptr);
            }
        })
    }
}

pub struct MemoryShortTermStore {
    tasks: RwLock<HashMap<String, Task>>,
    events: RwLock<HashMap<String, Vec<TaskEvent>>>,
    series_latest: RwLock<HashMap<String, TaskEvent>>,
}

impl MemoryShortTermStore {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            series_latest: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ShortTermStore for MemoryShortTermStore {
    async fn save_task(&self, task: &Task) -> Result<(), BoxError> {
        self.tasks.write().unwrap().insert(task.id.clone(), task.clone());
        Ok(())
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<Task>, BoxError> {
        Ok(self.tasks.read().unwrap().get(task_id).cloned())
    }

    async fn append_event(&self, task_id: &str, event: &TaskEvent) -> Result<(), BoxError> {
        self.events.write().unwrap()
            .entry(task_id.to_string()).or_default()
            .push(event.clone());
        Ok(())
    }

    async fn get_events(&self, task_id: &str, opts: Option<&EventQueryOptions>) -> Result<Vec<TaskEvent>, BoxError> {
        let events = self.events.read().unwrap();
        let all = match events.get(task_id) {
            Some(v) => v.clone(),
            None => return Ok(vec![]),
        };

        let mut result = all;
        if let Some(opts) = opts {
            if let Some(ref since) = opts.since {
                if let Some(ref id) = since.id {
                    if let Some(idx) = result.iter().position(|e| &e.id == id) {
                        result = result[idx + 1..].to_vec();
                    }
                } else if let Some(index) = since.index {
                    result.retain(|e| e.index > index);
                } else if let Some(ts) = since.timestamp {
                    result.retain(|e| e.timestamp > ts);
                }
            }
            if let Some(limit) = opts.limit {
                result.truncate(limit);
            }
        }
        Ok(result)
    }

    async fn set_ttl(&self, _task_id: &str, _ttl_seconds: u64) -> Result<(), BoxError> {
        Ok(()) // no-op in memory
    }

    async fn get_series_latest(&self, task_id: &str, series_id: &str) -> Result<Option<TaskEvent>, BoxError> {
        let key = format!("{task_id}:{series_id}");
        Ok(self.series_latest.read().unwrap().get(&key).cloned())
    }

    async fn set_series_latest(&self, task_id: &str, series_id: &str, event: &TaskEvent) -> Result<(), BoxError> {
        let key = format!("{task_id}:{series_id}");
        self.series_latest.write().unwrap().insert(key, event.clone());
        Ok(())
    }

    async fn replace_last_series_event(&self, task_id: &str, series_id: &str, event: &TaskEvent) -> Result<(), BoxError> {
        let key = format!("{task_id}:{series_id}");
        let prev = self.series_latest.read().unwrap().get(&key).cloned();
        if let Some(prev) = prev {
            let mut events = self.events.write().unwrap();
            if let Some(task_events) = events.get_mut(task_id) {
                if let Some(idx) = task_events.iter().rposition(|e| e.id == prev.id) {
                    task_events[idx] = event.clone();
                }
            }
        } else {
            self.append_event(task_id, event).await?;
        }
        self.series_latest.write().unwrap().insert(key, event.clone());
        Ok(())
    }
}
```

**Step 3: Run tests**

Run: `cd rust && cargo test -p taskcast-core`
Expected: all PASS

**Step 4: Commit**

```bash
git add rust/taskcast-core/
git commit -m "feat(rust): add adapter traits and memory adapter implementations"
```

---

## Task 6: Task Engine

**Files:**
- Create: `rust/taskcast-core/src/engine.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

Reference: `packages/core/src/engine.ts`

**Step 1: Write tests**

Tests covering: create task, get task, transition task, publish event, terminal status guard, index counters.

**Step 2: Run tests to verify failure**

Run: `cd rust && cargo test -p taskcast-core engine`
Expected: FAIL

**Step 3: Write TaskEngine implementation**

```rust
// rust/taskcast-core/src/engine.rs
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::adapters::*;
use crate::state_machine::{can_transition, is_terminal};
use crate::series::process_series;
use crate::types::*;

pub struct CreateTaskInput {
    pub id: Option<String>,
    pub r#type: Option<String>,
    pub params: Option<HashMap<String, serde_json::Value>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub ttl: Option<u64>,
    pub webhooks: Option<Vec<WebhookConfig>>,
    pub cleanup: Option<CleanupConfig>,
    pub auth_config: Option<TaskAuthConfig>,
}

pub struct PublishEventInput {
    pub r#type: String,
    pub level: Level,
    pub data: serde_json::Value,
    pub series_id: Option<String>,
    pub series_mode: Option<SeriesMode>,
}

pub struct TransitionPayload {
    pub result: Option<HashMap<String, serde_json::Value>>,
    pub error: Option<TaskError>,
}

pub struct TaskEngineOptions {
    pub short_term: Arc<dyn ShortTermStore>,
    pub broadcast: Arc<dyn BroadcastProvider>,
    pub long_term: Option<Arc<dyn LongTermStore>>,
}

pub struct TaskEngine {
    short_term: Arc<dyn ShortTermStore>,
    broadcast: Arc<dyn BroadcastProvider>,
    long_term: Option<Arc<dyn LongTermStore>>,
    index_counters: RwLock<HashMap<String, u64>>,
}

impl TaskEngine {
    pub fn new(opts: TaskEngineOptions) -> Self {
        Self {
            short_term: opts.short_term,
            broadcast: opts.broadcast,
            long_term: opts.long_term,
            index_counters: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_task(&self, input: CreateTaskInput) -> Result<Task, BoxError> {
        let now = now_ms();
        let task = Task {
            id: input.id.unwrap_or_else(|| ulid::Ulid::new().to_string()),
            r#type: input.r#type,
            status: TaskStatus::Pending,
            params: input.params,
            result: None,
            error: None,
            metadata: input.metadata,
            created_at: now,
            updated_at: now,
            completed_at: None,
            ttl: input.ttl,
            auth_config: input.auth_config,
            webhooks: input.webhooks,
            cleanup: input.cleanup,
        };
        self.short_term.save_task(&task).await?;
        if let Some(ref lt) = self.long_term {
            lt.save_task(&task).await?;
        }
        if let Some(ttl) = task.ttl {
            self.short_term.set_ttl(&task.id, ttl).await?;
        }
        Ok(task)
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Option<Task>, BoxError> {
        if let Some(task) = self.short_term.get_task(task_id).await? {
            return Ok(Some(task));
        }
        if let Some(ref lt) = self.long_term {
            return lt.get_task(task_id).await;
        }
        Ok(None)
    }

    pub async fn transition_task(
        &self,
        task_id: &str,
        to: TaskStatus,
        payload: Option<TransitionPayload>,
    ) -> Result<Task, BoxError> {
        let task = self.get_task(task_id).await?
            .ok_or_else(|| format!("Task not found: {task_id}"))?;

        if !can_transition(task.status, to) {
            return Err(format!("Invalid transition: {:?} → {:?}", task.status, to).into());
        }

        let now = now_ms();
        let new_result = payload.as_ref().and_then(|p| p.result.clone()).or(task.result);
        let new_error = payload.as_ref().and_then(|p| p.error.clone()).or(task.error);
        let completed_at = if is_terminal(to) { Some(now) } else { task.completed_at };

        let updated = Task {
            status: to,
            updated_at: now,
            completed_at,
            result: new_result,
            error: new_error,
            ..task
        };

        self.short_term.save_task(&updated).await?;
        if let Some(ref lt) = self.long_term {
            lt.save_task(&updated).await?;
        }

        self.emit(task_id, PublishEventInput {
            r#type: "taskcast:status".into(),
            level: Level::Info,
            data: serde_json::json!({
                "status": to,
                "result": updated.result,
                "error": updated.error,
            }),
            series_id: None,
            series_mode: None,
        }).await?;

        Ok(updated)
    }

    pub async fn publish_event(&self, task_id: &str, input: PublishEventInput) -> Result<TaskEvent, BoxError> {
        let task = self.get_task(task_id).await?
            .ok_or_else(|| format!("Task not found: {task_id}"))?;
        if is_terminal(task.status) {
            return Err(format!("Cannot publish to task in terminal status: {:?}", task.status).into());
        }
        self.emit(task_id, input).await
    }

    pub async fn get_events(&self, task_id: &str, opts: Option<&EventQueryOptions>) -> Result<Vec<TaskEvent>, BoxError> {
        self.short_term.get_events(task_id, opts).await
    }

    pub fn subscribe(&self, task_id: &str, handler: Box<dyn Fn(TaskEvent) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.broadcast.subscribe(task_id, handler)
    }

    async fn emit(&self, task_id: &str, input: PublishEventInput) -> Result<TaskEvent, BoxError> {
        let index = self.next_index(task_id);
        let raw = TaskEvent {
            id: ulid::Ulid::new().to_string(),
            task_id: task_id.to_string(),
            index,
            timestamp: now_ms(),
            r#type: input.r#type,
            level: input.level,
            data: input.data,
            series_id: input.series_id,
            series_mode: input.series_mode,
        };

        let event = process_series(raw, self.short_term.as_ref()).await?;
        self.short_term.append_event(task_id, &event).await?;
        self.broadcast.publish(task_id, &event).await?;

        if let Some(ref lt) = self.long_term {
            let lt = Arc::clone(lt);
            let event_clone = event.clone();
            tokio::spawn(async move {
                let _ = lt.save_event(&event_clone).await;
            });
        }

        Ok(event)
    }

    fn next_index(&self, task_id: &str) -> u64 {
        let mut counters = self.index_counters.write().unwrap();
        let counter = counters.entry(task_id.to_string()).or_insert(0);
        let idx = *counter;
        *counter += 1;
        idx
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
```

**Step 4: Run tests**

Run: `cd rust && cargo test -p taskcast-core engine`
Expected: all PASS

**Step 5: Commit**

```bash
git add rust/taskcast-core/
git commit -m "feat(rust): add TaskEngine with full lifecycle management"
```

---

## Task 7: Cleanup Module

**Files:**
- Create: `rust/taskcast-core/src/cleanup.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

Reference: `packages/core/src/cleanup.ts`

Direct port of `matchesCleanupRule` and `filterEventsForCleanup` with tests. Straightforward — follows the same pattern as filter.rs.

**Step 1-5:** Write tests → verify fail → implement → verify pass → commit

```bash
git commit -m "feat(rust): add cleanup rule matching and event filtering"
```

---

## Task 8: Config Module

**Files:**
- Create: `rust/taskcast-core/src/config.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

Reference: `packages/core/src/config.ts`

Implement:
- `TaskcastConfig` struct matching TS interface
- `interpolate_env_vars(value: &str) -> String` — replaces `${VAR}` patterns
- `parse_config(content: &str, format: ConfigFormat) -> Result<TaskcastConfig>`
- `load_config_file(config_path: Option<&str>) -> Result<TaskcastConfig>`

Note: Rust version only needs to handle YAML and JSON (no TS/JS config files — those are Node-only). Use `serde_yaml` and `serde_json`.

**Step 1-5:** Write tests → verify fail → implement → verify pass → commit

```bash
git commit -m "feat(rust): add config parsing with YAML/JSON and env var interpolation"
```

---

## Task 9: Axum Server — App Skeleton + Auth Middleware

**Files:**
- Create: `rust/taskcast-server/Cargo.toml`
- Create: `rust/taskcast-server/src/lib.rs`
- Create: `rust/taskcast-server/src/app.rs`
- Create: `rust/taskcast-server/src/auth.rs`
- Create: `rust/taskcast-server/src/error.rs`

Reference: `packages/server/src/index.ts`, `packages/server/src/auth.ts`

**Step 1: Create taskcast-server Cargo.toml**

```toml
[package]
name = "taskcast-server"
version = "0.1.0"
edition = "2021"

[dependencies]
taskcast-core = { path = "../taskcast-core" }
axum = { version = "0.8", features = ["macros"] }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors"] }
jsonwebtoken = "9"
http = "1"
```

**Step 2: Write auth.rs**

```rust
// rust/taskcast-server/src/auth.rs
use axum::{extract::Request, middleware::Next, response::Response};
use axum::http::StatusCode;
use axum::Json;
use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
use serde::{Deserialize, Serialize};
use taskcast_core::types::PermissionScope;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum AuthMode {
    None,
    Jwt(JwtConfig),
}

#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub algorithm: Algorithm,
    pub secret: Option<String>,
    pub public_key: Option<String>,
    pub issuer: Option<String>,
    pub audience: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub sub: Option<String>,
    pub task_ids: TaskIdAccess,
    pub scope: Vec<PermissionScope>,
}

#[derive(Debug, Clone)]
pub enum TaskIdAccess {
    All,
    List(Vec<String>),
}

impl AuthContext {
    pub fn open() -> Self {
        Self {
            sub: None,
            task_ids: TaskIdAccess::All,
            scope: vec![PermissionScope::All],
        }
    }
}

pub fn check_scope(auth: &AuthContext, required: PermissionScope, task_id: Option<&str>) -> bool {
    if let Some(tid) = task_id {
        match &auth.task_ids {
            TaskIdAccess::All => {}
            TaskIdAccess::List(ids) => {
                if !ids.iter().any(|id| id == tid) {
                    return false;
                }
            }
        }
    }
    auth.scope.contains(&PermissionScope::All) || auth.scope.contains(&required)
}

pub async fn auth_middleware(
    axum::extract::State(auth_mode): axum::extract::State<Arc<AuthMode>>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_context = match auth_mode.as_ref() {
        AuthMode::None => AuthContext::open(),
        AuthMode::Jwt(jwt_config) => {
            // Extract Bearer token, decode JWT, build AuthContext
            // On failure return 401
            match extract_jwt_context(&req, jwt_config) {
                Ok(ctx) => ctx,
                Err(resp) => return resp,
            }
        }
    };
    req.extensions_mut().insert(auth_context);
    next.run(req).await
}
```

**Step 3: Write app.rs — Router assembly**

```rust
// rust/taskcast-server/src/app.rs
use axum::{Router, middleware};
use std::sync::Arc;
use taskcast_core::engine::TaskEngine;
use crate::auth::{auth_middleware, AuthMode};
use crate::routes::{tasks, sse};

pub struct AppState {
    pub engine: Arc<TaskEngine>,
    pub auth_mode: Arc<AuthMode>,
}

pub fn create_app(engine: Arc<TaskEngine>, auth_mode: AuthMode) -> Router {
    let auth = Arc::new(auth_mode);
    let state = Arc::new(AppState {
        engine: Arc::clone(&engine),
        auth_mode: Arc::clone(&auth),
    });

    Router::new()
        .merge(tasks::router())
        .merge(sse::router())
        .layer(middleware::from_fn_with_state(auth, auth_middleware))
        .with_state(state)
}
```

**Step 4: Run `cargo check -p taskcast-server`**

Expected: compiles (routes module stubbed)

**Step 5: Commit**

```bash
git add rust/taskcast-server/
git commit -m "feat(rust): add Axum server skeleton with JWT auth middleware"
```

---

## Task 10: REST API Routes

**Files:**
- Create: `rust/taskcast-server/src/routes/mod.rs`
- Create: `rust/taskcast-server/src/routes/tasks.rs`

Reference: `packages/server/src/routes/tasks.ts`

Implement all 5 endpoints:
- `POST /tasks` — create task (requires `task:create`)
- `GET /tasks/:taskId` — get task (requires `event:subscribe`)
- `PATCH /tasks/:taskId/status` — transition (requires `task:manage`)
- `POST /tasks/:taskId/events` — publish event(s) (requires `event:publish`)
- `GET /tasks/:taskId/events/history` — query history (requires `event:history`)

Use `axum::extract::{Path, Query, Json, Extension}` for parameter extraction. Validate with serde deserialization (Axum rejects invalid payloads automatically).

**Step 1: Write tests using axum::test**

Test each endpoint with proper auth context injection.

**Step 2-5:** Implement → verify → commit

```bash
git commit -m "feat(rust): add REST API routes (create, get, transition, publish, history)"
```

---

## Task 11: SSE Streaming Route

**Files:**
- Create: `rust/taskcast-server/src/routes/sse.rs`

Reference: `packages/server/src/routes/sse.ts`

**Key implementation:**
- Use `axum::response::sse::Sse` with `tokio_stream`
- Parse query params into `SubscribeFilter`
- Replay history → subscribe live → merge into single `Stream`
- Send `taskcast.event` and `taskcast.done` SSE events
- Auto-close on terminal status or client disconnect

```rust
use axum::response::sse::{Event, Sse};
use tokio_stream::StreamExt;
use futures::stream::Stream;

async fn sse_handler(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(params): Query<SseQueryParams>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    // 1. Check auth scope
    // 2. Get task (404 if missing)
    // 3. Build filter from query params
    // 4. Replay history with filtered index
    // 5. If terminal, send done and return
    // 6. Subscribe to live events via broadcast
    // 7. Stream combined history + live
}
```

**Step 1-5:** Write tests → implement → verify → commit

```bash
git commit -m "feat(rust): add SSE streaming with history replay and live subscription"
```

---

## Task 12: Webhook Delivery

**Files:**
- Create: `rust/taskcast-server/src/webhook.rs`

Reference: `packages/server/src/webhook.ts`

Implement:
- HMAC-SHA256 signing (`hmac` + `sha2` crates)
- Exponential backoff retry (3 attempts, 1s-30s)
- Request timeout (5s via `reqwest`)
- Filter support
- Custom headers (X-Taskcast-Event, X-Taskcast-Timestamp, X-Taskcast-Signature)

**Step 1-5:** Write tests → implement → verify → commit

```bash
git commit -m "feat(rust): add webhook delivery with HMAC signing and retry"
```

---

## Task 13: PostgreSQL Adapter

**Files:**
- Create: `rust/taskcast-postgres/Cargo.toml`
- Create: `rust/taskcast-postgres/src/lib.rs`
- Create: `rust/taskcast-postgres/src/store.rs`
- Copy: `packages/postgres/migrations/001_initial.sql` → `rust/taskcast-postgres/migrations/001_initial.sql`

Reference: `packages/postgres/src/long-term.ts`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "taskcast-postgres"
version = "0.1.0"
edition = "2021"

[dependencies]
taskcast-core = { path = "../taskcast-core" }
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "json"] }
serde = { workspace = true }
serde_json = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["postgres"] }
```

**Step 2: Implement PostgresLongTermStore**

Port `saveTask`, `getTask`, `saveEvent`, `getEvents` using sqlx queries. Same SQL as the TS version. Use the same migration file.

**Step 3: Write tests with testcontainers**

**Step 4: Run and verify**

Run: `cd rust && cargo test -p taskcast-postgres`
Expected: all PASS (requires Docker)

**Step 5: Commit**

```bash
git add rust/taskcast-postgres/
git commit -m "feat(rust): add PostgreSQL long-term store adapter with sqlx"
```

---

## Task 14: Redis Adapters

**Files:**
- Create: `rust/taskcast-redis/Cargo.toml`
- Create: `rust/taskcast-redis/src/lib.rs`
- Create: `rust/taskcast-redis/src/broadcast.rs`
- Create: `rust/taskcast-redis/src/short_term.rs`

Reference: `packages/redis/src/broadcast.ts`, `packages/redis/src/short-term.ts`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "taskcast-redis"
version = "0.1.0"
edition = "2021"

[dependencies]
taskcast-core = { path = "../taskcast-core" }
redis = { version = "0.27", features = ["tokio-comp", "aio"] }
serde = { workspace = true }
serde_json = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["redis"] }
```

**Step 2: Implement RedisBroadcastProvider**

Port pub/sub pattern. Use `redis::aio::PubSub` for subscription, separate connections for pub and sub.

**Step 3: Implement RedisShortTermStore**

Port all methods: saveTask (SET JSON), getTask (GET), appendEvent (RPUSH), getEvents (LRANGE + filter), setTTL (EXPIRE), series operations.

**Step 4: Factory function**

```rust
pub fn create_redis_adapters(
    pub_client: redis::Client,
    sub_client: redis::Client,
    store_client: redis::Client,
    prefix: Option<&str>,
) -> (RedisBroadcastProvider, RedisShortTermStore) { ... }
```

**Step 5: Write tests with testcontainers, run, verify**

```bash
git commit -m "feat(rust): add Redis broadcast and short-term store adapters"
```

---

## Task 15: CLI Entry Point

**Files:**
- Create: `rust/taskcast-cli/Cargo.toml`
- Create: `rust/taskcast-cli/src/main.rs`

Reference: `packages/cli/src/index.ts`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "taskcast-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "taskcast"
path = "src/main.rs"

[dependencies]
taskcast-core = { path = "../taskcast-core" }
taskcast-server = { path = "../taskcast-server" }
taskcast-postgres = { path = "../taskcast-postgres" }
taskcast-redis = { path = "../taskcast-redis" }
clap = { version = "4", features = ["derive"] }
tokio = { workspace = true }
```

**Step 2: Implement main.rs**

```rust
use clap::{Parser, Subcommand};
use std::sync::Arc;
use taskcast_core::{
    config::load_config_file,
    engine::{TaskEngine, TaskEngineOptions},
    memory_adapters::{MemoryBroadcastProvider, MemoryShortTermStore},
};
use taskcast_server::{app::create_app, auth::AuthMode};

#[derive(Parser)]
#[command(name = "taskcast", about = "Taskcast — unified task tracking and streaming service")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the taskcast server (default)
    Start {
        #[arg(short, long)]
        config: Option<String>,
        #[arg(short, long, default_value = "3721")]
        port: u16,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let cmd = cli.command.unwrap_or(Commands::Start { config: None, port: 3721 });

    match cmd {
        Commands::Start { config, port } => {
            let file_config = load_config_file(config.as_deref()).unwrap_or_default();
            let port = file_config.port.unwrap_or(port);

            let redis_url = std::env::var("TASKCAST_REDIS_URL").ok()
                .or_else(|| file_config.adapters.as_ref()?.broadcast.as_ref()?.url.clone());
            let postgres_url = std::env::var("TASKCAST_POSTGRES_URL").ok()
                .or_else(|| file_config.adapters.as_ref()?.long_term.as_ref()?.url.clone());

            // Build adapters (Redis or memory fallback)
            let (broadcast, short_term) = if let Some(url) = redis_url {
                // ... create Redis adapters
                todo!("Redis adapter initialization")
            } else {
                eprintln!("[taskcast] No TASKCAST_REDIS_URL configured — using in-memory adapters");
                (
                    Arc::new(MemoryBroadcastProvider::new()) as Arc<dyn taskcast_core::adapters::BroadcastProvider>,
                    Arc::new(MemoryShortTermStore::new()) as Arc<dyn taskcast_core::adapters::ShortTermStore>,
                )
            };

            let long_term = if let Some(url) = postgres_url {
                // ... create Postgres adapter
                todo!("Postgres adapter initialization")
            } else {
                None
            };

            let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
                short_term,
                broadcast,
                long_term,
            }));

            let auth_mode_str = std::env::var("TASKCAST_AUTH_MODE").ok()
                .or_else(|| file_config.auth.as_ref().map(|a| a.mode.clone()))
                .unwrap_or_else(|| "none".into());

            let auth_mode = match auth_mode_str.as_str() {
                "none" => AuthMode::None,
                "jwt" => todo!("JWT config from file_config"),
                _ => AuthMode::None,
            };

            let app = create_app(engine, auth_mode);
            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
            println!("[taskcast] Server started on http://localhost:{port}");
            axum::serve(listener, app).await.unwrap();
        }
    }
}
```

**Step 3: Build**

Run: `cd rust && cargo build -p taskcast-cli`
Expected: binary compiles at `target/debug/taskcast`

**Step 4: Smoke test**

Run: `cd rust && cargo run -p taskcast-cli -- start --port 3722`
Expected: prints `[taskcast] Server started on http://localhost:3722`

**Step 5: Commit**

```bash
git add rust/taskcast-cli/
git commit -m "feat(rust): add CLI entry point with clap and server bootstrap"
```

---

## Task 16: Integration Testing with TS Client

**Files:**
- Create: `rust/tests/integration.sh` (helper script)
- Modify: Existing TS test files to support Rust server target

**Step 1: Build release binary**

Run: `cd rust && cargo build --release`

**Step 2: Run Rust server in background**

```bash
./target/release/taskcast start --port 3799 &
RUST_PID=$!
```

**Step 3: Run TS integration tests against Rust server**

Set `TASKCAST_BASE_URL=http://localhost:3799` and run existing server-sdk tests.

**Step 4: Compare results with TS server**

Ensure all tests pass identically.

**Step 5: Kill Rust server and commit**

```bash
kill $RUST_PID
git commit -m "test(rust): verify API compatibility via TS integration tests"
```

---

## Task 17: CI Configuration

**Files:**
- Modify: existing CI config to add Rust build + test job

**Step 1: Add Rust toolchain + cargo test step**
**Step 2: Add release binary build step**
**Step 3: Commit**

```bash
git commit -m "ci: add Rust build and test pipeline"
```

---

## Summary

| Task | What | Estimated Complexity |
|------|------|---------------------|
| 1 | Cargo workspace + core types | Low |
| 2 | State machine | Low |
| 3 | Filter module | Low |
| 4 | Series processing | Medium |
| 5 | Adapter traits + memory adapters | Medium |
| 6 | Task engine | Medium-High |
| 7 | Cleanup module | Low |
| 8 | Config module | Low |
| 9 | Axum server + auth middleware | Medium |
| 10 | REST API routes | Medium |
| 11 | SSE streaming | High |
| 12 | Webhook delivery | Medium |
| 13 | PostgreSQL adapter | Medium |
| 14 | Redis adapters | Medium-High |
| 15 | CLI entry point | Low |
| 16 | Integration testing | Medium |
| 17 | CI configuration | Low |

**Total: 17 tasks, ~2000 LOC Rust**
