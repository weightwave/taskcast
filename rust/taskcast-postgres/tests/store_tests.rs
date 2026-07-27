use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

use taskcast_core::archive::{
    compute_archive_batch_digest, compute_archive_source_digest,
    compute_archive_source_page_digest, compute_series_state_digest,
};
use taskcast_core::types::{
    ArchiveBatch, ArchiveBatchReceipt, ArchiveGeneration, ArchiveGenerationStatus,
    ArchiveSourceManifest, DurableSeriesState, EventQueryOptions, Level, LongTermStore, SeriesMode,
    SinceCursor, StorageReleaseRequest, StorageState, Task, TaskEvent, TaskStatus,
    TaskStorageMetadataCas, WorkerAssignment, WorkerAssignmentStatus, WorkerAuditAction,
    WorkerAuditEvent,
};
use taskcast_postgres::PostgresLongTermStore;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SCHEMA_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn storage_lifecycle_migration_avoids_event_table_rewrite() {
    let migration = include_str!("../../../migrations/postgres/003_storage_lifecycle.sql");
    let receipt_upgrade =
        include_str!("../../../migrations/postgres/004_archive_receipt_coverage.sql");
    let creation_claim = include_str!("../../../migrations/postgres/005_task_creation_claim.sql");
    assert!(migration.contains("storage_state"));
    assert!(migration.contains("archive_watermark"));
    assert!(migration.contains("execution_deadline_at"));
    assert!(migration.contains("taskcast_archive_generations"));
    assert!(migration.contains("taskcast_archive_batches"));
    assert!(migration.contains("taskcast_series_state"));
    assert!(migration.contains("taskcast_durable_assignments"));
    assert!(migration.contains("taskcast_terminal_outbox"));
    assert!(!migration.contains("ALTER TABLE taskcast_events"));
    assert!(migration.contains("previous_digest TEXT NOT NULL"));
    assert!(receipt_upgrade.contains("ALTER COLUMN previous_digest DROP NOT NULL"));
    assert!(receipt_upgrade.contains("series_coverage"));
    assert!(creation_claim.contains("creation_token"));
    assert!(creation_claim.contains("creation_claimed_at"));
    assert!(creation_claim.contains("creation_claim_expires_at"));
    assert!(creation_claim.contains("creation_completed_at"));
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn setup() -> (
    PostgresLongTermStore,
    Option<testcontainers::ContainerAsync<Postgres>>,
) {
    if let Ok(database_url) = std::env::var("TASKCAST_TEST_POSTGRES_URL") {
        let schema = format!(
            "taskcast_test_{}_{}",
            std::process::id(),
            TEST_SCHEMA_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;

        let connection_schema = schema.clone();
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |connection, _| {
                let schema = connection_schema.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {schema}"))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .unwrap();
        let store = PostgresLongTermStore::new(pool);
        store.migrate().await.unwrap();
        return (store, None);
    }

    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();
    (store, Some(container))
}

fn make_task(id: &str) -> Task {
    Task {
        id: id.to_string(),
        r#type: None,
        status: TaskStatus::Pending,
        params: Some(
            [("prompt".to_string(), serde_json::json!("hello"))]
                .into_iter()
                .collect(),
        ),
        result: None,
        error: None,
        metadata: None,
        auth_config: None,
        webhooks: None,
        cleanup: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
        created_at: 1000.0,
        updated_at: 1000.0,
        completed_at: None,
        ttl: None,
    }
}

fn make_event(task_id: &str, index: u64) -> TaskEvent {
    TaskEvent {
        id: format!("evt-{}-{}", task_id, index),
        task_id: task_id.to_string(),
        index,
        timestamp: 1000.0 + index as f64 * 100.0,
        r#type: "llm.delta".to_string(),
        level: Level::Info,
        data: serde_json::json!({"text": format!("msg-{}", index)}),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

fn make_ttl_task(id: &str) -> Task {
    let mut task = make_task(id);
    task.status = TaskStatus::Running;
    task.ttl = Some(60);
    task
}

fn make_assignment(task_id: &str) -> WorkerAssignment {
    WorkerAssignment {
        task_id: task_id.to_string(),
        worker_id: "worker-1".to_string(),
        cost: 3,
        assigned_at: 2_000.0,
        status: WorkerAssignmentStatus::Running,
    }
}

fn make_timeout_event(task_id: &str, index: u64) -> TaskEvent {
    TaskEvent {
        id: format!("timeout-{task_id}"),
        task_id: task_id.to_string(),
        index,
        timestamp: 3_000.0,
        r#type: "taskcast:status".to_string(),
        level: Level::Info,
        data: serde_json::json!({"status": "timeout"}),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

async fn make_overdue(
    store: &PostgresLongTermStore,
    task: Task,
    claim_ttl_ms: u64,
) -> taskcast_core::types::TtlClaim {
    let task_id = task.id.clone();
    store.save_task(task).await.unwrap();
    let metadata = store
        .get_task_storage_metadata(&task_id)
        .await
        .unwrap()
        .unwrap();
    let mut overdue = metadata.clone();
    overdue.execution_deadline_at = Some(0.0);
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id,
            expected_storage_state: metadata.storage_state,
            expected_storage_epoch: metadata.storage_epoch,
            expected_release_generation: metadata.active_release_generation,
            next: overdue,
        })
        .await
        .unwrap());
    store
        .claim_overdue_tasks(1, claim_ttl_ms)
        .await
        .unwrap()
        .remove(0)
}

#[tokio::test]
async fn ttl_deadline_uses_database_time_and_paused_tasks_are_suspended() {
    let (store, _container) = setup().await;
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    store
        .save_task(make_ttl_task("ttl-deadline"))
        .await
        .unwrap();
    let created = store
        .get_task_storage_metadata("ttl-deadline")
        .await
        .unwrap()
        .unwrap();
    assert!(created.execution_deadline_at.unwrap() >= before + 59_000.0);
    assert_eq!(created.task_version, 0);

    let mut paused = make_ttl_task("ttl-deadline");
    paused.status = TaskStatus::Paused;
    paused.updated_at = 2_000.0;
    store.save_task(paused).await.unwrap();
    let suspended = store
        .get_task_storage_metadata("ttl-deadline")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(suspended.execution_deadline_at, None);
    assert_eq!(suspended.task_version, 1);

    let mut resumed = make_ttl_task("ttl-deadline");
    resumed.updated_at = 3_000.0;
    store.save_task(resumed).await.unwrap();
    let resumed = store
        .get_task_storage_metadata("ttl-deadline")
        .await
        .unwrap()
        .unwrap();
    assert!(resumed.execution_deadline_at.unwrap() >= before + 59_000.0);
    assert_eq!(resumed.task_version, 2);
}

#[tokio::test]
async fn ttl_claim_is_exclusive_until_it_expires() {
    let (store, _container) = setup().await;
    let first = make_overdue(&store, make_ttl_task("ttl-claim"), 20).await;
    assert!(store.claim_overdue_tasks(1, 20).await.unwrap().is_empty());
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    let second = store.claim_overdue_tasks(1, 20).await.unwrap().remove(0);
    assert_eq!(second.task_id, first.task_id);
    assert_eq!(second.task_version, first.task_version);
    assert_ne!(second.claim_token, first.claim_token);
}

#[tokio::test]
async fn ttl_terminalization_commits_task_event_assignment_and_outbox_atomically() {
    let (store, _container) = setup().await;
    let claim = make_overdue(&store, make_ttl_task("ttl-terminalize"), 20).await;
    let assignment = make_assignment("ttl-terminalize");
    store
        .save_durable_assignment(assignment.clone())
        .await
        .unwrap();
    let mut timeout = make_ttl_task("ttl-terminalize");
    timeout.status = TaskStatus::Timeout;
    timeout.updated_at = 3_000.0;
    timeout.completed_at = Some(3_000.0);
    let event = make_timeout_event("ttl-terminalize", 0);
    let projection = store
        .terminalize_ttl_claim(
            claim.clone(),
            timeout.clone(),
            event.clone(),
            Some(assignment),
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        store
            .get_task("ttl-terminalize")
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Timeout
    );
    assert_eq!(
        store.get_events("ttl-terminalize", None).await.unwrap(),
        vec![event]
    );
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    let claimed = store
        .claim_terminal_projections(1, "projector", 30_000)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].projection_id, projection.projection_id);
    store
        .complete_terminal_projection(&claimed[0])
        .await
        .unwrap();
    assert!(store
        .claim_terminal_projections(1, "next-projector", 30_000)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn ttl_terminalization_loses_to_version_and_terminal_races() {
    let (store, _container) = setup().await;
    let version_claim = make_overdue(&store, make_ttl_task("ttl-version-race"), 30_000).await;
    let mut blocked = make_ttl_task("ttl-version-race");
    blocked.status = TaskStatus::Blocked;
    blocked.updated_at = 2_000.0;
    store.save_task(blocked).await.unwrap();
    let mut timeout = make_ttl_task("ttl-version-race");
    timeout.status = TaskStatus::Timeout;
    timeout.completed_at = Some(3_000.0);
    timeout.updated_at = 3_000.0;
    assert!(store
        .terminalize_ttl_claim(
            version_claim,
            timeout,
            make_timeout_event("ttl-version-race", 0),
            None,
        )
        .await
        .unwrap()
        .is_none());

    let terminal_claim = make_overdue(&store, make_ttl_task("ttl-terminal-race"), 30_000).await;
    let mut completed = make_ttl_task("ttl-terminal-race");
    completed.status = TaskStatus::Completed;
    completed.completed_at = Some(2_000.0);
    completed.updated_at = 2_000.0;
    store.save_task(completed).await.unwrap();
    let mut timeout = make_ttl_task("ttl-terminal-race");
    timeout.status = TaskStatus::Timeout;
    timeout.completed_at = Some(3_000.0);
    timeout.updated_at = 3_000.0;
    assert!(store
        .terminalize_ttl_claim(
            terminal_claim,
            timeout,
            make_timeout_event("ttl-terminal-race", 0),
            None,
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .get_task("ttl-terminal-race")
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Completed
    );
}

#[tokio::test]
async fn durable_assignment_delete_compares_the_assignment_identity() {
    let (store, _container) = setup().await;
    let claim = make_overdue(&store, make_ttl_task("ttl-assignment"), 30_000).await;
    let assignment = make_assignment("ttl-assignment");
    store
        .save_durable_assignment(assignment.clone())
        .await
        .unwrap();
    store
        .delete_durable_assignment("ttl-assignment", Some("wrong-assignment"))
        .await
        .unwrap();
    let mut timeout = make_ttl_task("ttl-assignment");
    timeout.status = TaskStatus::Timeout;
    timeout.completed_at = Some(3_000.0);
    timeout.updated_at = 3_000.0;
    assert!(store
        .terminalize_ttl_claim(
            claim,
            timeout,
            make_timeout_event("ttl-assignment", 0),
            Some(assignment),
        )
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn ttl_claim_methods_reject_invalid_bounds() {
    let (store, _container) = setup().await;
    assert!(store.claim_overdue_tasks(0, 30_000).await.is_err());
    assert!(store.claim_overdue_tasks(1, 0).await.is_err());
    assert!(store
        .claim_terminal_projections(0, "projector", 30_000)
        .await
        .is_err());
    assert!(store
        .claim_terminal_projections(1, "", 30_000)
        .await
        .is_err());
    assert!(store
        .claim_terminal_projections(1, "projector", 0)
        .await
        .is_err());
}

async fn start_archive_release(store: &PostgresLongTermStore, generation: &str) {
    store.save_task(make_task("task-archive")).await.unwrap();
    let metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    let mut next = metadata.clone();
    next.storage_state = StorageState::Releasing;
    next.active_release_generation = Some(generation.to_string());
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next,
        })
        .await
        .unwrap());
}

#[tokio::test]
async fn storage_release_request_is_persisted_and_compare_cleared() {
    let (store, _container) = setup().await;
    store.save_task(make_task("release-request")).await.unwrap();
    let request = StorageReleaseRequest {
        task_id: "release-request".to_string(),
        requested_at: 2_000.0,
        expected_last_event_index: 7,
        inactive_since: 1_500.0,
    };

    assert!(store
        .persist_storage_release_request(request.clone())
        .await
        .unwrap());
    assert_eq!(
        store.list_storage_release_requests(10).await.unwrap(),
        vec![request.clone()]
    );
    assert!(!store
        .clear_storage_release_request(&StorageReleaseRequest {
            requested_at: request.requested_at + 1.0,
            ..request.clone()
        })
        .await
        .unwrap());
    assert_eq!(
        store.list_storage_release_requests(10).await.unwrap(),
        vec![request.clone()]
    );
    assert!(store.clear_storage_release_request(&request).await.unwrap());
    assert!(store
        .list_storage_release_requests(10)
        .await
        .unwrap()
        .is_empty());
    assert!(!store
        .persist_storage_release_request(StorageReleaseRequest {
            task_id: "missing".to_string(),
            ..request
        })
        .await
        .unwrap());
    assert!(store.list_storage_release_requests(0).await.is_err());
}

fn build_archive(
    events: Vec<TaskEvent>,
    generation: &str,
) -> (ArchiveGeneration, Vec<ArchiveBatch>) {
    build_archive_with_series(events, vec![], generation)
}

fn build_archive_with_series(
    events: Vec<TaskEvent>,
    series_latest: Vec<DurableSeriesState>,
    generation: &str,
) -> (ArchiveGeneration, Vec<ArchiveBatch>) {
    let page_digests = events
        .iter()
        .cloned()
        .map(|event| compute_archive_source_page_digest(&[event]).unwrap())
        .collect::<Vec<_>>();
    let target_watermark = events.last().map(|event| event.index as i64).unwrap_or(-1);
    let manifest = ArchiveSourceManifest {
        prior_watermark: -1,
        target_watermark,
        source_entry_count: events.len() as u64,
        source_digest: compute_archive_source_digest(&page_digests),
        series_state_digest: compute_series_state_digest(&series_latest).unwrap(),
        expected_batch_ordinals: (0..events.len() as u64).collect(),
    };
    let archive = ArchiveGeneration {
        task_id: "task-archive".to_string(),
        generation: generation.to_string(),
        storage_epoch: 1,
        target_watermark,
        manifest,
        status: ArchiveGenerationStatus::Open,
        created_at: 3000.0,
        updated_at: 3000.0,
    };
    let mut previous_batch_digest = None;
    let mut batches = Vec::new();
    for (ordinal, event) in events.into_iter().enumerate() {
        let page_series = vec![];
        let batch_digest = compute_archive_batch_digest(
            previous_batch_digest.as_deref(),
            &[event.clone()],
            &page_series,
        )
        .unwrap();
        let receipt = ArchiveBatchReceipt {
            task_id: "task-archive".to_string(),
            generation: generation.to_string(),
            ordinal: ordinal as u64,
            previous_batch_digest: previous_batch_digest.clone(),
            batch_digest: batch_digest.clone(),
            entry_count: 1,
            first_index: Some(event.index),
            last_index: Some(event.index),
        };
        batches.push(ArchiveBatch {
            receipt,
            events: vec![event],
            series_latest: page_series,
        });
        previous_batch_digest = Some(batch_digest);
    }
    (archive, batches)
}

// ─── archive barrier ──────────────────────────────────────────────────────

#[tokio::test]
async fn archive_metadata_cas_rejects_non_monotonic_or_inconsistent_updates() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-archive")).await.unwrap();
    let metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();

    let mut invalid_epoch = metadata.clone();
    invalid_epoch.storage_epoch = 0;
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: invalid_epoch,
        })
        .await
        .is_err());

    let mut invalid_generation = metadata;
    invalid_generation.active_release_generation = Some("generation-without-release".to_string());
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: invalid_generation,
        })
        .await
        .is_err());

    let mut invalid_watermark = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    invalid_watermark.archive_watermark = 0;
    assert!(!store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: invalid_watermark,
        })
        .await
        .unwrap());
}

