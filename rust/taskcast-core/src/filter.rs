use crate::types::{SubscribeFilter, TaskEvent};

/// A task event annotated with its filtered (post-filter) index and raw (original) index.
#[derive(Debug, Clone)]
pub struct FilteredEvent {
    pub filtered_index: u64,
    pub raw_index: u64,
    pub event: TaskEvent,
}

/// Returns `true` if `event_type` matches at least one of the given patterns.
///
/// - `None` patterns means "no filter" and matches everything.
/// - An empty slice matches nothing.
/// - `"*"` matches any type.
/// - `"prefix.*"` matches any type that starts with `"prefix."` (but NOT `"prefix"` exactly).
/// - Otherwise, an exact string match is required.
pub fn matches_type(event_type: &str, patterns: Option<&[String]>) -> bool {
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
            // "llm.*" matches "llm.delta", "llm.delta.chunk" but NOT "llm"
            return event_type.starts_with(&format!("{}.", prefix));
        }
        event_type == pattern
    })
}

/// Returns `true` if the given event passes the subscribe filter.
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

/// Applies the subscribe filter to a list of events, assigning each matching event
/// a monotonically increasing `filtered_index`. If a `since.index` cursor is present,
/// events whose `filtered_index` is <= that cursor value are skipped from the result
/// (but still counted for indexing purposes).
pub fn apply_filtered_index(
    events: &[TaskEvent],
    filter: &SubscribeFilter,
) -> Vec<FilteredEvent> {
    let since_index = filter
        .since
        .as_ref()
        .and_then(|s| s.index);

    let mut filtered_counter: u64 = 0;
    let mut result = Vec::new();

    for event in events {
        if !matches_filter(event, filter) {
            continue;
        }

        let current_filtered_index = filtered_counter;
        filtered_counter += 1;

        // since.index: skip events where filteredIndex <= since.index
        if let Some(idx) = since_index {
            if current_filtered_index <= idx {
                continue;
            }
        }

        result.push(FilteredEvent {
            filtered_index: current_filtered_index,
            raw_index: event.index,
            event: event.clone(),
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Level, SinceCursor, SubscribeFilter};
    use serde_json::json;

    // ─── Helper ──────────────────────────────────────────────────────────

    fn make_event(index: u64, event_type: &str, level: Level) -> TaskEvent {
        TaskEvent {
            id: format!("evt_{}", index),
            task_id: "task_01".to_string(),
            index,
            timestamp: 1_700_000_000_000.0 + index as f64,
            r#type: event_type.to_string(),
            level,
            data: json!(null),
            series_id: None,
            series_mode: None,
        }
    }

    fn empty_filter() -> SubscribeFilter {
        SubscribeFilter {
            since: None,
            types: None,
            levels: None,
            include_status: None,
            wrap: None,
        }
    }

    // ─── matches_type ────────────────────────────────────────────────────

    #[test]
    fn matches_type_none_patterns_returns_true() {
        assert!(matches_type("anything", None));
    }

    #[test]
    fn matches_type_empty_patterns_returns_false() {
        assert!(!matches_type("anything", Some(&[])));
    }

    #[test]
    fn matches_type_star_wildcard_matches_all() {
        let patterns = vec!["*".to_string()];
        assert!(matches_type("llm.delta", Some(&patterns)));
        assert!(matches_type("anything", Some(&patterns)));
    }

    #[test]
    fn matches_type_exact_match() {
        let patterns = vec!["progress".to_string()];
        assert!(matches_type("progress", Some(&patterns)));
        assert!(!matches_type("log", Some(&patterns)));
    }

    #[test]
    fn matches_type_prefix_wildcard() {
        let patterns = vec!["llm.*".to_string()];
        // matches types starting with "llm."
        assert!(matches_type("llm.delta", Some(&patterns)));
        assert!(matches_type("llm.delta.chunk", Some(&patterns)));
        // does NOT match "llm" exactly
        assert!(!matches_type("llm", Some(&patterns)));
        // does NOT match unrelated types
        assert!(!matches_type("other.delta", Some(&patterns)));
    }

    #[test]
    fn matches_type_multiple_patterns() {
        let patterns = vec!["progress".to_string(), "llm.*".to_string()];
        assert!(matches_type("progress", Some(&patterns)));
        assert!(matches_type("llm.delta", Some(&patterns)));
        assert!(!matches_type("log", Some(&patterns)));
    }

    // ─── matches_filter ──────────────────────────────────────────────────

    #[test]
    fn matches_filter_excludes_status_when_include_status_false() {
        let event = make_event(0, "taskcast:status", Level::Info);
        let filter = SubscribeFilter {
            include_status: Some(false),
            ..empty_filter()
        };
        assert!(!matches_filter(&event, &filter));
    }

    #[test]
    fn matches_filter_includes_status_by_default() {
        let event = make_event(0, "taskcast:status", Level::Info);
        let filter = empty_filter();
        assert!(matches_filter(&event, &filter));
    }

    #[test]
    fn matches_filter_includes_status_when_include_status_true() {
        let event = make_event(0, "taskcast:status", Level::Info);
        let filter = SubscribeFilter {
            include_status: Some(true),
            ..empty_filter()
        };
        assert!(matches_filter(&event, &filter));
    }

    #[test]
    fn matches_filter_with_type_filter() {
        let event = make_event(0, "progress", Level::Info);
        let filter = SubscribeFilter {
            types: Some(vec!["progress".to_string()]),
            ..empty_filter()
        };
        assert!(matches_filter(&event, &filter));

        let filter_no_match = SubscribeFilter {
            types: Some(vec!["log".to_string()]),
            ..empty_filter()
        };
        assert!(!matches_filter(&event, &filter_no_match));
    }

    #[test]
    fn matches_filter_with_level_filter() {
        let event = make_event(0, "progress", Level::Warn);
        let filter = SubscribeFilter {
            levels: Some(vec![Level::Warn, Level::Error]),
            ..empty_filter()
        };
        assert!(matches_filter(&event, &filter));

        let filter_no_match = SubscribeFilter {
            levels: Some(vec![Level::Info]),
            ..empty_filter()
        };
        assert!(!matches_filter(&event, &filter_no_match));
    }

    #[test]
    fn matches_filter_with_both_type_and_level() {
        let event = make_event(0, "progress", Level::Info);

        // Both match
        let filter = SubscribeFilter {
            types: Some(vec!["progress".to_string()]),
            levels: Some(vec![Level::Info]),
            ..empty_filter()
        };
        assert!(matches_filter(&event, &filter));

        // Type matches but level does not
        let filter_level_mismatch = SubscribeFilter {
            types: Some(vec!["progress".to_string()]),
            levels: Some(vec![Level::Error]),
            ..empty_filter()
        };
        assert!(!matches_filter(&event, &filter_level_mismatch));

        // Level matches but type does not
        let filter_type_mismatch = SubscribeFilter {
            types: Some(vec!["log".to_string()]),
            levels: Some(vec![Level::Info]),
            ..empty_filter()
        };
        assert!(!matches_filter(&event, &filter_type_mismatch));
    }

    // ─── apply_filtered_index ────────────────────────────────────────────

    #[test]
    fn apply_filtered_index_basic_filtering() {
        let events = vec![
            make_event(0, "progress", Level::Info),
            make_event(1, "log", Level::Debug),
            make_event(2, "progress", Level::Info),
        ];
        let filter = SubscribeFilter {
            types: Some(vec!["progress".to_string()]),
            ..empty_filter()
        };

        let result = apply_filtered_index(&events, &filter);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filtered_index, 0);
        assert_eq!(result[0].raw_index, 0);
        assert_eq!(result[0].event.r#type, "progress");
        assert_eq!(result[1].filtered_index, 1);
        assert_eq!(result[1].raw_index, 2);
        assert_eq!(result[1].event.r#type, "progress");
    }

    #[test]
    fn apply_filtered_index_no_filter_returns_all() {
        let events = vec![
            make_event(0, "a", Level::Info),
            make_event(1, "b", Level::Warn),
            make_event(2, "c", Level::Error),
        ];
        let filter = empty_filter();
        let result = apply_filtered_index(&events, &filter);
        assert_eq!(result.len(), 3);
        for (i, fe) in result.iter().enumerate() {
            assert_eq!(fe.filtered_index, i as u64);
            assert_eq!(fe.raw_index, i as u64);
        }
    }

    #[test]
    fn apply_filtered_index_with_since_index_cursor() {
        let events = vec![
            make_event(0, "a", Level::Info),
            make_event(1, "b", Level::Info),
            make_event(2, "c", Level::Info),
            make_event(3, "d", Level::Info),
        ];
        let filter = SubscribeFilter {
            since: Some(SinceCursor {
                id: None,
                index: Some(1), // skip filtered_index 0 and 1
                timestamp: None,
            }),
            ..empty_filter()
        };

        let result = apply_filtered_index(&events, &filter);
        assert_eq!(result.len(), 2);
        // First returned event should have filtered_index 2 (indices 0 and 1 were skipped)
        assert_eq!(result[0].filtered_index, 2);
        assert_eq!(result[0].raw_index, 2);
        assert_eq!(result[0].event.r#type, "c");
        assert_eq!(result[1].filtered_index, 3);
        assert_eq!(result[1].raw_index, 3);
        assert_eq!(result[1].event.r#type, "d");
    }

    #[test]
    fn apply_filtered_index_with_since_and_type_filter() {
        // Events: progress(0), log(1), progress(2), log(3), progress(4)
        // Filter: types=["progress"], since.index=0
        // Filtered stream: progress(0) [fi=0], progress(2) [fi=1], progress(4) [fi=2]
        // since.index=0 means skip fi<=0, so result = [fi=1, fi=2]
        let events = vec![
            make_event(0, "progress", Level::Info),
            make_event(1, "log", Level::Debug),
            make_event(2, "progress", Level::Info),
            make_event(3, "log", Level::Debug),
            make_event(4, "progress", Level::Info),
        ];
        let filter = SubscribeFilter {
            since: Some(SinceCursor {
                id: None,
                index: Some(0),
                timestamp: None,
            }),
            types: Some(vec!["progress".to_string()]),
            ..empty_filter()
        };

        let result = apply_filtered_index(&events, &filter);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].filtered_index, 1);
        assert_eq!(result[0].raw_index, 2);
        assert_eq!(result[1].filtered_index, 2);
        assert_eq!(result[1].raw_index, 4);
    }

    #[test]
    fn apply_filtered_index_empty_events() {
        let events: Vec<TaskEvent> = vec![];
        let filter = empty_filter();
        let result = apply_filtered_index(&events, &filter);
        assert!(result.is_empty());
    }

    #[test]
    fn apply_filtered_index_since_beyond_all_events() {
        let events = vec![
            make_event(0, "a", Level::Info),
            make_event(1, "b", Level::Info),
        ];
        let filter = SubscribeFilter {
            since: Some(SinceCursor {
                id: None,
                index: Some(99),
                timestamp: None,
            }),
            ..empty_filter()
        };
        let result = apply_filtered_index(&events, &filter);
        assert!(result.is_empty());
    }
}
