# PostgreSQL Release Recovery Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make PostgreSQL-backed storage release use integer lifecycle timestamps and let an explicit release retry recover a task left in `releasing`.

**Architecture:** Preserve the existing durable release request as the crash-recovery anchor. Both engines recover the task before starting a new release; a recovered-cold result clears the request and returns immediately, while a recovered-hot result continues through the existing exact preconditions. Rust converts `SystemTime` to integer milliseconds before any lifecycle metadata write.

**Tech Stack:** TypeScript, Vitest, Rust, Tokio, sqlx, testcontainers, PostgreSQL, Changesets

---

### Task 1: TypeScript recovery-first explicit release

**Files:**
- Modify: `packages/core/tests/unit/storage-coordinator.test.ts`
- Modify: `packages/core/src/engine.ts:519-560`

- [ ] **Step 1: Write the failing engine regression test**

Append this test to the existing `StorageCoordinator` describe block:

```ts
it('recovers an interrupted release before retrying an explicit release', async () => {
  const hot = new MemoryShortTermStore()
  const durable = new CoordinatorLongTermStore()
  const engine = new TaskEngine({
    shortTermStore: hot,
    longTermStore: durable,
    broadcast: new MemoryBroadcastProvider(),
  })
  await engine.createTask({ id: 'task-1' })
  await engine.transitionTask('task-1', 'running')
  const event = await engine.publishEvent('task-1', {
    type: 'canary.event',
    level: 'info',
    data: { ok: true },
  })
  const preconditions = {
    expectedLastEventIndex: event.index,
    inactiveSince: event.timestamp,
  }
  await engine.releaseTaskStorage('task-1', preconditions)

  const cold = (await durable.getTaskStorageMetadata('task-1'))!
  durable.metadata.set('task-1', {
    ...cold,
    storageState: 'releasing',
    activeReleaseGeneration: 'interrupted-generation',
    coldAt: null,
  })

  await expect(engine.releaseTaskStorage('task-1', preconditions)).resolves.toMatchObject({
    taskId: 'task-1',
    storageState: 'cold',
    archiveWatermark: event.index,
    released: true,
  })
  expect(durable.releaseRequests.size).toBe(0)
  await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
    task: false,
    eventCount: 0,
    writeFence: false,
  })
})
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/storage-coordinator.test.ts \
  -t 'recovers an interrupted release before retrying an explicit release'
```

Expected: FAIL with `storage_busy` because `releaseTaskStorage` calls a new
release directly while durable metadata is `releasing`.

- [ ] **Step 3: Implement recovery-first ordering**

Replace the start of the existing `try` block in
`TaskEngine.releaseTaskStorage` with:

```ts
try {
  const recovery = await this.storageCoordinator.recoverTaskStorage(taskId)
  if (recovery.storageState === 'cold') {
    await durable.clearStorageReleaseRequest(request)
    return recovery
  }
  const result = await this.storageCoordinator.releaseTaskStorage(taskId, preconditions)
  await durable.clearStorageReleaseRequest(request)
  return result
} catch (error) {
  if (error instanceof StoragePreconditionError) {
    await durable.clearStorageReleaseRequest(request)
  }
  throw error
}
```