#[tokio::test]
async fn archive_metadata_cas_allows_a_cold_rehydration_generation() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-archive")).await.unwrap();
    let metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    let mut cold = metadata;
    cold.storage_state = StorageState::Cold;
    cold.active_release_generation = Some("rehydration-generation".to_string());

    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: cold,
        })
        .await
        .unwrap());
}

#[tokio::test]
async fn archive_watermark_is_monotonic_across_generations_and_metadata_cas() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (first, first_batches) = build_archive(vec![make_event("task-archive", 0)], "generation-1");
    store.begin_archive(first).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", first_batches[0].clone())
        .await
        .unwrap();
    store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![],
        )
        .await
        .unwrap();

    let mut metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    let expected_epoch = metadata.storage_epoch;
    metadata.storage_state = StorageState::Hot;
    metadata.active_release_generation = None;
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Releasing,
            expected_storage_epoch: expected_epoch,
            expected_release_generation: Some("generation-1".to_string()),
            next: metadata,
        })
        .await
        .unwrap());
    let mut metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    metadata.storage_state = StorageState::Releasing;
    metadata.active_release_generation = Some("generation-2".to_string());
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: expected_epoch,
            expected_release_generation: None,
            next: metadata,
        })
        .await
        .unwrap());

    let (mut second, second_batches) =
        build_archive(vec![make_event("task-archive", 1)], "generation-2");
    second.manifest.prior_watermark = 0;
    store.begin_archive(second).await.unwrap();
    store
        .archive_batch("task-archive", "generation-2", second_batches[0].clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .finalize_archive(
                "task-archive",
                "generation-2",
                make_task("task-archive"),
                vec![],
            )
            .await
            .unwrap(),
        1
    );

    let mut metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    metadata.archive_watermark = 0;
    assert!(!store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Releasing,
            expected_storage_epoch: expected_epoch,
            expected_release_generation: Some("generation-2".to_string()),
            next: metadata,
        })
        .await
        .unwrap());
    assert_eq!(
        store.get_archive_watermark("task-archive").await.unwrap(),
        1
    );
}

