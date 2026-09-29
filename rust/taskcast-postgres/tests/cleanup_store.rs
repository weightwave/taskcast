use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use taskcast_core::*;
use taskcast_postgres::PostgresLongTermStore;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

fn event(task_id: &str, index: u64) -> TaskEvent {
    serde_json::from_value(json!({"id": format!("{task_id}-{index}"), "taskId": task_id, "index": index, "timestamp": 1000, "type": "test", "level": "info", "data": {"delta": "x"}})).unwrap()
}

async fn enroll(
    store: &PostgresLongTermStore,
    id: &str,
    target: &str,
    count: u64,
) -> (Task, DurableWriteContext) {
    let ctx = DurableWriteContext {
        creation_token: format!("{id}-generation"),
    };
    let mut task: Task = serde_json::from_value(json!({"id": id, "status": "running", "createdAt": 0, "updatedAt": 0, "cleanupPolicyVersion": 1, "cleanupResolvedAt": 0, "cleanup": {"rules": [{"target": target, "trigger": {"afterMs": 0}}]}})).unwrap();
    assert!(store
        .claim_task_creation(task.clone(), &ctx.creation_token, 30000)
        .await
        .unwrap());
    store
        .complete_task_creation(id, &ctx.creation_token)
        .await
        .unwrap();
    for index in 0..count {
        store
            .save_event_with_context(event(id, index), Some(&ctx))
            .await
            .unwrap();
    }
    task.status = TaskStatus::Completed;
    task.completed_at = Some(1000.0);
    task.result = Some(serde_json::from_value(json!({"value": "keep"})).unwrap());
    store
        .save_task_with_context(task.clone(), Some(&ctx))
        .await
        .unwrap();
    // Storage fixture models a finalized cold archive; coordinator tests exercise the actual release.
    sqlx::query("UPDATE taskcast_tasks SET storage_state = 'cold', cold_at = 1000, archive_watermark = $1 WHERE id = $2")
        .bind(count as i64 - 1).bind(id).execute(store.pool()).await.unwrap();
    (task, ctx)
}