- [ ] **Step 4: Run the focused and package suites**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/storage-coordinator.test.ts
cd packages/core && pnpm test
```

Expected: all tests PASS.

- [ ] **Step 5: Commit the TypeScript behavior**

```bash
git add packages/core/src/engine.ts packages/core/tests/unit/storage-coordinator.test.ts
git commit -m "fix(core): recover interrupted explicit releases"
```

### Task 2: Rust recovery-first explicit release

**Files:**
- Modify: `rust/taskcast-core/tests/storage_release.rs`
- Modify: `rust/taskcast-core/src/engine.rs:669-717`

- [ ] **Step 1: Write the failing Rust engine regression test**

Add this test to `rust/taskcast-core/tests/storage_release.rs`:

```rust
#[tokio::test]
async fn engine_recovers_an_interrupted_release_before_explicit_retry() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    engine
        .create_task(CreateTaskInput {
            id: Some("task-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("task-1", TaskStatus::Running, None)
        .await
        .unwrap();
    let event = engine
        .publish_event(
            "task-1",
            PublishEventInput {
                r#type: "canary.event".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "ok": true }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    let release = || ReleasePreconditions {
        expected_last_event_index: event.index as i64,
        inactive_since: event.timestamp,
    };
    engine.release_task_storage("task-1", release()).await.unwrap();

    let cold = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    assert!(durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: StorageState::Cold,
            expected_storage_epoch: cold.storage_epoch,
            expected_release_generation: None,
            next: taskcast_core::TaskStorageMetadata {
                storage_state: StorageState::Releasing,
                active_release_generation: Some("interrupted-generation".to_string()),
                cold_at: None,
                ..cold
            },
        })
        .await
        .unwrap());

    let recovered = engine.release_task_storage("task-1", release()).await.unwrap();
    assert_eq!(recovered.storage_state, StorageState::Cold);
    assert!(recovered.released);
    assert!(durable
        .list_storage_release_requests(10)
        .await
        .unwrap()
        .is_empty());
    let presence = hot.get_task_storage_presence("task-1").await.unwrap();
    assert!(!presence.task);
    assert_eq!(presence.event_count, 0);
    assert!(!presence.write_fence);
}
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
cd rust
CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' \
  cargo test -p taskcast-core --test storage_release \
  engine_recovers_an_interrupted_release_before_explicit_retry
```

Expected: FAIL with `StorageBusyError`.

- [ ] **Step 3: Implement the same recovery-first ordering**

In `TaskEngine::release_task_storage`, after persisting the durable request and
before calling `coordinator.release_task_storage`, add:

```rust
let recovery = coordinator.recover_task_storage(task_id).await;
match recovery {
    Ok(result) if result.storage_state == StorageState::Cold => {
        durable.clear_storage_release_request(&request).await?;
        return Ok(result);
    }
    Ok(_) => {}
    Err(error) => return Err(EngineError::Store(error)),
}
```

Compare with `crate::types::StorageState::Cold`, matching the existing retry
sweeper code in the same file. Leave the current release result and
stale-precondition cleanup logic unchanged.

- [ ] **Step 4: Run the focused Rust suite**

Run:

```bash
cd rust
CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' \
  cargo test -p taskcast-core --test storage_release
```

Expected: all tests PASS.

- [ ] **Step 5: Commit the Rust behavior**

```bash
git add rust/taskcast-core/src/engine.rs rust/taskcast-core/tests/storage_release.rs
git commit -m "fix(rust): recover interrupted explicit releases"
```

### Task 3: Integer Rust lifecycle timestamps and PostgreSQL regression

**Files:**
- Modify: `rust/taskcast-core/src/storage_coordinator.rs:1411-1416`
- Modify: `rust/taskcast-postgres/tests/store_tests.rs`

- [ ] **Step 1: Add the deterministic clock regression test**

Add a pure conversion test at the end of
`rust/taskcast-core/src/storage_coordinator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn system_time_is_truncated_to_integer_milliseconds() {
        let time = UNIX_EPOCH + Duration::new(1, 123_456_789);
        assert_eq!(millis_since_epoch(time), 1_123.0);
        assert_eq!(millis_since_epoch(time).fract(), 0.0);
    }
}
```

- [ ] **Step 2: Run the unit test and verify RED**

Run:

```bash
cd rust
CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' \
  cargo test -p taskcast-core system_time_is_truncated_to_integer_milliseconds