#[tokio::test]
async fn archive_finalizes_an_empty_source() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (archive, batches) = build_archive(vec![], "generation-1");
    assert!(batches.is_empty());
    store.begin_archive(archive).await.unwrap();
    assert_eq!(
        store
            .finalize_archive(
                "task-archive",
                "generation-1",
                make_task("task-archive"),
                vec![],
            )
            .await
            .unwrap(),
        -1
    );
    assert!(store
        .get_events("task-archive", None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn archive_rejects_an_empty_source_that_advances_the_watermark() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (mut archive, batches) = build_archive(vec![], "generation-1");
    assert!(batches.is_empty());
    archive.target_watermark = 0;
    archive.manifest.target_watermark = 0;

    assert!(store.begin_archive(archive).await.is_err());
}

#[tokio::test]
async fn archive_rejects_uncovered_or_out_of_bounds_compact_source() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut compact_event = make_event("task-archive", 0);
    compact_event.series_id = Some("output".to_string());
    compact_event.series_mode = Some(SeriesMode::Latest);
    let (missing, missing_batches) = build_archive(vec![compact_event.clone()], "generation-1");
    store.begin_archive(missing).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", missing_batches[0].clone())
        .await
        .unwrap();
    assert!(store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![],
        )
        .await
        .is_err());

    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let out_of_bounds = DurableSeriesState {
        task_id: "task-archive".to_string(),
        series_id: "output".to_string(),
        mode: SeriesMode::Latest,
        event: compact_event.clone(),
        through_index: 1,
    };
    let (archive, batches) = build_archive_with_series(
        vec![compact_event],
        vec![out_of_bounds.clone()],
        "generation-1",
    );
    store.begin_archive(archive).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .unwrap();
    assert!(store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![out_of_bounds],
        )
        .await
        .is_err());
}

