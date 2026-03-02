# Rust Worker Assignment Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Port the TypeScript worker assignment feature to Rust, maintaining identical HTTP behavior — same paths, same JSON format, same WebSocket protocol, same status codes.

**Architecture:** Extend `taskcast-core` with new types, `Assigned` status, worker matching logic, and `WorkerManager`. Extend `taskcast-server` with worker REST routes and WebSocket handler. Extend Redis/Postgres/Memory adapters with worker CRUD, atomic claim, and audit storage. All new Rust code mirrors the TypeScript implementation in `packages/core` and `packages/server` on the `worktree-worker-assignment` branch.

**Tech Stack:** Rust, Axum 0.8, Tokio, sqlx (Postgres), redis-rs, serde, async-trait, tokio-tungstenite (WebSocket)

**Design Doc:** `docs/plans/2026-03-02-worker-assignment-design.md`

**TypeScript Reference:** `packages/core/src/worker-manager.ts`, `packages/core/src/worker-matching.ts`, `packages/server/src/routes/workers.ts`, `packages/server/src/routes/worker-ws.ts`

---

## Phase 1: Core Type Extensions & State Machine

### Task 1.1: Add worker types to `types.rs`

**Files:**
- Modify: `rust/taskcast-core/src/types.rs`

**Step 1: Add new enums after existing `PermissionScope`**

Add `WorkerConnect` and `WorkerManage` variants to the existing `PermissionScope` enum:

```rust
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
    #[serde(rename = "worker:connect")]   // NEW
    WorkerConnect,
    #[serde(rename = "worker:manage")]    // NEW
    WorkerManage,
    #[serde(rename = "*")]
    All,
}
```

**Step 2: Add `Assigned` to `TaskStatus`**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Pending,
    Assigned,  // NEW
    Running,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}
