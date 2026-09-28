use std::sync::Arc;
use taskcast_core::*;

fn setup(
    enabled: bool,
    target: CleanupTarget,
) -> (
    TaskEngine,
    Arc<MemoryShortTermStore>,
    Arc<MemoryLongTermStore>,
) {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let config = ResolvedCleanupConfig {
        enabled,
        rules: vec![
            serde_json::from_value(serde_json::json!({"target": target, "trigger": {}})).unwrap(),
        ],
    };
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    })
    .with_cleanup_config(config)
    .unwrap();
    (engine, hot, durable)
}
async fn complete(engine: &TaskEngine, durable: &MemoryLongTermStore, id: &str, events: usize) {
    engine
        .create_task(CreateTaskInput {
            id: Some(id.into()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(id, TaskStatus::Running, None)
        .await
        .unwrap();
    for n in 0..events {
        engine
            .publish_event(
                id,
                PublishEventInput {
                    r#type: "test".into(),
                    level: Level::Info,
                    data: serde_json::json!(n),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }
    engine
        .transition_task(
            id,
            TaskStatus::Completed,
            Some(TransitionPayload {
                result: Some(serde_json::from_value(serde_json::json!({"kept":true})).unwrap()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    // Wait for actual durable persistence, not just a scheduler yield.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while durable.get_events(id, None).await.unwrap().len() != events + 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn disabled_cleanup_does_nothing_and_enabled_requires_capability() {
    let (engine, _, _) = setup(false, CleanupTarget::Events);
    assert_eq!(
        engine
            .sweep_cleanup(100, 1000, 30000)
            .await
            .unwrap()
            .claimed,
        0
    );
    let unsupported = TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        long_term_store: None,
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    assert!(unsupported
        .with_cleanup_config(ResolvedCleanupConfig {
            enabled: true,
            rules: vec![]
        })
        .is_err());
}
#[tokio::test]
async fn cleans_hot_then_cold_in_bounded_batches_without_losing_results() {
    let (engine, hot, durable) = setup(true, CleanupTarget::Events);
    complete(&engine, &durable, "large", 1499).await;
    let first = engine.sweep_cleanup(100, 1000, 30000).await.unwrap();
    assert_eq!(
        (
            first.claimed,
            first.completed,
            first.deferred,
            first.deleted_events
        ),
        (1, 0, 1, 1000)
    );
    assert!(hot.get_task("large").await.unwrap().is_none());
    assert!(durable
        .get_task("large")
        .await
        .unwrap()
        .unwrap()
        .history_expired_at
        .is_some());
    tokio::time::sleep(std::time::Duration::from_millis(5100)).await;
    let last = engine.sweep_cleanup(100, 1000, 30000).await.unwrap();
    assert_eq!((last.completed, last.deleted_events), (1, 501));
    assert_eq!(durable.get_last_event_index("large").await.unwrap(), 1500);
    assert_eq!(
        durable
            .get_task("large")
            .await
            .unwrap()
            .unwrap()
            .result
            .unwrap()["kept"],
        true
    );
}
#[tokio::test]
async fn old_writer_blocks_cleanup_but_not_existing_v2_release() {
    let (engine, hot, durable) = setup(true, CleanupTarget::Events);
    complete(&engine, &durable, "mixed", 1).await;
    engine
        .register_storage_writer(
            StorageWriterRegistration {
                instance_id: "old".into(),
                storage_protocol_version: 2,
                build: "old".into(),
                expires_at: 0.0,
            },
            30000,
        )
        .await
        .unwrap();
    let result = engine.sweep_cleanup(100, 1000, 30000).await.unwrap();
    assert_eq!((result.completed, result.deferred), (0, 1));
    assert!(hot.get_task("mixed").await.unwrap().is_some());
    assert!(durable
        .get_task("mixed")
        .await
        .unwrap()
        .unwrap()
        .history_expired_at
        .is_none());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    assert_eq!(
        engine
            .release_task_storage_at_current_durable_index("mixed", now)
            .await
            .unwrap()
            .storage_state,
        StorageState::Cold
    );
}
#[tokio::test]
async fn task_bound_and_busy_dependencies_do_not_starve_later_tasks() {
    let (engine, _, durable) = setup(true, CleanupTarget::All);
    complete(&engine, &durable, "a-busy", 1).await;
    complete(&engine, &durable, "b-ready", 1).await;
    durable
        .save_durable_assignment(WorkerAssignment {
            task_id: "a-busy".into(),
            worker_id: "w".into(),
            cost: 1,
            assigned_at: 0.0,
            status: WorkerAssignmentStatus::Running,
        })
        .await
        .unwrap();
    let first = engine.sweep_cleanup(1, 1000, 30000).await.unwrap();
    assert_eq!((first.claimed, first.completed, first.deferred), (1, 0, 1));
    let next = engine.sweep_cleanup(1, 1000, 30000).await.unwrap();
    assert_eq!((next.claimed, next.completed), (1, 1));
    assert!(durable.get_task("a-busy").await.unwrap().is_some());
    assert!(durable.get_task("b-ready").await.unwrap().is_none());
}

#[tokio::test]
async fn competing_cleaners_only_complete_a_task_once() {
    let (engine, hot, durable) = setup(true, CleanupTarget::All);
    complete(&engine, &durable, "contended", 1).await;
    let other = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    })
    .with_cleanup_config(ResolvedCleanupConfig {
        enabled: true,
        rules: vec![],
    })
    .unwrap();
    let (a, b) = tokio::join!(
        engine.sweep_cleanup(100, 1000, 30000),
        other.sweep_cleanup(100, 1000, 30000)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.claimed + b.claimed, 1);
    assert_eq!(a.completed + b.completed, 1);
    assert!(durable.get_task("contended").await.unwrap().is_none());
}