#[tokio::test]
async fn archive_does_not_delete_a_compact_identity_conflict() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut old_event = make_event("task-archive", 0);
    old_event.data = serde_json::json!({ "text": "old" });
    old_event.series_id = Some("output".to_string());
    old_event.series_mode = Some(SeriesMode::Latest);
    store.save_event(old_event.clone()).await.unwrap();

    let mut replacement = old_event.clone();
    replacement.data = serde_json::json!({ "text": "replacement" });
    let state = DurableSeriesState {
        task_id: "task-archive".to_string(),
        series_id: "output".to_string(),
        mode: SeriesMode::Latest,
        event: replacement.clone(),
        through_index: 0,
    };
    let (archive, batches) =
        build_archive_with_series(vec![replacement], vec![state.clone()], "generation-1");
    store.begin_archive(archive).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .unwrap();

    assert!(store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![state],
        )
        .await
        .is_err());
    assert_eq!(
        store.get_events("task-archive", None).await.unwrap(),
        vec![old_event]
    );
}

#[tokio::test]
async fn archive_finalization_is_complete_and_idempotent() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (archive, batches) = build_archive(
        vec![make_event("task-archive", 0), make_event("task-archive", 1)],
        "generation-1",
    );

    assert_eq!(store.begin_archive(archive.clone()).await.unwrap(), archive);
    for batch in &batches {
        assert_eq!(
            store
                .archive_batch("task-archive", "generation-1", batch.clone())
                .await
                .unwrap(),
            batch.receipt
        );
    }
    assert_eq!(
        store
            .finalize_archive(
                "task-archive",
                "generation-1",
                make_task("task-archive"),
                vec![],
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.begin_archive(archive).await.unwrap().status,
        ArchiveGenerationStatus::Finalized
    );
    assert_eq!(
        store
            .archive_batch("task-archive", "generation-1", batches[0].clone())
            .await
            .unwrap(),
        batches[0].receipt
    );
    assert_eq!(
        store
            .finalize_archive(
                "task-archive",
                "generation-1",
                make_task("task-archive"),
                vec![],
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.get_archive_watermark("task-archive").await.unwrap(),
        1
    );
    assert_eq!(store.get_last_event_index("task-archive").await.unwrap(), 1);
}

#[tokio::test]
async fn archive_keeps_compact_coverage_bounded_across_accumulate_batches() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut first = make_event("task-archive", 0);
    first.data = serde_json::json!({ "delta": "hello" });
    first.series_id = Some("output".to_string());
    first.series_mode = Some(SeriesMode::Accumulate);
    first.series_acc_field = Some("delta".to_string());
    let mut second = make_event("task-archive", 1);
    second.data = serde_json::json!({ "delta": " world" });
    second.series_id = Some("output".to_string());
    second.series_mode = Some(SeriesMode::Accumulate);
    second.series_acc_field = Some("delta".to_string());
    let mut final_event = second.clone();
    final_event.data = serde_json::json!({ "delta": "hello world" });
    let final_state = DurableSeriesState {
        task_id: "task-archive".to_string(),
        series_id: "output".to_string(),
        mode: SeriesMode::Accumulate,
        event: final_event.clone(),
        through_index: 1,
    };
    store
        .accumulate_series("task-archive", "output", first.clone(), "delta")
        .await
        .unwrap();
    store
        .accumulate_series("task-archive", "output", second.clone(), "delta")
        .await
        .unwrap();
    let (archive, batches) = build_archive_with_series(
        vec![first, second.clone()],
        vec![final_state.clone()],
        "generation-1",
    );
    store.begin_archive(archive).await.unwrap();
    for batch in &batches {
        assert!(batch.series_latest.is_empty());
        store
            .archive_batch("task-archive", "generation-1", batch.clone())
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .finalize_archive(
                "task-archive",
                "generation-1",
                make_task("task-archive"),
                vec![final_state],
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.get_events("task-archive", None).await.unwrap(),
        vec![final_event.clone()]
    );
    assert_eq!(
        store
            .accumulate_series("task-archive", "output", second, "delta")
            .await
            .unwrap(),
        final_event
    );
}

#[tokio::test]
async fn archive_finalizes_caught_up_latest_and_ignores_a_delayed_write() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut first = make_event("task-archive", 0);
    first.data = serde_json::json!({ "status": "starting" });
    first.series_id = Some("status".to_string());
    first.series_mode = Some(SeriesMode::Latest);
    let mut second = make_event("task-archive", 1);
    second.data = serde_json::json!({ "status": "ready" });
    second.series_id = Some("status".to_string());
    second.series_mode = Some(SeriesMode::Latest);
    store
        .replace_last_series_event("task-archive", "status", first.clone())
        .await
        .unwrap();
    store
        .replace_last_series_event("task-archive", "status", second.clone())
        .await
        .unwrap();
    let final_state = DurableSeriesState {
        task_id: "task-archive".to_string(),
        series_id: "status".to_string(),
        mode: SeriesMode::Latest,
        event: second.clone(),
        through_index: 1,
    };
    let (archive, batches) = build_archive_with_series(
        vec![first.clone(), second.clone()],
        vec![final_state.clone()],
        "generation-1",
    );
    store.begin_archive(archive).await.unwrap();
    for batch in batches {
        store
            .archive_batch("task-archive", "generation-1", batch)
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .finalize_archive(
                "task-archive",
                "generation-1",
                make_task("task-archive"),
                vec![final_state],
            )
            .await
            .unwrap(),
        1
    );

    store
        .replace_last_series_event("task-archive", "status", first)
        .await
        .unwrap();
    assert_eq!(
        store.get_events("task-archive", None).await.unwrap(),
        vec![second]
    );
}