#[tokio::test]
async fn bounded_cleanup_preserves_results_fences_late_writes_and_resumes() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&format!(
            "postgres://postgres:postgres@127.0.0.1:{port}/postgres"
        ))
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();
    let legacy: Task = serde_json::from_value(json!({"id": "legacy", "status": "completed", "createdAt": 0, "updatedAt": 0, "completedAt": 1000, "cleanup": {"rules": [{"target": "all", "trigger": {}}]}})).unwrap();
    store.save_task(legacy).await.unwrap();
    let (task, ctx) = enroll(&store, "large", "events", 1501).await;
    assert!(store.save_task(task.clone()).await.is_err());
    let claims = store.claim_cleanup_tasks(10, 30000).await.unwrap();
    assert_eq!(claims.len(), 1);
    let claim = &claims[0];
    assert_eq!(claim.task_id, "large");
    let mut invalid_target = claim.clone();
    invalid_target.target = CleanupTarget::Task;
    assert!(store.begin_task_cleanup(&invalid_target, 1, 1500).await.is_err());
    assert!(store.begin_task_cleanup(claim, 1, -2).await.is_err());
    sqlx::query("UPDATE taskcast_tasks SET archive_watermark = 1499 WHERE id = 'large'")
        .execute(store.pool()).await.unwrap();
    assert!(!store.begin_task_cleanup(claim, 1, 1499).await.unwrap());
    sqlx::query("UPDATE taskcast_tasks SET archive_watermark = 1500 WHERE id = 'large'")
        .execute(store.pool()).await.unwrap();
    assert!(!store.begin_task_cleanup(claim, 2, 1500).await.unwrap());
    assert!(store.begin_task_cleanup(claim, 1, 1500).await.unwrap());
    let first = store.delete_task_cleanup_batch(claim, 1000).await.unwrap();
    assert_eq!(first.deleted_events, 1000);
    assert!(!first.complete);
    store.defer_cleanup_claim(claim, 0).await.unwrap();
    let retry = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    let last = store.delete_task_cleanup_batch(&retry, 1000).await.unwrap();
    assert_eq!(last.deleted_events, 501);
    assert!(last.complete);
    let retained = store.get_task("large").await.unwrap().unwrap();
    assert!(retained.history_expired_at.is_some());
    assert_eq!(retained.result.unwrap()["value"], "keep");
    assert_eq!(store.get_last_event_index("large").await.unwrap(), 1500);
    assert!(store
        .save_event_with_context(event("large", 0), Some(&ctx))
        .await
        .is_err());
    assert!(store
        .save_task_with_context(task, Some(&ctx))
        .await
        .is_err());
    let mut late = event("large", 0);
    late.series_id = Some("s".into());
    late.series_mode = Some(SeriesMode::Latest);
    assert!(store
        .replace_last_series_event_with_context("large", "s", late.clone(), Some(&ctx))
        .await
        .is_err());
    late.series_mode = Some(SeriesMode::Accumulate);
    assert!(store
        .accumulate_series_with_context("large", "s", late, "delta", Some(&ctx))
        .await
        .is_err());
    assert!(store
        .claim_cleanup_tasks(1, 30000)
        .await
        .unwrap()
        .is_empty());
    assert!(store.get_task("legacy").await.unwrap().is_some());

    let (old, old_ctx) = enroll(&store, "reuse", "all", 1).await;
    let claim = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert!(store.renew_cleanup_claim(&claim, 30000).await.unwrap());
    let mut stolen = claim.clone();
    stolen.claim_token = "wrong".into();
    assert!(!store.renew_cleanup_claim(&stolen, 30000).await.unwrap());
    assert!(!store.begin_task_cleanup(&stolen, 1, 0).await.unwrap());
    assert!(store.begin_task_cleanup(&claim, 1, 0).await.unwrap());
    assert!(
        store
            .delete_task_cleanup_batch(&claim, 1000)
            .await
            .unwrap()
            .complete
    );
    assert!(store.get_task("reuse").await.unwrap().is_none());
    assert!(store
        .save_task_with_context(old.clone(), Some(&old_ctx))
        .await
        .is_err());
    assert!(store.save_task(old.clone()).await.is_err());
    let mut new = old;
    new.status = TaskStatus::Running;
    new.completed_at = None;
    assert!(store
        .claim_task_creation(new, "new-generation", 30000)
        .await
        .unwrap());
    store
        .complete_task_creation("reuse", "new-generation")
        .await
        .unwrap();
    assert!(store
        .save_event_with_context(event("reuse", 0), Some(&old_ctx))
        .await
        .is_err());
    store
        .save_event_with_context(
            event("reuse", 1),
            Some(&DurableWriteContext {
                creation_token: "new-generation".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(store.get_events("reuse", None).await.unwrap().len(), 1);
    assert!(store.claim_cleanup_tasks(0, 1).await.is_err());
    store.pool().close().await;
}

#[tokio::test]
async fn cleanup_claims_defer_dependencies_and_rollback_failed_batches() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&format!(
            "postgres://postgres:postgres@127.0.0.1:{port}/postgres"
        ))
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();
    enroll(&store, "a-busy", "events", 1).await;
    enroll(&store, "b-ready", "events", 1).await;
    let (left, right) = tokio::join!(
        store.claim_cleanup_tasks(1, 30000),
        store.claim_cleanup_tasks(1, 30000)
    );
    let mut claims = [left.unwrap(), right.unwrap()].concat();
    claims.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    assert_eq!(claims.len(), 2);
    assert_ne!(claims[0].task_id, claims[1].task_id);
    let busy = &claims[0];
    store
        .save_durable_assignment(WorkerAssignment {
            task_id: "a-busy".into(),
            worker_id: "worker".into(),
            cost: 1,
            assigned_at: 100.0,
            status: WorkerAssignmentStatus::Running,
        })
        .await
        .unwrap();
    assert!(!store.begin_task_cleanup(busy, 1, 0).await.unwrap());
    assert!(!store.can_cleanup_task(busy).await.unwrap());
    store.defer_cleanup_claim(busy, 60000).await.unwrap();
    store.defer_cleanup_claim(&claims[1], 0).await.unwrap();
    let ready = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert_eq!(ready.task_id, "b-ready");
    sqlx::query("INSERT INTO taskcast_terminal_outbox (projection_id, task_id, event_id, payload, created_at) VALUES ('p', 'b-ready', 'e', '{}'::jsonb, 0)").execute(store.pool()).await.unwrap();
    assert!(!store.begin_task_cleanup(&ready, 1, 0).await.unwrap());
    sqlx::query("UPDATE taskcast_terminal_outbox SET projected_at = 1 WHERE projection_id = 'p'")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO taskcast_archive_generations (task_id,generation,storage_epoch,target_watermark,manifest,status,created_at,updated_at) VALUES ('b-ready','archive',1,0,'{}'::jsonb,'uploading',0,0)").execute(store.pool()).await.unwrap();
    assert!(!store.begin_task_cleanup(&ready, 1, 0).await.unwrap());
    sqlx::query(
        "UPDATE taskcast_archive_generations SET status = 'finalized' WHERE task_id = 'b-ready'",
    )
    .execute(store.pool())
    .await
    .unwrap();
    for series_id in ["s1", "s2"] {
        sqlx::query("INSERT INTO taskcast_series_state (task_id, series_id, mode, event, through_index, updated_at) VALUES ('b-ready', $1, 'latest', '{}'::jsonb, 0, 0)")
            .bind(series_id).execute(store.pool()).await.unwrap();
    }
    assert!(store.begin_task_cleanup(&ready, 1, 0).await.unwrap());
    sqlx::raw_sql("CREATE FUNCTION cleanup_test_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected delete failure'; END; $$; CREATE TRIGGER cleanup_test_failure AFTER DELETE ON taskcast_events FOR EACH STATEMENT EXECUTE FUNCTION cleanup_test_failure();").execute(store.pool()).await.unwrap();
    assert!(store.delete_task_cleanup_batch(&ready, 1000).await.is_err());
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM taskcast_events WHERE task_id = 'b-ready'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(remaining, 1);
    sqlx::raw_sql("DROP TRIGGER cleanup_test_failure ON taskcast_events; DROP FUNCTION cleanup_test_failure();").execute(store.pool()).await.unwrap();
    sqlx::query("UPDATE taskcast_tasks SET cleanup_claim_until = 0 WHERE id = 'b-ready'")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(store.delete_task_cleanup_batch(&ready, 1000).await.is_err());
    let retry = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    let partial = store.delete_task_cleanup_batch(&retry, 1).await.unwrap();
    assert_eq!(partial.deleted_events, 1);
    assert!(!partial.complete);
    store.defer_cleanup_claim(&retry, 0).await.unwrap();
    let retry = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert!(
        store
            .delete_task_cleanup_batch(&retry, 1000)
            .await
            .unwrap()
            .complete
    );
    for table in ["taskcast_terminal_outbox", "taskcast_archive_generations", "taskcast_series_state"] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE task_id = 'b-ready'"
        ))
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
    store.pool().close().await;
}

