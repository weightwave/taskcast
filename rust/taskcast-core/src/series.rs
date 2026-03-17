use crate::types::{SeriesMode, SeriesResult, ShortTermStore, TaskEvent};

/// Process a task event through its series mode logic.
///
/// - If the event has no `series_id` or `series_mode`, it is returned unchanged.
/// - `keep-all`: returned unchanged with no store interaction.
/// - `accumulate`: delegates to `store.accumulate_series()`, returns both delta and accumulated.
/// - `latest`: replaces the last series event in the store and returns the event.
pub async fn process_series(
    event: TaskEvent,
    store: &dyn ShortTermStore,
) -> Result<SeriesResult, Box<dyn std::error::Error + Send + Sync>> {
    let (series_id, series_mode) = match (&event.series_id, &event.series_mode) {
        (Some(sid), Some(mode)) => (sid.clone(), mode.clone()),
        _ => return Ok(SeriesResult { event, accumulated_event: None, stored: false }),
    };

    match series_mode {
        SeriesMode::KeepAll => Ok(SeriesResult { event, accumulated_event: None, stored: false }),

        SeriesMode::Accumulate => {
            let field = event
                .series_acc_field
                .as_deref()
                .unwrap_or("delta");
            let accumulated = store
                .accumulate_series(&event.task_id, &series_id, event.clone(), field)
                .await?;
            Ok(SeriesResult { event, accumulated_event: Some(accumulated), stored: false })
        }

        SeriesMode::Latest => {
            store
                .replace_last_series_event(&event.task_id, &series_id, event.clone())
                .await?;
            Ok(SeriesResult { event, accumulated_event: None, stored: true })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_adapters::MemoryShortTermStore;
    use crate::types::Level;
    use serde_json::json;

    fn make_event(
        id: &str,
        task_id: &str,
        index: u64,
        data: serde_json::Value,
    ) -> TaskEvent {
        TaskEvent {
            id: id.to_string(),
            task_id: task_id.to_string(),
            index,
            timestamp: 1000.0 + (index as f64) * 1000.0,
            r#type: "progress".to_string(),
            level: Level::Info,
            data,
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        }
    }

    fn make_series_event(
        id: &str,
        task_id: &str,
        index: u64,
        data: serde_json::Value,
        series_id: &str,
        series_mode: SeriesMode,
    ) -> TaskEvent {
        TaskEvent {
            series_id: Some(series_id.to_string()),
            series_mode: Some(series_mode),
            ..make_event(id, task_id, index, data)
        }
    }

    // ─── No series_id / series_mode → returned unchanged ─────────────────

    #[tokio::test]
    async fn event_without_series_id_returned_unchanged() {
        let store = MemoryShortTermStore::new();
        let event = make_event("e1", "t1", 0, json!({ "text": "hello" }));
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event, event);
        assert!(result.accumulated_event.is_none());
    }

    #[tokio::test]
    async fn event_with_series_id_but_no_mode_returned_unchanged() {
        let store = MemoryShortTermStore::new();
        let mut event = make_event("e1", "t1", 0, json!({ "text": "hello" }));
        event.series_id = Some("s1".to_string());
        // series_mode is still None
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event, event);
        assert!(result.accumulated_event.is_none());
    }

    #[tokio::test]
    async fn event_with_series_mode_but_no_id_returned_unchanged() {
        let store = MemoryShortTermStore::new();
        let mut event = make_event("e1", "t1", 0, json!({ "text": "hello" }));
        event.series_mode = Some(SeriesMode::Accumulate);
        // series_id is still None
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event, event);
        assert!(result.accumulated_event.is_none());
    }

    // ─── keep-all mode → returned unchanged, no store interaction ────────

    #[tokio::test]
    async fn keep_all_returns_event_unchanged() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "text": "hello" }),
            "s1",
            SeriesMode::KeepAll,
        );
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event, event);
        assert!(result.accumulated_event.is_none());

        // Store should have no series data
        let latest = store.get_series_latest("t1", "s1").await.unwrap();
        assert!(latest.is_none());
    }

    // ─── accumulate mode: first event ────────────────────────────────────

    #[tokio::test]
    async fn accumulate_first_event_sets_series_latest() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "delta": "hello" }),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event.clone(), &store).await.unwrap();

        // Delta event should be the original event
        assert_eq!(result.event, event);
        // Accumulated event should exist (same as delta for first event)
        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.id, "e1");
        assert_eq!(acc.data, json!({ "delta": "hello" }));

        // Store should now have the event as series latest
        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.id, "e1");
        assert_eq!(latest.data, json!({ "delta": "hello" }));
    }

    // ─── accumulate mode: second event concatenates delta ────────────────

    #[tokio::test]
    async fn accumulate_second_event_concatenates_text() {
        let store = MemoryShortTermStore::new();

        // First event
        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "delta": "hello" }),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(event1, &store).await.unwrap();

        // Second event
        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "delta": " world" }),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event2.clone(), &store).await.unwrap();

        // Delta event should be the original (not accumulated)
        assert_eq!(result.event.data["delta"], " world");
        assert_eq!(result.event.id, "e2");

        // Accumulated event should have the concatenated text
        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data["delta"], "hello world");
        assert_eq!(acc.id, "e2");

        // Series latest should be the merged event
        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.data["delta"], "hello world");
    }

    #[tokio::test]
    async fn accumulate_three_events_concatenates_all_text() {
        let store = MemoryShortTermStore::new();

        let e1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "delta": "a" }),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(e1, &store).await.unwrap();

        let e2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "delta": "b" }),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(e2, &store).await.unwrap();

        let e3 = make_series_event(
            "e3",
            "t1",
            2,
            json!({ "delta": "c" }),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(e3, &store).await.unwrap();

        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data["delta"], "abc");
    }

    // ─── accumulate mode: non-matching field data → no concatenation ─────

    #[tokio::test]
    async fn accumulate_non_text_data_no_concatenation() {
        let store = MemoryShortTermStore::new();

        // First event with numeric data
        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "count": 1 }),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(event1, &store).await.unwrap();

        // Second event with numeric data
        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "count": 2 }),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event2, &store).await.unwrap();

        // Accumulated event data should match (no concatenation for non-string)
        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data, json!({ "count": 2 }));
    }

    #[tokio::test]
    async fn accumulate_non_object_data_no_concatenation() {
        let store = MemoryShortTermStore::new();

        // First event with string data (not an object)
        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!("plain string"),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(event1, &store).await.unwrap();

        // Second event with string data
        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!("another string"),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event2, &store).await.unwrap();

        // No concatenation since data is not an object with accumulate field
        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data, json!("another string"));
    }

    #[tokio::test]
    async fn accumulate_prev_has_text_new_does_not() {
        let store = MemoryShortTermStore::new();

        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "delta": "hello" }),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(event1, &store).await.unwrap();

        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "count": 42 }),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event2, &store).await.unwrap();

        // No concatenation since new event has no accumulate field
        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data, json!({ "count": 42 }));
    }

    #[tokio::test]
    async fn accumulate_preserves_extra_fields_in_new_data() {
        let store = MemoryShortTermStore::new();

        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "delta": "hello" }),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(event1, &store).await.unwrap();

        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "delta": " world", "extra": true }),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event2, &store).await.unwrap();

        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data["delta"], "hello world");
        assert_eq!(acc.data["extra"], true);
    }

    // ─── accumulate mode: custom series_acc_field ─────────────────────────

    #[tokio::test]
    async fn accumulate_custom_series_acc_field() {
        let store = MemoryShortTermStore::new();

        let mut event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "content": "hello" }),
            "s1",
            SeriesMode::Accumulate,
        );
        event1.series_acc_field = Some("content".to_string());
        process_series(event1, &store).await.unwrap();

        let mut event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "content": " world" }),
            "s1",
            SeriesMode::Accumulate,
        );
        event2.series_acc_field = Some("content".to_string());
        let result = process_series(event2, &store).await.unwrap();

        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data["content"], "hello world");

        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.data["content"], "hello world");
    }

    #[tokio::test]
    async fn accumulate_legacy_text_field() {
        let store = MemoryShortTermStore::new();

        let mut event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "text": "hello" }),
            "s1",
            SeriesMode::Accumulate,
        );
        event1.series_acc_field = Some("text".to_string());
        process_series(event1, &store).await.unwrap();

        let mut event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "text": " world" }),
            "s1",
            SeriesMode::Accumulate,
        );
        event2.series_acc_field = Some("text".to_string());
        let result = process_series(event2, &store).await.unwrap();

        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data["text"], "hello world");

        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.data["text"], "hello world");
    }

    // ─── latest mode → calls replace_last_series_event ───────────────────

    #[tokio::test]
    async fn latest_calls_replace_last_series_event() {
        let store = MemoryShortTermStore::new();

        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "text": "hello" }),
            "s1",
            SeriesMode::Latest,
        );
        let result = process_series(event.clone(), &store).await.unwrap();

        // Should return event unchanged
        assert_eq!(result.event, event);
        assert!(result.accumulated_event.is_none());

        // Store should have updated series latest via replace_last_series_event
        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.id, "e1");
    }

    #[tokio::test]
    async fn latest_replaces_previous_event_in_store() {
        let store = MemoryShortTermStore::new();

        // First latest event
        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!({ "text": "first" }),
            "s1",
            SeriesMode::Latest,
        );
        process_series(event1, &store).await.unwrap();

        // Verify it was appended (since no prior event existed)
        let events = store.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "e1");

        // Second latest event should replace the first
        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!({ "text": "second" }),
            "s1",
            SeriesMode::Latest,
        );
        process_series(event2.clone(), &store).await.unwrap();

        let events = store.get_events("t1", None).await.unwrap();
        assert_eq!(events.len(), 1); // still 1 event, replaced
        assert_eq!(events[0].id, "e2");
        assert_eq!(events[0].data["text"], "second");

        let latest = store.get_series_latest("t1", "s1").await.unwrap().unwrap();
        assert_eq!(latest.id, "e2");
    }

    // ─── accumulate mode: non-standard data types (TS parity) ────────────

    #[tokio::test]
    async fn accumulate_null_data() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!(null),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event.data, json!(null));
        assert!(result.accumulated_event.is_some());
    }

    #[tokio::test]
    async fn accumulate_string_data_not_object() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!("just a string"),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event.data, json!("just a string"));
    }

    #[tokio::test]
    async fn accumulate_array_data() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!([1, 2, 3]),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event.data, json!([1, 2, 3]));
    }

    #[tokio::test]
    async fn accumulate_number_primitive_data() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!(42),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event.data, json!(42));
    }

    #[tokio::test]
    async fn accumulate_boolean_data() {
        let store = MemoryShortTermStore::new();
        let event = make_series_event(
            "e1",
            "t1",
            0,
            json!(false),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event.clone(), &store).await.unwrap();
        assert_eq!(result.event.data, json!(false));
    }

    #[tokio::test]
    async fn accumulate_two_string_data_events_no_concat() {
        let store = MemoryShortTermStore::new();

        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!("first string"),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(event1, &store).await.unwrap();

        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!("second string"),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event2, &store).await.unwrap();

        // Strings aren't objects, so accumulate field logic can't find the field to concat
        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data, json!("second string"));
    }

    #[tokio::test]
    async fn accumulate_two_array_data_events_no_concat() {
        let store = MemoryShortTermStore::new();

        let event1 = make_series_event(
            "e1",
            "t1",
            0,
            json!([1, 2, 3]),
            "s1",
            SeriesMode::Accumulate,
        );
        process_series(event1, &store).await.unwrap();

        let event2 = make_series_event(
            "e2",
            "t1",
            1,
            json!([4, 5, 6]),
            "s1",
            SeriesMode::Accumulate,
        );
        let result = process_series(event2, &store).await.unwrap();

        // Arrays aren't objects with named fields, so no concatenation happens
        let acc = result.accumulated_event.unwrap();
        assert_eq!(acc.data, json!([4, 5, 6]));
    }
}