#[tokio::test]
async fn archive_rejects_final_state_that_omits_a_legacy_compact_row() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut legacy = make_event("task-archive", 0);
    legacy.series_id = Some("legacy-output".to_string());
    legacy.series_mode = Some(SeriesMode::Latest);
    store.save_event(legacy).await.unwrap();
    let (archive, _) = build_archive(vec![], "generation-1");
    store.begin_archive(archive).await.unwrap();

    assert!(store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![],
        )
        .await
        .is_err());
}

#[tokio::test]
async fn archive_does_not_regress_a_newer_legacy_compact_row() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut legacy_newer = make_event("task-archive", 1);
    legacy_newer.series_id = Some("legacy-output".to_string());
    legacy_newer.series_mode = Some(SeriesMode::Latest);
    store.save_event(legacy_newer.clone()).await.unwrap();
    let mut source = make_event("task-archive", 0);
    source.series_id = Some("legacy-output".to_string());
    source.series_mode = Some(SeriesMode::Latest);
    let final_state = DurableSeriesState {
        task_id: "task-archive".to_string(),
        series_id: "legacy-output".to_string(),
        mode: SeriesMode::Latest,
        event: source.clone(),
        through_index: 0,
    };
    let (archive, batches) =
        build_archive_with_series(vec![source], vec![final_state.clone()], "generation-1");
    store.begin_archive(archive).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .unwrap();

    assert!(store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![final_state],
        )
        .await
        .is_err());
    assert_eq!(
        store.get_events("task-archive", None).await.unwrap(),
        vec![legacy_newer]
    );
}

#[tokio::test]
async fn archive_rejects_reordered_and_broken_batches() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (archive, batches) = build_archive(
        vec![make_event("task-archive", 0), make_event("task-archive", 1)],
        "generation-1",
    );
    store.begin_archive(archive).await.unwrap();

    assert!(store
        .archive_batch("task-archive", "generation-1", batches[1].clone())
        .await
        .is_err());
    store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .unwrap();
    assert!(store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![],
        )
        .await
        .is_err());
    let mut broken = batches[1].clone();
    broken.receipt.previous_batch_digest = Some("f".repeat(64));
    broken.receipt.batch_digest = compute_archive_batch_digest(
        broken.receipt.previous_batch_digest.as_deref(),
        &broken.events,
        &broken.series_latest,
    )
    .unwrap();
    assert!(store
        .archive_batch("task-archive", "generation-1", broken)
        .await
        .is_err());

    let mut overlapping = batches[1].clone();
    let mut overlapping_event = make_event("task-archive", 0);
    overlapping_event.id = "overlapping-event".to_string();
    overlapping.events = vec![overlapping_event];
    overlapping.receipt.first_index = Some(0);
    overlapping.receipt.last_index = Some(0);
    overlapping.receipt.batch_digest = compute_archive_batch_digest(
        overlapping.receipt.previous_batch_digest.as_deref(),
        &overlapping.events,
        &overlapping.series_latest,
    )
    .unwrap();
    assert!(store
        .archive_batch("task-archive", "generation-1", overlapping)
        .await
        .is_err());
}

#[tokio::test]
async fn archive_rejects_a_conflicting_begin_replay() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (archive, _) = build_archive(vec![make_event("task-archive", 0)], "generation-1");
    store.begin_archive(archive.clone()).await.unwrap();
    let mut conflict = archive;
    conflict.manifest.source_entry_count = 2;

    assert!(store.begin_archive(conflict).await.is_err());
}

#[tokio::test]
async fn archive_rejects_changed_content_for_an_idempotent_receipt() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (archive, batches) = build_archive(vec![make_event("task-archive", 0)], "generation-1");
    store.begin_archive(archive).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .unwrap();
    let mut changed = batches[0].clone();
    changed.events[0].data = serde_json::json!({ "text": "changed" });
    changed.receipt.batch_digest = compute_archive_batch_digest(
        changed.receipt.previous_batch_digest.as_deref(),
        &changed.events,
        &changed.series_latest,
    )
    .unwrap();

    assert!(store
        .archive_batch("task-archive", "generation-1", changed)
        .await
        .is_err());
}

#[tokio::test]
async fn archive_rejects_event_identity_conflicts() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut conflict = make_event("task-archive", 0);
    conflict.id = "different-event".to_string();
    store.save_event(conflict).await.unwrap();
    let (archive, batches) = build_archive(vec![make_event("task-archive", 0)], "generation-1");
    store.begin_archive(archive).await.unwrap();

    assert!(store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .is_err());
    assert_eq!(
        store.get_archive_watermark("task-archive").await.unwrap(),
        -1
    );
}

#[tokio::test]
async fn archive_allows_only_one_canonical_payload_when_stale_generations_race() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-a").await;
    let (first, first_batches) = build_archive(vec![make_event("task-archive", 0)], "generation-a");
    store.begin_archive(first).await.unwrap();

    let metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    let mut next = metadata;
    next.active_release_generation = Some("generation-b".to_string());
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Releasing,
            expected_storage_epoch: 1,
            expected_release_generation: Some("generation-a".to_string()),
            next,
        })
        .await
        .unwrap());

    let mut conflicting = make_event("task-archive", 0);
    conflicting.data = serde_json::json!({ "text": "conflicting" });
    let (second, second_batches) = build_archive(vec![conflicting], "generation-b");
    store.begin_archive(second).await.unwrap();

    let (first_result, second_result) = tokio::join!(
        store.archive_batch("task-archive", "generation-a", first_batches[0].clone()),
        store.archive_batch("task-archive", "generation-b", second_batches[0].clone())
    );
    assert_ne!(first_result.is_ok(), second_result.is_ok());
    assert_eq!(
        store.get_archive_watermark("task-archive").await.unwrap(),
        -1
    );
}