#[tokio::test]
async fn coordinates_real_archive_release_then_cleanup() {
    use std::sync::Arc;
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new().max_connections(5).connect(&format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")).await.unwrap();
    let durable = Arc::new(PostgresLongTermStore::new(pool));
    durable.migrate().await.unwrap();
    let hot = Arc::new(MemoryShortTermStore::new());
    let engine = TaskEngine::new(TaskEngineOptions { short_term_store: hot.clone(), long_term_store: Some(durable.clone()), broadcast: Arc::new(MemoryBroadcastProvider::new()), hooks: None }).with_cleanup_config(ResolvedCleanupConfig { enabled: true, rules: vec![serde_json::from_value(json!({"target":"events","trigger":{}})).unwrap()] }).unwrap();
    let task = engine.create_task(CreateTaskInput::default()).await.unwrap();
    engine.transition_task(&task.id,TaskStatus::Running,None).await.unwrap();
    engine.publish_event(&task.id,PublishEventInput { r#type: "test".into(), level: Level::Info, data: json!("value"), series_id: None, series_mode: None, series_acc_field: None }).await.unwrap();
    engine.transition_task(&task.id,TaskStatus::Completed,Some(TransitionPayload { result: Some(serde_json::from_value(json!({"kept":true})).unwrap()),..Default::default() })).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),async { while durable.get_events(&task.id,None).await.unwrap().len() != 3 { tokio::task::yield_now().await; } }).await.unwrap();
    let result = engine.sweep_cleanup(100,1000,30000).await.unwrap();
    assert_eq!((result.completed,result.deleted_events,result.failed),(1,3,0));
    assert!(hot.get_task(&task.id).await.unwrap().is_none());
    let retained = durable.get_task(&task.id).await.unwrap().unwrap();
    assert!(retained.history_expired_at.is_some());
    assert_eq!(retained.result.unwrap()["kept"],true);
    assert_eq!(durable.get_last_event_index(&task.id).await.unwrap(),2);
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM taskcast_archive_batches WHERE task_id = $1").bind(&task.id).fetch_one(durable.pool()).await.unwrap();
    assert_eq!(receipts,0);
    durable.pool().close().await;
}


#[tokio::test]
async fn explicit_restore_rotates_generation_and_never_enrolls_archived_markers() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new().max_connections(5).connect(&format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")).await.unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();
    let (mut task, ctx) = enroll(&store, "restore", "events", 2).await;
    let claim = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert!(store.begin_task_cleanup(&claim, 1, 1).await.unwrap());
    task.history_expired_at = Some(1.0);
    let archive = TaskArchive { schema: "taskcast.taskArchive".into(), version: 1, exported_at: 1.0, task,
        events: vec![event("restore", 0), event("restore", 1)] };
    let data = build_task_archive_restore_data(&archive).unwrap();
    assert!(store.restore_task_archive(data.clone(), None).await.is_err());
    store.restore_task_archive(data, Some(TaskArchiveImportOptions { overwrite: true })).await.unwrap();
    let restored = store.get_task("restore").await.unwrap().unwrap();
    assert!(restored.cleanup_policy_version.is_none());
    assert!(restored.history_expired_at.is_none());
    assert_ne!(store.get_task_storage_metadata("restore").await.unwrap().unwrap().creation_token, Some(ctx.creation_token.clone()));
    assert!(store.save_event_with_context(event("restore", 2), Some(&ctx)).await.is_err());
    assert!(store.save_event(event("restore", 3)).await.is_err());
    assert!(store.save_task(restored).await.is_err());
    assert!(store.claim_cleanup_tasks(1, 30000).await.unwrap().is_empty());
    assert_eq!(store.get_events("restore", None).await.unwrap().len(), 2);
}


#[tokio::test]
async fn expired_retry_does_not_starve_untouched_due_tasks() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new().max_connections(5).connect(&format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")).await.unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();
    enroll(&store, "a-retry", "events", 1).await;
    enroll(&store, "b-ready", "events", 1).await;
    let first = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert_eq!(first.task_id, "a-retry");
    store.defer_cleanup_claim(&first, 0).await.unwrap();
    assert_eq!(store.claim_cleanup_tasks(1, 30000).await.unwrap()[0].task_id, "b-ready");
}
