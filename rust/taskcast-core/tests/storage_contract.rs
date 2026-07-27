use taskcast_core::{
    can_transition, ArchiveBatchReceipt, ArchiveGeneration, ArchiveSourceManifest,
    ArchiveSourcePage, CanonicalHistoryEntry, DurableSeriesState, HotWriteToken, RehydrateSnapshot,
    ReleasePreconditions, ReleaseResult, StorageBusyError, StorageFenceConflictError,
    StorageIntegrityError, StorageLease, StorageReleaseUnsupportedError, TaskStatus,
    TaskStorageMetadata, TerminalProjection, TtlClaim,
};

#[test]
fn lifecycle_types_use_camel_case_wire_fields() {
    let metadata = TaskStorageMetadata {
        task_id: "task-1".into(),
        storage_state: taskcast_core::StorageState::Releasing,
        storage_epoch: 3,
        active_release_generation: Some("generation-1".into()),
        archive_watermark: 7,
        last_event_at: Some(1_000.0),
        cold_at: None,
        execution_deadline_at: Some(2_000.0),
        task_version: 4,
    };
    let release = ReleaseResult {
        task_id: "task-1".into(),
        storage_state: taskcast_core::StorageState::Cold,
        archive_watermark: 7,
        released: true,
    };

    assert_eq!(
        serde_json::to_value(metadata).unwrap(),
        serde_json::json!({
            "taskId": "task-1",
            "storageState": "releasing",
            "storageEpoch": 3,
            "activeReleaseGeneration": "generation-1",
            "archiveWatermark": 7,
            "lastEventAt": 1000.0,
            "coldAt": null,
            "executionDeadlineAt": 2000.0,
            "taskVersion": 4
        })
    );
    assert_eq!(
        serde_json::to_value(release).unwrap(),
        serde_json::json!({
            "taskId": "task-1",
            "storageState": "cold",
            "archiveWatermark": 7,
            "released": true
        })
    );
}

#[test]
fn storage_contract_types_are_public_and_serializable() {
    fn assert_contract<T>()
    where
        T: serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
    }

    assert_contract::<HotWriteToken>();
    assert_contract::<StorageLease>();
    assert_contract::<ReleasePreconditions>();
    assert_contract::<ArchiveSourceManifest>();
    assert_contract::<ArchiveGeneration>();
    assert_contract::<ArchiveBatchReceipt>();
    assert_contract::<ArchiveSourcePage>();
    assert_contract::<DurableSeriesState>();
    assert_contract::<RehydrateSnapshot>();
    assert_contract::<CanonicalHistoryEntry>();
    assert_contract::<TtlClaim>();
    assert_contract::<TerminalProjection>();
}

#[test]
fn storage_errors_have_stable_codes_and_retryability() {
    assert_eq!(
        StorageFenceConflictError::default().code(),
        "storage_fence_conflict"
    );
    assert!(StorageFenceConflictError::default().retryable());
    assert_eq!(StorageBusyError::default().code(), "storage_busy");
    assert_eq!(
        StorageIntegrityError::default().code(),
        "storage_integrity_error"
    );
    assert_eq!(
        StorageReleaseUnsupportedError::default().code(),
        "storage_release_unsupported"
    );
}

#[test]
fn every_non_terminal_status_can_timeout() {
    for status in [
        TaskStatus::Pending,
        TaskStatus::Assigned,
        TaskStatus::Running,
        TaskStatus::Paused,
        TaskStatus::Blocked,
    ] {
        assert!(can_transition(&status, &TaskStatus::Timeout));
    }

    for status in [
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Timeout,
        TaskStatus::Cancelled,
    ] {
        assert!(!can_transition(&status, &TaskStatus::Timeout));
    }
}