#[tokio::test]
async fn archive_rejects_a_stale_keep_all_row_conflicting_with_compact_source() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-a").await;
    let (stale, stale_batches) = build_archive(vec![make_event("task-archive", 0)], "generation-a");
    store.begin_archive(stale).await.unwrap();
    store
        .archive_batch("task-archive", "generation-a", stale_batches[0].clone())
        .await
        .unwrap();

    let metadata = store
        .get_task_storage_metadata("task-archive")
        .await
        .unwrap()
        .unwrap();
    let mut next = metadata;
    next.active_release_generation = Some("generation-b".to_string());
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-archive".to_string(),
            expected_storage_state: StorageState::Releasing,
            expected_storage_epoch: 1,
            expected_release_generation: Some("generation-a".to_string()),
            next,
        })
        .await
        .unwrap());

    let mut first = make_event("task-archive", 0);
    first.series_id = Some("status".to_string());
    first.series_mode = Some(SeriesMode::Latest);
    let mut second = make_event("task-archive", 1);
    second.series_id = Some("status".to_string());
    second.series_mode = Some(SeriesMode::Latest);
    let final_state = DurableSeriesState {
        task_id: "task-archive".to_string(),
        series_id: "status".to_string(),
        mode: SeriesMode::Latest,
        event: second.clone(),
        through_index: 1,
    };
    let (active, active_batches) =
        build_archive_with_series(vec![first, second], vec![final_state], "generation-b");
    store.begin_archive(active).await.unwrap();

    assert!(store
        .archive_batch("task-archive", "generation-b", active_batches[0].clone(),)
        .await
        .is_err());
    assert_eq!(
        store.get_archive_watermark("task-archive").await.unwrap(),
        -1
    );
}

#[tokio::test]
async fn archive_rejects_wrong_manifest_digest_at_finalize() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let (mut archive, batches) = build_archive(vec![make_event("task-archive", 0)], "generation-1");
    archive.manifest.source_digest = "0".repeat(64);
    store.begin_archive(archive).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .unwrap();

    assert!(store
        .finalize_archive(
            "task-archive",
            "generation-1",
            make_task("task-archive"),
            vec![],
        )
        .await
        .is_err());
    assert_eq!(
        store.get_archive_watermark("task-archive").await.unwrap(),
        -1
    );
}

#[tokio::test]
async fn archive_rejects_wrong_source_count_and_series_digest() {
    for wrong_count in [true, false] {
        let (store, _container) = setup().await;
        start_archive_release(&store, "generation-1").await;
        let (mut archive, batches) =
            build_archive(vec![make_event("task-archive", 0)], "generation-1");
        if wrong_count {
            archive.manifest.source_entry_count = 2;
        } else {
            archive.manifest.series_state_digest = "0".repeat(64);
        }
        store.begin_archive(archive).await.unwrap();
        store
            .archive_batch("task-archive", "generation-1", batches[0].clone())
            .await
            .unwrap();
        assert!(store
            .finalize_archive(
                "task-archive",
                "generation-1",
                make_task("task-archive"),
                vec![],
            )
            .await
            .is_err());
        assert_eq!(
            store.get_archive_watermark("task-archive").await.unwrap(),
            -1
        );
    }
}

#[tokio::test]
async fn archive_rejects_a_generation_without_the_active_task_fence() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "different-generation").await;
    let (archive, _) = build_archive(vec![make_event("task-archive", 0)], "generation-1");

    assert!(store.begin_archive(archive).await.is_err());
}

#[tokio::test]
async fn archive_finalizes_canonical_series_state() {
    let (store, _container) = setup().await;
    start_archive_release(&store, "generation-1").await;
    let mut event = make_event("task-archive", 0);
    event.series_id = Some("output".to_string());
    event.series_mode = Some(SeriesMode::Accumulate);
    event.series_acc_field = Some("delta".to_string());
    event.data = serde_json::json!({ "delta": "hello world" });
    let series = DurableSeriesState {
        task_id: "task-archive".to_string(),
        series_id: "output".to_string(),
        mode: SeriesMode::Accumulate,
        event: event.clone(),
        through_index: 0,
    };
    let (archive, batches) =
        build_archive_with_series(vec![event.clone()], vec![series.clone()], "generation-1");
    store.begin_archive(archive).await.unwrap();
    store
        .archive_batch("task-archive", "generation-1", batches[0].clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .finalize_archive(
                "task-archive",
                "generation-1",
                make_task("task-archive"),
                vec![series.clone()],
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .get_durable_series_state("task-archive")
            .await
            .unwrap(),
        vec![series]
    );
    assert_eq!(
        store.get_events("task-archive", None).await.unwrap(),
        vec![event]
    );
}

// ─── save_task / get_task ─────────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_a_task() {
    let (store, _container) = setup().await;
    let task = make_task("task-1");
    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("task-1").await.unwrap();
    assert_eq!(retrieved, Some(task));
}