```

Expected: compilation FAIL because `millis_since_epoch` does not exist.

- [ ] **Step 3: Implement integer millisecond conversion**

Replace the Rust clock helper with:

```rust
fn millis_since_epoch(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

fn now_millis() -> f64 {
    millis_since_epoch(SystemTime::now())
}
```

- [ ] **Step 4: Add a real PostgreSQL release regression**

Extend the imports in `rust/taskcast-postgres/tests/store_tests.rs` with
`HotWriteToken`, `MemoryShortTermStore`, `ReleasePreconditions`,
`ShortTermStore`, and `StorageCoordinator`, then add:

```rust
#[tokio::test]
async fn storage_coordinator_release_persists_integer_cold_timestamp() {
    let (store, _container) = setup().await;
    let durable = std::sync::Arc::new(store);
    let hot = std::sync::Arc::new(MemoryShortTermStore::new());
    let task = make_task("release-integer-time");
    hot.save_task(task.clone()).await.unwrap();
    durable.save_task(task).await.unwrap();
    let committed = hot
        .commit_event_fenced(
            "release-integer-time",
            make_event("release-integer-time", 0),
            &HotWriteToken {
                task_id: "release-integer-time".to_string(),
                storage_epoch: 1,
            },
        )
        .await
        .unwrap();
    durable.save_event(committed.event.clone()).await.unwrap();

    let released = StorageCoordinator::new(hot, durable.clone())
        .release_task_storage(
            "release-integer-time",
            ReleasePreconditions {
                expected_last_event_index: committed.event.index as i64,
                inactive_since: committed.event.timestamp,
            },
        )
        .await
        .unwrap();

    assert_eq!(released.storage_state, StorageState::Cold);
    let metadata = durable
        .get_task_storage_metadata("release-integer-time")
        .await
        .unwrap()
        .unwrap();
    let cold_at = metadata.cold_at.unwrap();
    assert_eq!(cold_at.fract(), 0.0);
}
```

- [ ] **Step 5: Run unit and PostgreSQL integration tests**

Run:

```bash
cd rust
CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' \
  cargo test -p taskcast-core system_time_is_truncated_to_integer_milliseconds
CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' \
  cargo test -p taskcast-postgres --test store_tests \
  storage_coordinator_release_persists_integer_cold_timestamp
```

Expected: both tests PASS. The PostgreSQL test may use
`TASKCAST_TEST_POSTGRES_URL`; otherwise testcontainers must start PostgreSQL.

- [ ] **Step 6: Commit the timestamp fix**

```bash
git add rust/taskcast-core/src/storage_coordinator.rs \
  rust/taskcast-postgres/tests/store_tests.rs
git commit -m "fix(postgres): persist integer release timestamps"
```

### Task 4: Patch changeset and full verification

**Files:**
- Create: `.changeset/postgres-release-recovery.md`

- [ ] **Step 1: Add the patch changeset**

```md
---
"@taskcast/core": patch
"@taskcast/server": patch
"@taskcast/postgres": patch
"@taskcast/cli": patch
---

Recover interrupted explicit storage releases before retrying and ensure Rust
persists integer PostgreSQL lifecycle timestamps.
```

- [ ] **Step 2: Run repository verification**

Run:

```bash
pnpm test
pnpm lint
pnpm build
cd rust
CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' cargo test --workspace
CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

Expected: every command exits 0. Build `packages/playground/dist` before the
Rust workspace commands if the embedded CLI asset directory is absent.

- [ ] **Step 3: Commit release metadata**

```bash
git add .changeset/postgres-release-recovery.md
git commit -m "chore: add release recovery changeset"
```

### Task 5: Review, publish, and development canary

**Files:**
- No additional source files expected.

- [ ] **Step 1: Request independent code review**

Review the complete diff against
`docs/superpowers/specs/2026-07-28-postgres-release-recovery-fix-design.md`.
Resolve every Critical or Important finding and rerun affected tests.

- [ ] **Step 2: Push and open the Taskcast pull request**

```bash
git push -u origin codex/postgres-release-recovery-fix
gh pr create \
  --title "fix: recover interrupted PostgreSQL releases" \
  --body $'## Summary\n- recover interrupted explicit releases before retrying\n- persist integer Rust lifecycle timestamps\n- cover the PostgreSQL failure boundary\n\n## Verification\n- pnpm test / lint / build\n- cargo test / clippy / fmt'
```

Expected: PR contains the design, implementation, regression tests, and patch
changeset. All GitHub Actions checks pass.

- [ ] **Step 3: Merge feature and release PRs**

Merge the verified feature PR. Wait for the generated "Release Packages" PR,
verify its version/changelog diff and CI, then merge it. Verify npm packages,
GitHub release binaries, Docker manifests, and public version metadata for the
new fixed version.

- [ ] **Step 4: Deploy only development**

Update the development GitOps image to the new immutable linux/amd64 digest.
Keep:

```yaml
TASKCAST_AUTO_MIGRATE: "false"
TASKCAST_HOT_RETENTION_ENABLED: "false"
```

Do not change staging or production.

- [ ] **Step 5: Recover and verify the existing canary**

Repeat the exact operator request for
`taskcast-dev-storage-canary-20260728T025637Z`:

```json
{
  "expectedLastEventIndex": 4,
  "inactiveSince": 1785207528959
}
```

Expected:

- HTTP 200 with `storageState: "cold"` and `archiveWatermark: 4`.
- PostgreSQL has no pending release request and the finalized receipt remains.
- Canonical history indexes remain `[0, 2, 3, 4]`.
- Terminal SSE raw indexes remain `[0, 2, 3, 4]`.
- Task-local Redis key count remains zero after repeated reads.
- `/health/detail` remains release-ready at storage protocol version 2.