```

**Step 3: Add worker-related types**

After the existing `CleanupRule` struct, add:

```rust
// ─── Worker Assignment ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssignMode {
    External,
    Pull,
    WsOffer,
    WsRace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DisconnectPolicy {
    Reassign,
    Mark,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerStatus {
    Idle,
    Busy,
    Draining,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TagMatcher {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub none: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkerMatchRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<TagMatcher>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worker {
    pub id: String,
    pub status: WorkerStatus,
    pub match_rule: WorkerMatchRule,
    pub capacity: u32,
    pub used_slots: u32,
    pub weight: u32,
    pub connection_mode: ConnectionMode,
    pub connected_at: f64,
    pub last_heartbeat_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionMode {
    Pull,
    Websocket,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerAssignmentStatus {
    Offered,
    Assigned,
    Running,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerAssignment {
    pub task_id: String,
    pub worker_id: String,
    pub cost: u32,
    pub assigned_at: f64,
    pub status: WorkerAssignmentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerAuditEvent {
    pub id: String,
    pub worker_id: String,
    pub timestamp: f64,
    pub action: WorkerAuditAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerAuditAction {
    Connected,
    Disconnected,
    Updated,
    TaskAssigned,
    TaskDeclined,
    TaskReclaimed,
    Draining,
    HeartbeatTimeout,
    PullRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkerFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkerStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_mode: Option<ConnectionMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assign_mode: Option<AssignMode>,
}
```

**Step 4: Extend `Task` struct**

Add new optional fields to the existing `Task` struct:

```rust
pub struct Task {
    // ...existing fields...

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assign_mode: Option<AssignMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_worker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disconnect_policy: Option<DisconnectPolicy>,
}
```

**Step 5: Extend trait interfaces**

Add to `ShortTermStore`:

```rust
#[async_trait]
pub trait ShortTermStore: Send + Sync {
    // ...existing methods...

    // Task query
    async fn list_tasks(&self, filter: TaskFilter) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>>;

    // Worker state
    async fn save_worker(&self, worker: Worker) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_worker(&self, worker_id: &str) -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>>;
    async fn list_workers(&self, filter: Option<WorkerFilter>) -> Result<Vec<Worker>, Box<dyn std::error::Error + Send + Sync>>;
    async fn delete_worker(&self, worker_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    // Atomic claim
    async fn claim_task(&self, task_id: &str, worker_id: &str, cost: u32) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;

    // Worker assignments
    async fn add_assignment(&self, assignment: WorkerAssignment) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn remove_assignment(&self, task_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_worker_assignments(&self, worker_id: &str) -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>>;
    async fn get_task_assignment(&self, task_id: &str) -> Result<Option<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>>;
}
```

Add to `LongTermStore`:

```rust
#[async_trait]
pub trait LongTermStore: Send + Sync {
    // ...existing methods...

    async fn save_worker_event(&self, event: WorkerAuditEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn get_worker_events(&self, worker_id: &str, opts: Option<EventQueryOptions>) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>>;
}
```

Add to `TaskcastHooks`:

```rust
pub trait TaskcastHooks: Send + Sync {
    // ...existing methods...

    fn on_task_created(&self, _task: &Task) {}
    fn on_task_transitioned(&self, _task: &Task, _from: &TaskStatus, _to: &TaskStatus) {}
    fn on_worker_connected(&self, _worker: &Worker) {}
    fn on_worker_disconnected(&self, _worker: &Worker, _reason: &str) {}
    fn on_task_assigned(&self, _task: &Task, _worker: &Worker) {}
    fn on_task_declined(&self, _task: &Task, _worker: &Worker, _blacklisted: bool) {}
}
```

**Step 6: Extend `CreateTaskInput`**

```rust
pub struct CreateTaskInput {
    // ...existing fields...

    pub tags: Option<Vec<String>>,
    pub assign_mode: Option<AssignMode>,
    pub cost: Option<u32>,
    pub disconnect_policy: Option<DisconnectPolicy>,
}
```

**Step 7: Run `cargo check` in `rust/` to verify compilation**

Run: `cd rust && cargo check 2>&1`
Expected: Compilation errors in adapters (unimplemented trait methods) — this is expected, we fix them in later tasks.

**Step 8: Commit**

```bash
git add rust/taskcast-core/src/types.rs
git commit -m "feat(rust/core): add worker assignment types, interfaces, and assigned task status"
```

---

### Task 1.2: Update state machine

**Files:**
- Modify: `rust/taskcast-core/src/state_machine.rs`
- Test: `rust/taskcast-core/src/state_machine.rs` (inline tests)

**Step 1: Write failing tests**

Add tests to the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_pending_to_assigned_is_valid() {
    assert!(can_transition(&TaskStatus::Pending, &TaskStatus::Assigned));
}

#[test]
fn test_assigned_to_running_is_valid() {
    assert!(can_transition(&TaskStatus::Assigned, &TaskStatus::Running));
}

#[test]
fn test_assigned_to_pending_is_valid() {
    assert!(can_transition(&TaskStatus::Assigned, &TaskStatus::Pending));
}

#[test]
fn test_assigned_to_cancelled_is_valid() {
    assert!(can_transition(&TaskStatus::Assigned, &TaskStatus::Cancelled));
}

#[test]
fn test_assigned_to_completed_is_invalid() {
    assert!(!can_transition(&TaskStatus::Assigned, &TaskStatus::Completed));
}

#[test]
fn test_assigned_to_failed_is_invalid() {
    assert!(!can_transition(&TaskStatus::Assigned, &TaskStatus::Failed));
}

#[test]
fn test_assigned_is_not_terminal() {
    assert!(!is_terminal(&TaskStatus::Assigned));
}
```

**Step 2: Run tests to verify they fail**

Run: `cd rust && cargo test -p taskcast-core -- state_machine 2>&1`
Expected: FAIL — `Assigned` not handled in `allowed_transitions`

**Step 3: Update `allowed_transitions`**

```rust
pub fn allowed_transitions(from: &TaskStatus) -> &'static [TaskStatus] {
    match from {
        TaskStatus::Pending => &[TaskStatus::Assigned, TaskStatus::Running, TaskStatus::Cancelled],
        TaskStatus::Assigned => &[TaskStatus::Running, TaskStatus::Pending, TaskStatus::Cancelled],
        TaskStatus::Running => &[
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Timeout,
            TaskStatus::Cancelled,
        ],
        TaskStatus::Completed
        | TaskStatus::Failed
        | TaskStatus::Timeout
        | TaskStatus::Cancelled => &[],
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd rust && cargo test -p taskcast-core -- state_machine 2>&1`
Expected: All PASS

**Step 5: Commit**

```bash
git add rust/taskcast-core/src/state_machine.rs
git commit -m "feat(rust/core): add assigned status to state machine transitions"
```

---

### Task 1.3: Extend engine with worker fields and hooks

**Files:**
- Modify: `rust/taskcast-core/src/engine.rs`

**Step 1: Update `create_task` to pass new fields**

In the `create_task` method, after setting existing fields on the `Task` struct, add:

```rust
tags: input.tags,
assign_mode: input.assign_mode,
cost: input.cost,
assigned_worker: None,
disconnect_policy: input.disconnect_policy,
```

After saving task, call hook:

```rust
if let Some(hooks) = &self.hooks {
    hooks.on_task_created(&task);
}
```

**Step 2: Update `transition_task` to call hook**

After successful transition, add:

```rust
if let Some(hooks) = &self.hooks {
    hooks.on_task_transitioned(&task, &current_status, &to);
}
```

(Where `current_status` is captured before the transition.)

**Step 3: Add `list_tasks` method to engine**

```rust
pub async fn list_tasks(&self, filter: TaskFilter) -> Result<Vec<Task>, EngineError> {
    Ok(self.short_term.list_tasks(filter).await?)
}
```

**Step 4: Run `cargo check`**

Run: `cd rust && cargo check 2>&1`
Expected: Still errors from unimplemented adapter methods — expected.

**Step 5: Commit**

```bash
git add rust/taskcast-core/src/engine.rs
git commit -m "feat(rust/core): extend TaskEngine with worker fields and hooks"
```

---

## Phase 2: Worker Matching & Manager (Core Logic)

### Task 2.1: Create worker matching module

**Files:**
- Create: `rust/taskcast-core/src/worker_matching.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

**Step 1: Write the module with tests**

Create `rust/taskcast-core/src/worker_matching.rs`:

```rust
use crate::types::{TagMatcher, Task, Worker, WorkerMatchRule};
use crate::filter::matches_type;

/// Check if task tags satisfy a TagMatcher (all/any/none).
pub fn matches_tag(task_tags: Option<&[String]>, matcher: &TagMatcher) -> bool {
    let tags = task_tags.unwrap_or(&[]);

    // all: every tag in matcher.all must exist in task tags
    if let Some(all) = &matcher.all {
        if !all.iter().all(|t| tags.contains(t)) {
            return false;
        }
    }

    // any: at least one tag in matcher.any must exist in task tags
    if let Some(any) = &matcher.any {
        if !any.iter().any(|t| tags.contains(t)) {
            return false;
        }
    }

    // none: none of matcher.none tags may exist in task tags
    if let Some(none) = &matcher.none {
        if none.iter().any(|t| tags.contains(t)) {
            return false;
        }
    }

    true
}

/// Check if a task matches a worker's match rule.
/// Both taskType matching (wildcard) and tag matching must pass.
pub fn matches_worker_rule(task: &Task, rule: &WorkerMatchRule) -> bool {
    // Check task type matching
    if let Some(task_types) = &rule.task_types {
        let task_type = task.r#type.as_deref().unwrap_or("");
        let type_patterns: Vec<String> = task_types.clone();
        if !matches_type(task_type, Some(&type_patterns)) {
            return false;
        }
    }

    // Check tag matching
    if let Some(tag_matcher) = &rule.tags {
        if !matches_tag(task.tags.as_deref(), tag_matcher) {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tag_matcher(all: Option<Vec<&str>>, any: Option<Vec<&str>>, none: Option<Vec<&str>>) -> TagMatcher {
        TagMatcher {
            all: all.map(|v| v.into_iter().map(String::from).collect()),
            any: any.map(|v| v.into_iter().map(String::from).collect()),
            none: none.map(|v| v.into_iter().map(String::from).collect()),
        }
    }

    fn tags(t: &[&str]) -> Option<Vec<String>> {
        Some(t.iter().map(|s| s.to_string()).collect())
    }

    // ── matches_tag tests ──

    #[test]
    fn empty_matcher_matches_anything() {
        let m = TagMatcher::default();
        assert!(matches_tag(None, &m));
        assert!(matches_tag(tags(&["a"]).as_deref(), &m));
    }

    #[test]
    fn all_requires_every_tag() {
        let m = make_tag_matcher(Some(vec!["a", "b"]), None, None);
        assert!(matches_tag(tags(&["a", "b", "c"]).as_deref(), &m));
        assert!(!matches_tag(tags(&["a"]).as_deref(), &m));
        assert!(!matches_tag(None, &m));
    }

    #[test]
    fn any_requires_at_least_one() {
        let m = make_tag_matcher(None, Some(vec!["a", "b"]), None);
        assert!(matches_tag(tags(&["a"]).as_deref(), &m));
        assert!(matches_tag(tags(&["b", "c"]).as_deref(), &m));
        assert!(!matches_tag(tags(&["c"]).as_deref(), &m));
        assert!(!matches_tag(None, &m));
    }

    #[test]
    fn none_rejects_matching_tags() {
        let m = make_tag_matcher(None, None, Some(vec!["x"]));
        assert!(matches_tag(tags(&["a"]).as_deref(), &m));
        assert!(matches_tag(None, &m));
        assert!(!matches_tag(tags(&["x"]).as_deref(), &m));
        assert!(!matches_tag(tags(&["a", "x"]).as_deref(), &m));
    }

    #[test]
    fn combined_all_any_none() {
        let m = make_tag_matcher(Some(vec!["a"]), Some(vec!["b", "c"]), Some(vec!["x"]));
        assert!(matches_tag(tags(&["a", "b"]).as_deref(), &m));
        assert!(!matches_tag(tags(&["a", "b", "x"]).as_deref(), &m)); // none fails
        assert!(!matches_tag(tags(&["b"]).as_deref(), &m)); // all fails
        assert!(!matches_tag(tags(&["a", "d"]).as_deref(), &m)); // any fails
    }

    // ── matches_worker_rule tests ──

    #[test]
    fn empty_rule_matches_any_task() {
        let rule = WorkerMatchRule::default();
        let task = make_task(None, None);
        assert!(matches_worker_rule(&task, &rule));
    }

    #[test]
    fn task_type_wildcard_match() {
        let rule = WorkerMatchRule {
            task_types: Some(vec!["llm.*".to_string()]),
            tags: None,
        };
        assert!(matches_worker_rule(&make_task(Some("llm.chat"), None), &rule));
        assert!(!matches_worker_rule(&make_task(Some("img.gen"), None), &rule));
    }

    #[test]
    fn rule_with_tags_and_type() {
        let rule = WorkerMatchRule {
            task_types: Some(vec!["llm.*".to_string()]),
            tags: Some(make_tag_matcher(Some(vec!["gpu"]), None, None)),
        };
        assert!(matches_worker_rule(
            &make_task(Some("llm.chat"), tags(&["gpu"]).as_deref().map(|s| s.to_vec())),
            &rule
        ));
        assert!(!matches_worker_rule(
            &make_task(Some("llm.chat"), None),
            &rule
        )); // missing gpu tag
    }

    fn make_task(task_type: Option<&str>, task_tags: Option<Vec<String>>) -> Task {
        Task {
            id: "test".to_string(),
            r#type: task_type.map(String::from),
            status: crate::types::TaskStatus::Pending,
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
            tags: task_tags,
            assign_mode: None,
            cost: None,
            assigned_worker: None,
            disconnect_policy: None,
        }
    }
}
```

**Step 2: Add module to `lib.rs`**

```rust
pub mod worker_matching;
```

**Step 3: Run tests**

Run: `cd rust && cargo test -p taskcast-core -- worker_matching 2>&1`
Expected: All PASS

**Step 4: Commit**

```bash
git add rust/taskcast-core/src/worker_matching.rs rust/taskcast-core/src/lib.rs
git commit -m "feat(rust/core): add worker matching logic (tag + type wildcards)"
```

---

### Task 2.2: Create WorkerManager

**Files:**
- Create: `rust/taskcast-core/src/worker_manager.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

This is the largest single module. It mirrors `packages/core/src/worker-manager.ts`.

**Step 1: Create the WorkerManager struct and options**

```rust
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use crate::types::*;
use crate::engine::TaskEngine;
use crate::worker_matching::matches_worker_rule;

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

#[derive(Debug, Clone)]
pub struct WorkerManagerDefaults {
    pub assign_mode: Option<AssignMode>,
    pub heartbeat_interval_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub offer_timeout_ms: u64,
    pub disconnect_policy: DisconnectPolicy,
    pub disconnect_grace_ms: u64,
}

impl Default for WorkerManagerDefaults {
    fn default() -> Self {
        Self {
            assign_mode: None,
            heartbeat_interval_ms: 30_000,
            heartbeat_timeout_ms: 90_000,
            offer_timeout_ms: 10_000,
            disconnect_policy: DisconnectPolicy::Reassign,
            disconnect_grace_ms: 30_000,
        }
    }
}

pub struct WorkerManagerOptions {
    pub engine: Arc<TaskEngine>,
    pub short_term: Arc<dyn ShortTermStore>,
    pub broadcast: Arc<dyn BroadcastProvider>,
    pub long_term: Option<Arc<dyn LongTermStore>>,
    pub hooks: Option<Arc<dyn TaskcastHooks>>,
    pub defaults: WorkerManagerDefaults,
}

pub struct WorkerManager {
    engine: Arc<TaskEngine>,
    short_term: Arc<dyn ShortTermStore>,
    broadcast: Arc<dyn BroadcastProvider>,
    long_term: Option<Arc<dyn LongTermStore>>,
    hooks: Option<Arc<dyn TaskcastHooks>>,
    defaults: WorkerManagerDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRegistration {
    pub worker_id: Option<String>,
    pub match_rule: WorkerMatchRule,
    pub capacity: u32,
    pub weight: Option<u32>,
    pub connection_mode: ConnectionMode,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerUpdate {
    pub weight: Option<u32>,
    pub capacity: Option<u32>,
    pub match_rule: Option<WorkerMatchRule>,
    pub status: Option<WorkerStatus>,
}

#[derive(Debug, Clone)]
pub struct DeclineOptions {
    pub blacklist: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DispatchResult {
    Dispatched { worker_id: String },
    NoMatch,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimResult {
    Claimed,
    Failed,
}
```

**Step 2: Implement core methods**

```rust
impl WorkerManager {
    pub fn new(opts: WorkerManagerOptions) -> Self {
        Self {
            engine: opts.engine,
            short_term: opts.short_term,
            broadcast: opts.broadcast,
            long_term: opts.long_term,
            hooks: opts.hooks,
            defaults: opts.defaults,
        }
    }

    // ── Registration & Lifecycle ──

    pub async fn register_worker(&self, config: WorkerRegistration)
        -> Result<Worker, Box<dyn std::error::Error + Send + Sync>>
    {
        let now = now_ms();
        let worker = Worker {
            id: config.worker_id.unwrap_or_else(|| ulid::Ulid::new().to_string()),
            status: WorkerStatus::Idle,
            match_rule: config.match_rule,
            capacity: config.capacity,
            used_slots: 0,
            weight: config.weight.unwrap_or(50),
            connection_mode: config.connection_mode,
            connected_at: now,
            last_heartbeat_at: now,
            metadata: config.metadata,
        };
        self.short_term.save_worker(worker.clone()).await?;
        self.emit_worker_audit(WorkerAuditAction::Connected, &worker.id, None);
        if let Some(hooks) = &self.hooks {
            hooks.on_worker_connected(&worker);
        }
        Ok(worker)
    }

    pub async fn unregister_worker(&self, worker_id: &str)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let worker = self.short_term.get_worker(worker_id).await?;
        self.short_term.delete_worker(worker_id).await?;
        self.emit_worker_audit(WorkerAuditAction::Disconnected, worker_id, None);
        if let Some(w) = &worker {
            if let Some(hooks) = &self.hooks {
                hooks.on_worker_disconnected(w, "unregistered");
            }
        }
        Ok(())
    }

    pub async fn update_worker(&self, worker_id: &str, update: WorkerUpdate)
        -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>>
    {
        let mut worker = match self.short_term.get_worker(worker_id).await? {
            Some(w) => w,
            None => return Ok(None),
        };
        if let Some(w) = update.weight { worker.weight = w; }
        if let Some(c) = update.capacity { worker.capacity = c; }
        if let Some(r) = update.match_rule { worker.match_rule = r; }
        if let Some(WorkerStatus::Draining) = update.status {
            worker.status = WorkerStatus::Draining;
            self.emit_worker_audit(WorkerAuditAction::Draining, worker_id, None);
        }
        self.short_term.save_worker(worker.clone()).await?;
        self.emit_worker_audit(WorkerAuditAction::Updated, worker_id, None);
        Ok(Some(worker))
    }

    pub async fn heartbeat(&self, worker_id: &str)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        if let Some(mut worker) = self.short_term.get_worker(worker_id).await? {
            worker.last_heartbeat_at = now_ms();
            self.short_term.save_worker(worker).await?;
        }
        Ok(())
    }

    pub async fn get_worker(&self, worker_id: &str)
        -> Result<Option<Worker>, Box<dyn std::error::Error + Send + Sync>>
    {
        self.short_term.get_worker(worker_id).await
    }

    pub async fn list_workers(&self, filter: Option<WorkerFilter>)
        -> Result<Vec<Worker>, Box<dyn std::error::Error + Send + Sync>>
    {
        self.short_term.list_workers(filter).await
    }

    // ── Task Dispatch ──

    pub async fn dispatch_task(&self, task_id: &str)
        -> Result<DispatchResult, Box<dyn std::error::Error + Send + Sync>>
    {
        let task = match self.engine.get_task(task_id).await? {
            Some(t) => t,
            None => return Ok(DispatchResult::NoMatch),
        };
        if task.status != TaskStatus::Pending {
            return Ok(DispatchResult::NoMatch);
        }

        let task_cost = task.cost.unwrap_or(1);
        let blacklist = self.get_blacklist(&task);

        let mut workers = self.short_term.list_workers(None).await?;

        // Filter candidates
        workers.retain(|w| {
            w.status != WorkerStatus::Offline
                && w.status != WorkerStatus::Draining
                && !blacklist.contains(&w.id)
                && w.used_slots + task_cost <= w.capacity
                && matches_worker_rule(&task, &w.match_rule)
        });

        if workers.is_empty() {
            return Ok(DispatchResult::NoMatch);
        }

        // Sort: weight desc → available slots desc → connectedAt asc
        workers.sort_by(|a, b| {
            b.weight.cmp(&a.weight)
                .then_with(|| (b.capacity - b.used_slots).cmp(&(a.capacity - a.used_slots)))
                .then_with(|| a.connected_at.partial_cmp(&b.connected_at).unwrap_or(std::cmp::Ordering::Equal))
        });

        let best = &workers[0];
        let claimed = self.short_term.claim_task(task_id, &best.id, task_cost).await?;
        if claimed {
            self.finalize_claim(task_id, &best.id, task_cost).await?;
            Ok(DispatchResult::Dispatched { worker_id: best.id.clone() })
        } else {
            Ok(DispatchResult::NoMatch)
        }
    }

    // ── Task Claim (for pull/ws-race) ──

    pub async fn claim_task(&self, task_id: &str, worker_id: &str)
        -> Result<ClaimResult, Box<dyn std::error::Error + Send + Sync>>
    {
        let task = match self.engine.get_task(task_id).await? {
            Some(t) => t,
            None => return Ok(ClaimResult::Failed),
        };
        let task_cost = task.cost.unwrap_or(1);

        let claimed = self.short_term.claim_task(task_id, worker_id, task_cost).await?;
        if claimed {
            self.finalize_claim(task_id, worker_id, task_cost).await?;
            Ok(ClaimResult::Claimed)
        } else {
            Ok(ClaimResult::Failed)
        }
    }

    async fn finalize_claim(&self, task_id: &str, worker_id: &str, cost: u32)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        // Re-read task (now assigned)
        let task = self.engine.get_task(task_id).await?;

        // Save to long-term if available
        if let (Some(long_term), Some(ref t)) = (&self.long_term, &task) {
            let _ = long_term.save_task(t.clone()).await;
        }

        // Emit audit events
        self.emit_worker_audit(WorkerAuditAction::TaskAssigned, worker_id, Some({
            let mut m = HashMap::new();
            m.insert("taskId".to_string(), serde_json::Value::String(task_id.to_string()));
            m
        }));
        self.emit_task_audit(task_id, "assigned", Some({
            let mut m = HashMap::new();
            m.insert("workerId".to_string(), serde_json::Value::String(worker_id.to_string()));
            m
        })).await;

        // Create assignment record
        self.short_term.add_assignment(WorkerAssignment {
            task_id: task_id.to_string(),
            worker_id: worker_id.to_string(),
            cost,
            assigned_at: now_ms(),
            status: WorkerAssignmentStatus::Assigned,
        }).await?;

        // Update worker slots
        if let Some(mut worker) = self.short_term.get_worker(worker_id).await? {
            worker.used_slots += cost;
            if worker.used_slots >= worker.capacity {
                worker.status = WorkerStatus::Busy;
            }
            self.short_term.save_worker(worker.clone()).await?;

            // Call hook
            if let (Some(hooks), Some(ref t)) = (&self.hooks, &task) {
                hooks.on_task_assigned(t, &worker);
            }
        }

        Ok(())
    }

    // ── Task Decline ──

    pub async fn decline_task(&self, task_id: &str, worker_id: &str, opts: Option<DeclineOptions>)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let assignment = self.short_term.get_task_assignment(task_id).await?;
        let cost = assignment.as_ref().map(|a| a.cost).unwrap_or(1);

        // Remove assignment
        self.short_term.remove_assignment(task_id).await?;

        // Restore worker capacity
        if let Some(mut worker) = self.short_term.get_worker(worker_id).await? {
            worker.used_slots = worker.used_slots.saturating_sub(cost);
            if worker.status == WorkerStatus::Busy {
                worker.status = WorkerStatus::Idle;
            }
            self.short_term.save_worker(worker.clone()).await?;
        }

        // Transition task back to pending
        let _ = self.engine.transition_task(task_id, TaskStatus::Pending, None).await;

        // Clear assigned worker
        if let Some(mut task) = self.short_term.get_task(task_id).await? {
            task.assigned_worker = None;

            // Handle blacklist
            let blacklisted = opts.as_ref().map(|o| o.blacklist).unwrap_or(false);
            if blacklisted {
                let metadata = task.metadata.get_or_insert_with(HashMap::new);
                let bl = metadata.entry("_blacklistedWorkers".to_string())
                    .or_insert_with(|| serde_json::Value::Array(vec![]));
                if let serde_json::Value::Array(arr) = bl {
                    arr.push(serde_json::Value::String(worker_id.to_string()));
                }
            }

            self.short_term.save_task(task.clone()).await?;

            // Emit audits
            self.emit_worker_audit(WorkerAuditAction::TaskDeclined, worker_id, Some({
                let mut m = HashMap::new();
                m.insert("taskId".to_string(), serde_json::Value::String(task_id.to_string()));
                m
            }));
            self.emit_task_audit(task_id, "declined", Some({
                let mut m = HashMap::new();
                m.insert("workerId".to_string(), serde_json::Value::String(worker_id.to_string()));
                m
            })).await;

            // Call hook
            if let Some(hooks) = &self.hooks {
                if let Some(worker) = self.short_term.get_worker(worker_id).await? {
                    hooks.on_task_declined(&task, &worker, blacklisted);
                }
            }
        }

        Ok(())
    }

    // ── Worker Tasks ──

    pub async fn get_worker_tasks(&self, worker_id: &str)
        -> Result<Vec<WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>>
    {
        self.short_term.get_worker_assignments(worker_id).await
    }

    // ── Pull Mode ──

    pub async fn wait_for_task(&self, worker_id: &str, timeout_ms: u64)
        -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>>
    {
        // Register/refresh heartbeat
        self.heartbeat(worker_id).await?;

        let worker = match self.short_term.get_worker(worker_id).await? {
            Some(w) => w,
            None => return Ok(None),
        };

        // Check existing pending pull-mode tasks
        let tasks = self.short_term.list_tasks(TaskFilter {
            status: Some(TaskStatus::Pending),
            r#type: None,
            assign_mode: Some(AssignMode::Pull),
        }).await?;

        let task_cost_default = 1u32;
        let blacklist = Vec::<String>::new(); // Worker has no task-specific blacklist

        for task in &tasks {
            let cost = task.cost.unwrap_or(task_cost_default);
            if worker.used_slots + cost > worker.capacity {
                continue;
            }
            if !matches_worker_rule(task, &worker.match_rule) {
                continue;
            }
            let claimed = self.short_term.claim_task(&task.id, worker_id, cost).await?;
            if claimed {
                self.finalize_claim(&task.id, worker_id, cost).await?;
                self.emit_worker_audit(WorkerAuditAction::PullRequest, worker_id, Some({
                    let mut m = HashMap::new();
                    m.insert("matched".to_string(), serde_json::Value::Bool(true));
                    m.insert("taskId".to_string(), serde_json::Value::String(task.id.clone()));
                    m
                }));
                let task = self.engine.get_task(&task.id).await?;
                return Ok(task);
            }
        }

        // No task available — wait for broadcast notification
        let (tx, mut rx) = tokio::sync::oneshot::channel::<String>();
        let tx = std::sync::Mutex::new(Some(tx));

        let unsub = self.broadcast.subscribe(
            "taskcast:worker:new-task",
            Box::new(move |event| {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(event.task_id.clone());
                }
            }),
        ).await;

        let result = tokio::select! {
            task_id = &mut rx => {
                match task_id {
                    Ok(tid) => {
                        // Try to claim the notified task
                        if let Some(task) = self.engine.get_task(&tid).await? {
                            if matches_worker_rule(&task, &worker.match_rule) {
                                let cost = task.cost.unwrap_or(1);
                                if self.short_term.claim_task(&tid, worker_id, cost).await? {
                                    self.finalize_claim(&tid, worker_id, cost).await?;
                                    self.emit_worker_audit(WorkerAuditAction::PullRequest, worker_id, Some({
                                        let mut m = HashMap::new();
                                        m.insert("matched".to_string(), serde_json::Value::Bool(true));
                                        m.insert("taskId".to_string(), serde_json::Value::String(tid));
                                        m
                                    }));
                                    return Ok(self.engine.get_task(&task.id).await?);
                                }
                            }
                        }
                        self.emit_worker_audit(WorkerAuditAction::PullRequest, worker_id, Some({
                            let mut m = HashMap::new();
                            m.insert("matched".to_string(), serde_json::Value::Bool(false));
                            m
                        }));
                        Ok(None)
                    }
                    Err(_) => Ok(None),
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(timeout_ms)) => {
                self.emit_worker_audit(WorkerAuditAction::PullRequest, worker_id, Some({
                    let mut m = HashMap::new();
                    m.insert("matched".to_string(), serde_json::Value::Bool(false));
                    m
                }));
                Ok(None)
            }
        };

        unsub();
        result
    }

    pub async fn notify_new_task(&self, task_id: &str)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        // Create a minimal event to broadcast on the new-task channel
        let event = TaskEvent {
            id: ulid::Ulid::new().to_string(),
            task_id: task_id.to_string(),
            index: 0,
            timestamp: now_ms(),
            r#type: "taskcast:worker:new-task".to_string(),
            level: Level::Info,
            data: serde_json::Value::Null,
            series_id: None,
            series_mode: None,
        };
        self.broadcast.publish("taskcast:worker:new-task", event).await?;
        Ok(())
    }

    // ── Private Helpers ──

    fn get_blacklist(&self, task: &Task) -> Vec<String> {
        task.metadata
            .as_ref()
            .and_then(|m| m.get("_blacklistedWorkers"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    async fn emit_task_audit(&self, task_id: &str, action: &str, extra: Option<HashMap<String, serde_json::Value>>) {
        let mut data = HashMap::new();
        data.insert("action".to_string(), serde_json::Value::String(action.to_string()));
        if let Some(extra) = extra {
            for (k, v) in extra {
                data.insert(k, v);
            }
        }

        let input = crate::engine::PublishEventInput {
            r#type: "taskcast:audit".to_string(),
            level: Level::Info,
            data: serde_json::to_value(&data).unwrap_or(serde_json::Value::Null),
            series_id: None,
            series_mode: None,
        };

        // Best-effort: ignore errors (task may be terminal)
        let _ = self.engine.publish_event(task_id, input).await;
    }

    fn emit_worker_audit(&self, action: WorkerAuditAction, worker_id: &str, data: Option<HashMap<String, serde_json::Value>>) {
        if let Some(long_term) = &self.long_term {
            let event = WorkerAuditEvent {
                id: ulid::Ulid::new().to_string(),
                worker_id: worker_id.to_string(),
                timestamp: now_ms(),
                action,
                data,
            };
            let lt = Arc::clone(long_term);
            tokio::spawn(async move {
                let _ = lt.save_worker_event(event).await;
            });
        }
    }
}
```

**Step 3: Add to `lib.rs`**

```rust
pub mod worker_manager;
```

**Step 4: Run `cargo check`**

Run: `cd rust && cargo check -p taskcast-core 2>&1`
Expected: Errors from adapters not implementing new trait methods — expected.

**Step 5: Commit**

```bash
git add rust/taskcast-core/src/worker_manager.rs rust/taskcast-core/src/lib.rs
git commit -m "feat(rust/core): add WorkerManager with dispatch, claim, decline, pull"
```

---

## Phase 3: Memory Adapters

### Task 3.1: Implement new ShortTermStore methods in MemoryShortTermStore

**Files:**
- Modify: `rust/taskcast-core/src/memory_adapters.rs`

**Step 1: Add worker storage fields to `MemoryShortTermStore`**

```rust
pub struct MemoryShortTermStore {
    // ...existing fields...
    workers: RwLock<HashMap<String, Worker>>,
    assignments: RwLock<HashMap<String, WorkerAssignment>>,
}
```

Initialize both as `RwLock::new(HashMap::new())` in `new()`.

**Step 2: Implement all new trait methods**

```rust
// list_tasks
async fn list_tasks(&self, filter: TaskFilter) -> Result<Vec<Task>, ...> {
    let tasks = self.tasks.read().await;
    Ok(tasks.values()
        .filter(|t| {
            filter.status.as_ref().map_or(true, |s| &t.status == s)
                && filter.r#type.as_ref().map_or(true, |ty| t.r#type.as_deref() == Some(ty))
                && filter.assign_mode.as_ref().map_or(true, |am| t.assign_mode.as_ref() == Some(am))
        })
        .cloned()
        .collect())
}

// save_worker, get_worker, list_workers, delete_worker — straightforward HashMap ops with filter

// claim_task — single-threaded safe in Rust with write lock
async fn claim_task(&self, task_id: &str, worker_id: &str, cost: u32) -> Result<bool, ...> {
    let mut tasks = self.tasks.write().await;
    let mut workers = self.workers.write().await;

    let task = match tasks.get_mut(task_id) {
        Some(t) if t.status == TaskStatus::Pending || t.status == TaskStatus::Assigned => t,
        _ => return Ok(false),
    };
    let worker = match workers.get_mut(worker_id) {
        Some(w) if w.used_slots + cost <= w.capacity => w,
        _ => return Ok(false),
    };

    task.status = TaskStatus::Assigned;
    task.assigned_worker = Some(worker_id.to_string());
    task.cost = Some(cost);
    task.updated_at = now_ms();

    Ok(true)
}

// add_assignment, remove_assignment, get_worker_assignments, get_task_assignment
// — HashMap keyed by taskId for assignments, with worker_id lookups via filter
```

**Step 3: Implement new `LongTermStore` methods on in-memory adapter (if one exists for testing)**

Add default implementations or a `MemoryLongTermStore` extension:

```rust
async fn save_worker_event(&self, event: WorkerAuditEvent) -> Result<(), ...> {
    let mut events = self.worker_events.write().await;
    events.entry(event.worker_id.clone()).or_default().push(event);
    Ok(())
}

async fn get_worker_events(&self, worker_id: &str, opts: Option<EventQueryOptions>) -> Result<Vec<WorkerAuditEvent>, ...> {
    let events = self.worker_events.read().await;
    // Apply opts filtering (since timestamp, limit)
    Ok(events.get(worker_id).cloned().unwrap_or_default())
}
```

**Step 4: Run full core test suite**

Run: `cd rust && cargo test -p taskcast-core 2>&1`
Expected: All existing tests PASS. New trait methods compile.

**Step 5: Commit**

```bash
git add rust/taskcast-core/src/memory_adapters.rs
git commit -m "feat(rust/core): implement worker methods in memory adapters"
```

---

## Phase 4: Redis Adapter

### Task 4.1: Implement worker methods in Redis short-term store

**Files:**
- Modify: `rust/taskcast-redis/src/short_term.rs`

**Step 1: Add Redis key helpers**

```rust
fn worker_key(&self, worker_id: &str) -> String {
    format!("{}:worker:{}", self.prefix, worker_id)
}
fn workers_set_key(&self) -> String {
    format!("{}:workers", self.prefix)
}
fn assignment_key(&self, task_id: &str) -> String {
    format!("{}:assignment:{}", self.prefix, task_id)
}
fn worker_assignments_key(&self, worker_id: &str) -> String {
    format!("{}:workerAssignments:{}", self.prefix, worker_id)
}
```

**Step 2: Implement CRUD methods**

- `save_worker`: `SET worker:{id}` + `SADD workers {id}`
- `get_worker`: `GET worker:{id}` + deserialize
- `list_workers`: `SMEMBERS workers` + `MGET` all + filter
- `delete_worker`: `DEL worker:{id}` + `SREM workers {id}`
- `list_tasks`: `SCAN` or maintain a set of task IDs + filter (matches TS impl)

**Step 3: Implement `claim_task` with Lua script (CRITICAL)**

```rust
async fn claim_task(&self, task_id: &str, worker_id: &str, cost: u32)
    -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
{
    let script = redis::Script::new(r#"
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
    "#);

    let now = now_ms();
    let result: i32 = script
        .key(self.task_key(task_id))
        .key(self.worker_key(worker_id))
        .arg(cost)
        .arg(worker_id)
        .arg(now as i64)
        .invoke_async(&mut self.conn.clone())
        .await?;

    Ok(result == 1)
}
```

**Step 4: Implement assignment methods**

- `add_assignment`: `SET assignment:{taskId}` + `SADD workerAssignments:{workerId} {taskId}`
- `remove_assignment`: `GET assignment:{taskId}` → get workerId → `SREM workerAssignments:{workerId} {taskId}` + `DEL assignment:{taskId}`
- `get_worker_assignments`: `SMEMBERS workerAssignments:{workerId}` + `MGET`
- `get_task_assignment`: `GET assignment:{taskId}`

**Step 5: Run `cargo check`**

Run: `cd rust && cargo check -p taskcast-redis 2>&1`
Expected: Compiles

**Step 6: Commit**

```bash
git add rust/taskcast-redis/src/short_term.rs
git commit -m "feat(rust/redis): implement worker CRUD, atomic claim via Lua, and assignments"
```

---

## Phase 5: PostgreSQL Adapter

### Task 5.1: Add migration and worker event storage

**Files:**
- Modify: `rust/taskcast-postgres/src/store.rs`

**Step 1: Update `migrate()` method to create worker tables and add new columns**

Add to the `migrate` method, after existing table creation:

```rust
// Worker audit events table
sqlx::query(&format!(
    "CREATE TABLE IF NOT EXISTS {}_worker_events (
        id TEXT PRIMARY KEY,
        worker_id TEXT NOT NULL,
        timestamp BIGINT NOT NULL,
        action TEXT NOT NULL,
        data JSONB,
        created_at TIMESTAMPTZ DEFAULT now()
    )", self.tables.prefix
))
.execute(&self.pool).await?;

sqlx::query(&format!(
    "CREATE INDEX IF NOT EXISTS idx_{}_worker_events_worker_id
     ON {}_worker_events (worker_id, timestamp DESC)",
    self.tables.prefix, self.tables.prefix
))
.execute(&self.pool).await?;

// Add worker columns to tasks table
for col in &[
    ("tags", "JSONB"),
    ("assign_mode", "TEXT"),
    ("cost", "INTEGER"),
    ("assigned_worker", "TEXT"),
    ("disconnect_policy", "TEXT"),
] {
    let _ = sqlx::query(&format!(
        "ALTER TABLE {} ADD COLUMN IF NOT EXISTS {} {}",
        self.tables.tasks, col.0, col.1
    ))
    .execute(&self.pool).await;
}
```

**Step 2: Update `save_task` to include new columns**

Update the INSERT and UPDATE SQL to include `tags`, `assign_mode`, `cost`, `assigned_worker`, `disconnect_policy`.

**Step 3: Update `row_to_task` to read new columns**

Read new columns from SQL rows and map to Task fields:
- `tags`: JSONB → `Option<Vec<String>>`
- `assign_mode`: TEXT → `Option<AssignMode>` (deserialize from string)
- `cost`: INTEGER → `Option<u32>` (use `row.get::<_, Option<i32>>()` and convert)
- `assigned_worker`: TEXT → `Option<String>`
- `disconnect_policy`: TEXT → `Option<DisconnectPolicy>` (deserialize from string)

**Step 4: Implement `save_worker_event` and `get_worker_events`**

```rust
async fn save_worker_event(&self, event: WorkerAuditEvent)
    -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    sqlx::query(&format!(
        "INSERT INTO {}_worker_events (id, worker_id, timestamp, action, data) VALUES ($1, $2, $3, $4, $5)",
        self.tables.prefix
    ))
    .bind(&event.id)
    .bind(&event.worker_id)
    .bind(event.timestamp as i64)
    .bind(serde_json::to_string(&event.action)?.trim_matches('"'))
    .bind(event.data.as_ref().map(|d| serde_json::to_value(d).ok()).flatten())
    .execute(&self.pool)
    .await?;
    Ok(())
}

async fn get_worker_events(&self, worker_id: &str, opts: Option<EventQueryOptions>)
    -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>>
{
    let mut query = format!(
        "SELECT id, worker_id, timestamp, action, data FROM {}_worker_events WHERE worker_id = $1",
        self.tables.prefix
    );
    // Apply opts: since.timestamp, since.id, limit
    query.push_str(" ORDER BY timestamp ASC");
    if let Some(opts) = &opts {
        if let Some(limit) = opts.limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }
    }
    // Execute query and map rows to WorkerAuditEvent
    // ...
}
```

**Step 5: Run `cargo check`**

Run: `cd rust && cargo check -p taskcast-postgres 2>&1`
Expected: Compiles

**Step 6: Commit**

```bash
git add rust/taskcast-postgres/src/store.rs
git commit -m "feat(rust/postgres): add worker events table, new task columns, migration"
```

---

## Phase 6: Server Auth Extension

### Task 6.1: Add `jti` and `worker_id` to AuthContext

**Files:**
- Modify: `rust/taskcast-server/src/auth.rs`

**Step 1: Extend `AuthContext`**

```rust
pub struct AuthContext {
    pub sub: Option<String>,
    pub jti: Option<String>,        // NEW
    pub worker_id: Option<String>,   // NEW
    pub task_ids: TaskIdAccess,
    pub scope: Vec<PermissionScope>,
}
```

**Step 2: Extend `JwtClaims`**

```rust
#[derive(Debug, Serialize, Deserialize)]
struct JwtClaims {
    // ...existing fields...
    #[serde(default)]
    jti: Option<String>,
    #[serde(default, rename = "workerId")]
    worker_id: Option<String>,
}
```

**Step 3: Extract new fields in `auth_middleware`**

After decoding JWT, when building `AuthContext`:

```rust
jti: claims.jti,
worker_id: claims.worker_id,
```

**Step 4: Update `AuthContext::open()`**

```rust
pub fn open() -> Self {
    Self {
        sub: None,
        jti: None,
        worker_id: None,
        task_ids: TaskIdAccess::All,
        scope: vec![PermissionScope::All],
    }
}
```

**Step 5: Commit**

```bash
git add rust/taskcast-server/src/auth.rs
git commit -m "feat(rust/server): add jti and workerId to AuthContext"
```

---

## Phase 7: Server Worker Routes

### Task 7.1: Create worker REST routes

**Files:**
- Create: `rust/taskcast-server/src/routes/workers.rs`
- Modify: `rust/taskcast-server/src/routes/mod.rs`

**Step 1: Create workers route module**

```rust
use axum::{
    extract::{Path, Query, State, Extension},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, delete},
    Json, Router,
};
use std::sync::Arc;
use taskcast_core::{
    worker_manager::{WorkerManager, DeclineOptions},
    types::PermissionScope,
};
use crate::auth::{AuthContext, check_scope};
use crate::error::AppError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullQuery {
    pub worker_id: String,
    pub weight: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclineBody {
    pub worker_id: String,
    pub blacklist: Option<bool>,
}

pub fn workers_router() -> Router<Arc<WorkerManager>> {
    Router::new()
        .route("/", get(list_workers))
        .route("/pull", get(pull_task))
        .route("/{worker_id}", get(get_worker).delete(delete_worker))
        .route("/tasks/{task_id}/decline", post(decline_task))
}

async fn list_workers(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }
    let workers = manager.list_workers(None).await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(Json(workers))
}

async fn get_worker(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(worker_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }
    match manager.get_worker(&worker_id).await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        Some(w) => Ok(Json(w).into_response()),
        None => Err(AppError::NotFound(format!("Worker {}", worker_id))),
    }
}

async fn delete_worker(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(worker_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerManage, None) {
        return Err(AppError::Forbidden);
    }
    manager.unregister_worker(&worker_id).await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pull_task(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<PullQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }

    // Update weight if provided
    if let Some(weight) = query.weight {
        let _ = manager.update_worker(&query.worker_id, taskcast_core::worker_manager::WorkerUpdate {
            weight: Some(weight),
            capacity: None,
            match_rule: None,
            status: None,
        }).await;
    }

    match manager.wait_for_task(&query.worker_id, 30_000).await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        Some(task) => Ok((StatusCode::OK, Json(task)).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn decline_task(
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    Json(body): Json<DeclineBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }
    manager.decline_task(&task_id, &body.worker_id, Some(DeclineOptions {
        blacklist: body.blacklist.unwrap_or(false),
    })).await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```

**Step 2: Update `routes/mod.rs`**

```rust
pub mod tasks;
pub mod sse;
pub mod workers;
```

**Step 3: Commit**

```bash
git add rust/taskcast-server/src/routes/workers.rs rust/taskcast-server/src/routes/mod.rs
git commit -m "feat(rust/server): add worker REST routes (list, get, delete, pull, decline)"
```

---

### Task 7.2: Create WebSocket handler

**Files:**
- Create: `rust/taskcast-server/src/routes/worker_ws.rs`
- Modify: `rust/taskcast-server/src/routes/mod.rs`
- Modify: `rust/taskcast-server/Cargo.toml` (add `axum` ws feature + `tokio-tungstenite`)

**Step 1: Add WebSocket dependency**

In `rust/taskcast-server/Cargo.toml`, ensure axum has `ws` feature:

```toml
[dependencies]
axum = { version = "0.8", features = ["ws"] }
```

**Step 2: Create WebSocket handler**

```rust
use axum::{
    extract::{ws::{WebSocket, WebSocketUpgrade, Message}, State, Extension},
    response::IntoResponse,
};
use std::sync::Arc;
use std::collections::HashMap;
use taskcast_core::{
    worker_manager::{WorkerManager, WorkerRegistration, WorkerUpdate, ClaimResult, DeclineOptions},
    types::*,
};
use crate::auth::{AuthContext, check_scope};
use crate::error::AppError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientMessage {
    Register {
        match_rule: WorkerMatchRule,
        capacity: u32,
        #[serde(default)]
        worker_id: Option<String>,
        #[serde(default)]
        weight: Option<u32>,
    },
    Update {
        #[serde(default)]
        weight: Option<u32>,
        #[serde(default)]
        capacity: Option<u32>,
        #[serde(default)]
        match_rule: Option<WorkerMatchRule>,
    },
    Accept { task_id: String },
    Decline { task_id: String, #[serde(default)] blacklist: Option<bool> },
    Claim { task_id: String },
    Drain,
    Pong,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMessage {
    Registered { worker_id: String },
    Offer { task_id: String, task: TaskSummary },
    Available { task_id: String, task: TaskSummary },
    Assigned { task_id: String },
    Claimed { task_id: String, success: bool },
    Declined { task_id: String },
    Ping,
    Error { message: String, #[serde(skip_serializing_if = "Option::is_none")] code: Option<String> },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
}

fn to_summary(task: &Task) -> TaskSummary {
    TaskSummary {
        id: task.id.clone(),
        r#type: task.r#type.clone(),
        tags: task.tags.clone(),
        cost: task.cost,
        params: task.params.clone(),
    }
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(manager): State<Arc<WorkerManager>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, manager, auth)))
}

async fn handle_socket(mut socket: WebSocket, manager: Arc<WorkerManager>, auth: AuthContext) {
    let mut worker_id: Option<String> = None;

    while let Some(msg) = socket.recv().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            _ => continue,
        };

        let client_msg: ClientMessage = match serde_json::from_str(&msg) {
            Ok(m) => m,
            Err(e) => {
                let _ = send(&mut socket, &ServerMessage::Error {
                    message: format!("Invalid message: {}", e),
                    code: Some("PARSE_ERROR".to_string()),
                }).await;
                continue;
            }
        };

        match client_msg {
            ClientMessage::Register { match_rule, capacity, worker_id: req_wid, weight } => {
                // If JWT specifies workerId, enforce it
                let wid = auth.worker_id.clone().or(req_wid);
                match manager.register_worker(WorkerRegistration {
                    worker_id: wid,
                    match_rule,
                    capacity,
                    weight,
                    connection_mode: ConnectionMode::Websocket,
                    metadata: None,
                }).await {
                    Ok(w) => {
                        worker_id = Some(w.id.clone());
                        let _ = send(&mut socket, &ServerMessage::Registered { worker_id: w.id }).await;
                    }
                    Err(e) => {
                        let _ = send(&mut socket, &ServerMessage::Error {
                            message: e.to_string(),
                            code: None,
                        }).await;
                    }
                }
            }
            ClientMessage::Update { weight, capacity, match_rule } => {
                if let Some(wid) = &worker_id {
                    let _ = manager.update_worker(wid, WorkerUpdate {
                        weight,
                        capacity,
                        match_rule,
                        status: None,
                    }).await;
                }
            }
            ClientMessage::Accept { task_id } | ClientMessage::Claim { task_id } => {
                if let Some(wid) = &worker_id {
                    match manager.claim_task(&task_id, wid).await {
                        Ok(ClaimResult::Claimed) => {
                            let resp = if matches!(client_msg, ClientMessage::Accept { .. }) {
                                ServerMessage::Assigned { task_id }
                            } else {
                                ServerMessage::Claimed { task_id, success: true }
                            };
                            let _ = send(&mut socket, &resp).await;
                        }
                        Ok(ClaimResult::Failed) => {
                            let resp = if matches!(client_msg, ClientMessage::Accept { .. }) {
                                ServerMessage::Error {
                                    message: "Claim failed".to_string(),
                                    code: Some("CLAIM_FAILED".to_string()),
                                }
                            } else {
                                ServerMessage::Claimed { task_id, success: false }
                            };
                            let _ = send(&mut socket, &resp).await;
                        }
                        Err(e) => {
                            let _ = send(&mut socket, &ServerMessage::Error {
                                message: e.to_string(),
                                code: None,
                            }).await;
                        }
                    }
                }
            }
            ClientMessage::Decline { task_id, blacklist } => {
                if let Some(wid) = &worker_id {
                    let _ = manager.decline_task(&task_id, wid, Some(DeclineOptions {
                        blacklist: blacklist.unwrap_or(false),
                    })).await;
                    let _ = send(&mut socket, &ServerMessage::Declined { task_id }).await;
                }
            }
            ClientMessage::Drain => {
                if let Some(wid) = &worker_id {
                    let _ = manager.update_worker(wid, WorkerUpdate {
                        weight: None,
                        capacity: None,
                        match_rule: None,
                        status: Some(WorkerStatus::Draining),
                    }).await;
                }
            }
            ClientMessage::Pong => {
                if let Some(wid) = &worker_id {
                    let _ = manager.heartbeat(wid).await;
                }
            }
        }
    }

    // Handle disconnect
    if let Some(wid) = &worker_id {
        let _ = manager.unregister_worker(wid).await;
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap_or_default();
    socket.send(Message::Text(text.into())).await
}
```

**Step 3: Update `routes/mod.rs`**

```rust
pub mod workers;
pub mod worker_ws;
```

**Step 4: Commit**

```bash
git add rust/taskcast-server/src/routes/worker_ws.rs rust/taskcast-server/src/routes/mod.rs rust/taskcast-server/Cargo.toml
git commit -m "feat(rust/server): add WebSocket handler for ws-offer and ws-race modes"
```

---

## Phase 8: Server Assembly & Task Route Extension

### Task 8.1: Extend task create route with new fields

**Files:**
- Modify: `rust/taskcast-server/src/routes/tasks.rs`

**Step 1: Extend `CreateTaskBody` with new optional fields**

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskBody {
    // ...existing fields...
    pub tags: Option<Vec<String>>,
    pub assign_mode: Option<AssignMode>,
    pub cost: Option<u32>,
    pub disconnect_policy: Option<DisconnectPolicy>,
}
```

**Step 2: Pass new fields to `CreateTaskInput`**

```rust
let input = CreateTaskInput {
    // ...existing fields...
    tags: body.tags,
    assign_mode: body.assign_mode,
    cost: body.cost,
    disconnect_policy: body.disconnect_policy,
};
```

**Step 3: Extend transition route to accept `assigned` status**

In the `transition_task` handler, ensure `"assigned"` is accepted as a valid status string (serde should handle this automatically since `TaskStatus::Assigned` is in the enum).

**Step 4: Commit**

```bash
git add rust/taskcast-server/src/routes/tasks.rs
git commit -m "feat(rust/server): extend task create route with worker assignment fields"
```

---

### Task 8.2: Mount worker routes in app assembly

**Files:**
- Modify: `rust/taskcast-server/src/app.rs`
- Modify: `rust/taskcast-server/src/lib.rs`

**Step 1: Update `create_app` to optionally accept WorkerManager**

```rust
pub fn create_app(
    engine: Arc<TaskEngine>,
    auth_mode: AuthMode,
    worker_manager: Option<Arc<WorkerManager>>,
) -> Router {
    let auth_mode = Arc::new(auth_mode);

    let task_routes = Router::new()
        // ...existing routes...
        .with_state(Arc::clone(&engine));

    let mut app = Router::new()
        .route("/health", get(|| async { Json(serde_json::json!({"ok": true})) }))
        .nest("/tasks", task_routes);

    // Conditionally mount worker routes
    if let Some(manager) = worker_manager {
        let worker_routes = crate::routes::workers::workers_router()
            .with_state(Arc::clone(&manager));
        let ws_route = Router::new()
            .route("/workers/ws", get(crate::routes::worker_ws::ws_handler))
            .with_state(manager);
        app = app.merge(ws_route).nest("/workers", worker_routes);
    }

    app.layer(middleware::from_fn_with_state(
        Arc::clone(&auth_mode),
        auth_middleware,
    ))
}
```

**Step 2: Update `lib.rs` exports**

```rust
pub mod routes;
pub mod auth;
pub mod app;
pub mod error;
pub mod webhook;
```

Ensure `WorkerManager` is re-exported or accessible.

**Step 3: Run `cargo check`**

Run: `cd rust && cargo check 2>&1`
Expected: Full workspace compiles.

**Step 4: Commit**

```bash
git add rust/taskcast-server/src/app.rs rust/taskcast-server/src/lib.rs
git commit -m "feat(rust/server): mount worker routes and WebSocket handler in app assembly"
```

---

## Phase 9: CLI Integration

### Task 9.1: Wire WorkerManager into CLI startup

**Files:**
- Modify: `rust/taskcast-cli/src/main.rs`

**Step 1: Parse worker config from YAML**

Add worker config parsing after existing config loading:

```rust
let workers_enabled = config.get("workers")
    .and_then(|w| w.get("enabled"))
    .and_then(|e| e.as_bool())
    .unwrap_or(false);
```

**Step 2: Create WorkerManager if enabled**

```rust
let worker_manager = if workers_enabled {
    Some(Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term: Arc::clone(&short_term),
        broadcast: Arc::clone(&broadcast),
        long_term: long_term.clone(),
        hooks: None,
        defaults: WorkerManagerDefaults::default(),
    })))
} else {
    None
};
```

**Step 3: Pass to `create_app`**

```rust
let app = create_app(Arc::clone(&engine), auth_mode, worker_manager);
```

**Step 4: Run `cargo build`**

Run: `cd rust && cargo build 2>&1`
Expected: Full build succeeds.

**Step 5: Commit**

```bash
git add rust/taskcast-cli/src/main.rs
git commit -m "feat(rust/cli): wire WorkerManager into CLI startup when workers enabled"
```

---

## Phase 10: Tests

### Task 10.1: Core unit tests

**Files:**
- Create: `rust/taskcast-core/src/worker_manager.rs` (append `#[cfg(test)] mod tests` block)

**Tests to write:**

1. `test_register_worker` — registers and retrieves worker
2. `test_unregister_worker` — registers then unregisters, verify gone
3. `test_update_worker_weight` — update weight, verify changed
4. `test_heartbeat_updates_timestamp` — heartbeat renews lastHeartbeatAt
5. `test_dispatch_task_selects_best_worker` — multiple workers, verify priority (weight > slots > connectedAt)
6. `test_dispatch_task_respects_capacity` — worker at capacity gets skipped
7. `test_dispatch_task_respects_blacklist` — blacklisted worker gets skipped
8. `test_claim_task_atomic` — claim succeeds and assigns worker
9. `test_claim_task_already_assigned` — second claim fails
10. `test_decline_task_returns_to_pending` — decline reverts status
11. `test_decline_with_blacklist` — blacklisted worker ID added to metadata
12. `test_worker_status_idle_to_busy` — usedSlots fills capacity → status changes to busy
13. `test_worker_status_busy_to_idle_on_decline` — decline frees slots → idle

All tests use `MemoryShortTermStore` and `MemoryBroadcastProvider`.

**Step 1: Write tests, verify they pass**

Run: `cd rust && cargo test -p taskcast-core -- worker_manager 2>&1`
Expected: All PASS

**Step 2: Commit**

```bash
git add rust/taskcast-core/src/worker_manager.rs
git commit -m "test(rust/core): add WorkerManager unit tests"
```

---

### Task 10.2: Memory adapter tests

**Files:**
- Modify: `rust/taskcast-core/src/memory_adapters.rs` (append to existing test module)

**Tests to write:**

1. `test_save_and_get_worker` — round-trip worker storage
2. `test_list_workers_with_filter` — filter by status
3. `test_delete_worker` — delete removes worker
4. `test_claim_task_success` — atomic claim on pending task
5. `test_claim_task_fails_wrong_status` — claim on running task fails
6. `test_claim_task_fails_over_capacity` — worker at capacity fails
7. `test_assignment_crud` — add, get, remove assignment
8. `test_list_tasks_with_filter` — filter by status, type, assignMode
9. `test_worker_events_crud` — save and retrieve worker audit events

Run: `cd rust && cargo test -p taskcast-core -- memory_adapters 2>&1`
Expected: All PASS

**Commit:**

```bash
git add rust/taskcast-core/src/memory_adapters.rs
git commit -m "test(rust/core): add memory adapter tests for worker methods"
```

---

### Task 10.3: Worker matching tests (already in Task 2.1)

Already written inline in `worker_matching.rs`. Verify:

Run: `cd rust && cargo test -p taskcast-core -- worker_matching 2>&1`
Expected: All PASS

---

### Task 10.4: Full workspace build and test

**Step 1: Build entire workspace**

Run: `cd rust && cargo build 2>&1`
Expected: Clean build

**Step 2: Run all tests**

Run: `cd rust && cargo test 2>&1`
Expected: All tests pass

**Step 3: Commit any remaining fixes**

```bash
git add -A
git commit -m "fix(rust): resolve any remaining compilation/test issues"
```

---

## Phase 11: SSE Terminal Status Fix

### Task 11.1: Update SSE to use `TERMINAL_STATUSES` from state machine

**Files:**
- Modify: `rust/taskcast-server/src/routes/sse.rs`

Ensure the SSE handler uses `TERMINAL_STATUSES` from `taskcast_core::state_machine` rather than a hardcoded set, so `Assigned` is correctly treated as non-terminal.

**Step 1: Verify SSE uses `is_terminal()` or `TERMINAL_STATUSES`**

Check the existing code. If it uses a hardcoded match/set, replace with:

```rust
use taskcast_core::state_machine::is_terminal;

// Replace any `matches!(status, Completed | Failed | ...)` with:
if is_terminal(&task.status) {
    // ... terminal handling
}
```

**Step 2: Commit if changed**

```bash
git add rust/taskcast-server/src/routes/sse.rs
git commit -m "fix(rust/server): use TERMINAL_STATUSES for SSE instead of hardcoded set"
```

---

## Summary

| Phase | Tasks | What it does |
|-------|-------|-------------|
| 1 | 1.1–1.3 | Core types, state machine, engine hooks |
| 2 | 2.1–2.2 | Worker matching + WorkerManager |
| 3 | 3.1 | Memory adapter implementation |
| 4 | 4.1 | Redis adapter + Lua atomic claim |
| 5 | 5.1 | Postgres migration + worker events |
| 6 | 6.1 | Auth jti + workerId |
| 7 | 7.1–7.2 | Worker REST routes + WebSocket |
| 8 | 8.1–8.2 | Task route extension + app assembly |
| 9 | 9.1 | CLI integration |
| 10 | 10.1–10.4 | Tests |
| 11 | 11.1 | SSE terminal status fix |

**Total: 16 tasks across 11 phases.**