#[tokio::test]
async fn return_none_for_missing_task() {
    let (store, _container) = setup().await;
    let result = store.get_task("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn upsert_task_on_conflict() {
    let (store, _container) = setup().await;
    let task = make_task("task-1");
    store.save_task(task.clone()).await.unwrap();

    let mut updated = task.clone();
    updated.status = TaskStatus::Running;
    updated.updated_at = 2000.0;
    store.save_task(updated.clone()).await.unwrap();

    let retrieved = store.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(retrieved.status, TaskStatus::Running);
    assert_eq!(retrieved.updated_at, 2000.0);
}

#[tokio::test]
async fn atomically_claim_task_identity_once() {
    let (store, _container) = setup().await;
    let first = store.create_task_if_absent(make_task("task-1"));
    let second = store.create_task_if_absent(make_task("task-1"));
    let (first, second) = tokio::join!(first, second);
    let mut results = vec![first.unwrap(), second.unwrap()];
    results.sort();
    assert_eq!(results, vec![false, true]);
    assert!(store.get_task("task-1").await.unwrap().is_some());
}

#[tokio::test]
async fn completes_or_aborts_only_the_matching_pristine_creation_claim() {
    let (store, _container) = setup().await;
    assert!(store
        .claim_task_creation(make_task("task-1"), "token-1", 30_000)
        .await
        .unwrap());
    assert!(!store
        .abort_task_creation("task-1", "wrong-token")
        .await
        .unwrap());
    let mut running = make_task("task-1");
    running.status = TaskStatus::Running;
    store.save_task(running).await.unwrap();
    assert!(!store
        .abort_task_creation("task-1", "token-1")
        .await
        .unwrap());
    assert!(store
        .complete_task_creation("task-1", "token-1")
        .await
        .unwrap());
    assert!(store
        .complete_task_creation("task-1", "token-1")
        .await
        .unwrap());
    assert!(!store
        .abort_task_creation("task-1", "token-1")
        .await
        .unwrap());
    assert!(store.get_task("task-1").await.unwrap().is_some());

    assert!(store
        .claim_task_creation(make_task("task-retry"), "token-2", 30_000)
        .await
        .unwrap());
    assert!(store
        .abort_task_creation("task-retry", "token-2")
        .await
        .unwrap());
    assert!(store.get_task("task-retry").await.unwrap().is_none());
    assert!(store
        .claim_task_creation(make_task("task-retry"), "token-3", 30_000)
        .await
        .unwrap());
}

#[tokio::test]
async fn takes_over_only_an_expired_pristine_creation_claim() {
    let (store, _container) = setup().await;
    assert!(store
        .claim_task_creation(make_task("task-1"), "token-1", 500)
        .await
        .unwrap());
    assert!(!store
        .claim_task_creation(make_task("task-1"), "token-2", 500)
        .await
        .unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(510)).await;
    assert!(store
        .claim_task_creation(make_task("task-1"), "token-2", 30_000)
        .await
        .unwrap());
    assert!(!store
        .complete_task_creation("task-1", "token-1")
        .await
        .unwrap());
    assert!(store
        .complete_task_creation("task-1", "token-2")
        .await
        .unwrap());
    assert!(store
        .complete_task_creation("task-1", "token-2")
        .await
        .unwrap());
}

#[tokio::test]
async fn preserve_optional_fields_on_round_trip() {
    let (store, _container) = setup().await;
    let mut task = make_task("task-1");
    task.r#type = Some("llm".to_string());
    task.result = Some(
        [("answer".to_string(), serde_json::json!(42))]
            .into_iter()
            .collect(),
    );
    task.error = Some(taskcast_core::types::TaskError {
        message: "boom".to_string(),
        code: Some("ERR".to_string()),
        details: None,
    });
    task.metadata = Some(
        [("source".to_string(), serde_json::json!("test"))]
            .into_iter()
            .collect(),
    );
    task.completed_at = Some(3000.0);
    task.ttl = Some(60);

    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(retrieved, task);
}

#[tokio::test]
async fn handle_task_with_no_optional_fields() {
    let (store, _container) = setup().await;
    let task = Task {
        id: "minimal".to_string(),
        r#type: None,
        status: TaskStatus::Pending,
        params: None,
        result: None,
        error: None,
        metadata: None,
        auth_config: None,
        webhooks: None,
        cleanup: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
        created_at: 1000.0,
        updated_at: 1000.0,
        completed_at: None,
        ttl: None,
    };
    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("minimal").await.unwrap().unwrap();
    assert_eq!(retrieved, task);
}

// ─── save_event / get_events ──────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_events() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();

    let e0 = make_event("task-1", 0);
    let e1 = make_event("task-1", 1);
    let e2 = make_event("task-1", 2);

    store.save_event(e0.clone()).await.unwrap();
    store.save_event(e1.clone()).await.unwrap();
    store.save_event(e2.clone()).await.unwrap();

    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events, vec![e0, e1, e2]);
}

#[tokio::test]
async fn return_empty_vec_when_no_events() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    let events = store.get_events("task-1", None).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn compact_latest_series_events_in_long_term_storage() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();

    let mut first = make_event("task-1", 0);
    first.id = "status-1".to_string();
    first.r#type = "task.status".to_string();
    first.data = serde_json::json!({ "status": "starting" });
    first.series_id = Some("status".to_string());
    first.series_mode = Some(SeriesMode::Latest);

    let mut second = make_event("task-1", 1);
    second.id = "status-2".to_string();
    second.r#type = "task.status".to_string();
    second.data = serde_json::json!({ "status": "ready" });
    second.series_id = Some("status".to_string());
    second.series_mode = Some(SeriesMode::Latest);

    store
        .replace_last_series_event("task-1", "status", first)
        .await
        .unwrap();
    store
        .replace_last_series_event("task-1", "status", second)
        .await
        .unwrap();

    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "status-1");
    assert_eq!(events[0].index, 0);
    assert_eq!(events[0].data, serde_json::json!({ "status": "ready" }));
}

#[tokio::test]
async fn compact_accumulate_series_events_in_long_term_storage() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();

    let mut first = make_event("task-1", 0);
    first.id = "output-1".to_string();
    first.r#type = "task.output".to_string();
    first.data = serde_json::json!({ "delta": "hello " });
    first.series_id = Some("output".to_string());
    first.series_mode = Some(SeriesMode::Accumulate);
    first.series_acc_field = Some("delta".to_string());

    let mut second = make_event("task-1", 1);
    second.id = "output-2".to_string();
    second.r#type = "task.output".to_string();
    second.data = serde_json::json!({ "delta": "world" });
    second.series_id = Some("output".to_string());
    second.series_mode = Some(SeriesMode::Accumulate);
    second.series_acc_field = Some("delta".to_string());

    let first_result = store
        .accumulate_series("task-1", "output", first, "delta")
        .await
        .unwrap();
    let second_result = store
        .accumulate_series("task-1", "output", second, "delta")
        .await
        .unwrap();

    assert_eq!(first_result.data, serde_json::json!({ "delta": "hello " }));
    assert_eq!(
        second_result.data,
        serde_json::json!({ "delta": "hello world" })
    );

    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "output-1");
    assert_eq!(events[0].index, 0);
    assert_eq!(
        events[0].data,
        serde_json::json!({ "delta": "hello world" })
    );
}

#[tokio::test]
async fn accumulate_series_defaults_an_omitted_field_to_delta() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    let mut first = make_event("task-1", 0);
    first.data = serde_json::json!({ "delta": "hello " });
    first.series_id = Some("output".to_string());
    first.series_mode = Some(SeriesMode::Accumulate);
    let mut second = make_event("task-1", 1);
    second.data = serde_json::json!({ "delta": "world" });
    second.series_id = Some("output".to_string());
    second.series_mode = Some(SeriesMode::Accumulate);

    store
        .accumulate_series("task-1", "output", first, "delta")
        .await
        .unwrap();
    let accumulated = store
        .accumulate_series("task-1", "output", second, "delta")
        .await
        .unwrap();
    assert_eq!(
        accumulated.data,
        serde_json::json!({ "delta": "hello world" })
    );
    assert_eq!(accumulated.series_acc_field, None);
    let state = store.get_durable_series_state("task-1").await.unwrap();
    assert_eq!(state[0].through_index, 1);
    assert_eq!(state[0].event.series_acc_field, None);
}

#[tokio::test]
async fn filter_events_by_since_index() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(2),
            timestamp: None,
            id: None,
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 3);
    assert_eq!(events[1].index, 4);
}

#[tokio::test]
async fn filter_events_by_since_timestamp() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1200.0),
            id: None,
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1300.0);
    assert_eq!(events[1].timestamp, 1400.0);
}

#[tokio::test]
async fn filter_events_by_since_id() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("evt-task-1-2".to_string()),
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "evt-task-1-3");
    assert_eq!(events[1].id, "evt-task-1-4");
}

#[tokio::test]
async fn return_all_events_when_since_id_not_found() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..3 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("nonexistent-id".to_string()),
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn respect_limit_parameter() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: None,
        limit: Some(3),
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].index, 0);
    assert_eq!(events[2].index, 2);
}

#[tokio::test]
async fn apply_limit_after_since_filter() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(5),
            timestamp: None,
            id: None,
        }),
        limit: Some(2),
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 6);
    assert_eq!(events[1].index, 7);
}

#[tokio::test]
async fn save_event_on_conflict_do_nothing() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();

    let event = make_event("task-1", 0);
    store.save_event(event.clone()).await.unwrap();
    // Saving the same event again should not error
    store.save_event(event.clone()).await.unwrap();

    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn preserve_series_fields_on_events() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    let mut event = make_event("task-1", 0);
    event.series_id = Some("my-series".to_string());
    event.series_mode = Some(taskcast_core::types::SeriesMode::Accumulate);

    store.save_event(event.clone()).await.unwrap();
    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events[0], event);
}

// ─── Worker event helpers ─────────────────────────────────────────────────

fn make_worker_event(id: &str, worker_id: &str, index: u64) -> WorkerAuditEvent {
    WorkerAuditEvent {
        id: id.to_string(),
        worker_id: worker_id.to_string(),
        timestamp: 1000.0 + index as f64 * 100.0,
        action: WorkerAuditAction::Connected,
        data: None,
    }
}

// ─── save_worker_event / get_worker_events ────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_worker_events() {
    let (store, _container) = setup().await;

    let e0 = make_worker_event("we-1", "worker-1", 0);
    let e1 = make_worker_event("we-2", "worker-1", 1);

    store.save_worker_event(e0.clone()).await.unwrap();
    store.save_worker_event(e1.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], e0);
    assert_eq!(events[1], e1);
}

#[tokio::test]
async fn return_empty_when_no_worker_events() {
    let (store, _container) = setup().await;

    let events = store
        .get_worker_events("nonexistent-worker", None)
        .await
        .unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn save_multiple_worker_events_verify_ordering() {
    let (store, _container) = setup().await;

    // Insert events out of timestamp order
    let e2 = make_worker_event("we-3", "worker-1", 2);
    let e0 = make_worker_event("we-1", "worker-1", 0);
    let e1 = make_worker_event("we-2", "worker-1", 1);

    store.save_worker_event(e2.clone()).await.unwrap();
    store.save_worker_event(e0.clone()).await.unwrap();
    store.save_worker_event(e1.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 3);
    // Should be ordered by timestamp ASC regardless of insertion order
    assert_eq!(events[0].id, "we-1");
    assert_eq!(events[1].id, "we-2");
    assert_eq!(events[2].id, "we-3");
}

#[tokio::test]
async fn save_worker_event_with_data_field() {
    let (store, _container) = setup().await;

    let mut data = HashMap::new();
    data.insert("reason".to_string(), serde_json::json!("timeout"));
    data.insert("duration_ms".to_string(), serde_json::json!(5000));

    let event = WorkerAuditEvent {
        id: "we-data-1".to_string(),
        worker_id: "worker-1".to_string(),
        timestamp: 1000.0,
        action: WorkerAuditAction::HeartbeatTimeout,
        data: Some(data),
    };

    store.save_worker_event(event.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], event);
    let retrieved_data = events[0].data.as_ref().unwrap();
    assert_eq!(retrieved_data["reason"], serde_json::json!("timeout"));
    assert_eq!(retrieved_data["duration_ms"], serde_json::json!(5000));
}

#[tokio::test]
async fn duplicate_worker_event_id_is_ignored() {
    let (store, _container) = setup().await;

    let event = make_worker_event("we-dup", "worker-1", 0);
    store.save_worker_event(event.clone()).await.unwrap();
    // Saving the same event again should not error (ON CONFLICT DO NOTHING)
    store.save_worker_event(event.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
}

// ─── Worker event filtering ──────────────────────────────────────────────

#[tokio::test]
async fn filter_worker_events_by_since_timestamp() {
    let (store, _container) = setup().await;

    for i in 0..5 {
        store
            .save_worker_event(make_worker_event(&format!("we-{}", i), "worker-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1200.0),
            id: None,
        }),
        limit: None,
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1300.0);
    assert_eq!(events[1].timestamp, 1400.0);
}

#[tokio::test]
async fn filter_worker_events_by_since_id() {
    let (store, _container) = setup().await;

    for i in 0..5 {
        store
            .save_worker_event(make_worker_event(&format!("we-{}", i), "worker-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("we-2".to_string()),
        }),
        limit: None,
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "we-3");
    assert_eq!(events[1].id, "we-4");
}

#[tokio::test]
async fn return_all_worker_events_when_since_id_not_found() {
    let (store, _container) = setup().await;

    for i in 0..3 {
        store
            .save_worker_event(make_worker_event(&format!("we-{}", i), "worker-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("nonexistent-id".to_string()),
        }),
        limit: None,
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn respect_limit_on_worker_events() {
    let (store, _container) = setup().await;

    for i in 0..10 {
        store
            .save_worker_event(make_worker_event(&format!("we-{}", i), "worker-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: None,
        limit: Some(3),
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].id, "we-0");
    assert_eq!(events[2].id, "we-2");
}

#[tokio::test]
async fn combine_since_timestamp_and_limit_on_worker_events() {
    let (store, _container) = setup().await;

    for i in 0..10 {
        store
            .save_worker_event(make_worker_event(&format!("we-{}", i), "worker-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1400.0),
            id: None,
        }),
        limit: Some(2),
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    // Events after timestamp 1400.0 are indices 5,6,7,8,9 (timestamps 1500,1600,...,1900)
    assert_eq!(events[0].timestamp, 1500.0);
    assert_eq!(events[1].timestamp, 1600.0);
}
