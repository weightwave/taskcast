use crate::filter::matches_type;
use crate::state_machine::is_terminal;
use crate::types::{CleanupRule, Task, TaskEvent};

/// Returns `true` if the given task matches the cleanup rule at time `now` (ms).
///
/// A task matches when:
/// 1. The task is in a terminal status.
/// 2. If the rule specifies `match.status`, the task's status must be in that list.
/// 3. If the rule specifies `match.task_types`, the task must have a type that matches.
/// 4. If the rule specifies `trigger.after_ms`, enough time must have elapsed since completion.
pub fn matches_cleanup_rule(task: &Task, rule: &CleanupRule, now: f64) -> bool {
    if !is_terminal(&task.status) {
        return false;
    }

    if let Some(ref rule_match) = rule.r#match {
        if let Some(ref statuses) = rule_match.status {
            if !statuses.contains(&task.status) {
                return false;
            }
        }

        if let Some(ref task_types) = rule_match.task_types {
            match &task.r#type {
                Some(t) => {
                    if !matches_type(t, Some(task_types)) {
                        return false;
                    }
                }
                None => return false,
            }
        }
    }

    if let Some(after_ms) = rule.trigger.after_ms {
        let completed_at = task.completed_at.unwrap_or(task.updated_at);
        let elapsed = now - completed_at;
        if elapsed < after_ms as f64 {
            return false;
        }
    }

    true
}

/// Filters events that should be cleaned up according to the rule's `event_filter`.
///
/// If the rule has no `event_filter`, all events are returned (meaning all match for cleanup).
/// Otherwise, only events matching **all** specified filter criteria are returned.
pub fn filter_events_for_cleanup(
    events: &[TaskEvent],
    rule: &CleanupRule,
    _now: f64,
    completed_at: Option<f64>,
) -> Vec<TaskEvent> {
    let ef = match &rule.event_filter {
        Some(ef) => ef,
        None => return events.to_vec(),
    };

    events
        .iter()
        .filter(|event| {
            if let Some(ref types) = ef.types {
                if !matches_type(&event.r#type, Some(types)) {
                    return false;
                }
            }

            if let Some(ref levels) = ef.levels {
                if !levels.contains(&event.level) {
                    return false;
                }
            }

            if let Some(ref series_modes) = ef.series_mode {
                match &event.series_mode {
                    Some(sm) => {
                        if !series_modes.contains(sm) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }

            if let Some(older_than_ms) = ef.older_than_ms {
                if let Some(completed) = completed_at {
                    let cutoff = completed - older_than_ms as f64;
                    if event.timestamp >= cutoff {
                        return false;
                    }
                }
            }

            true
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CleanupEventFilter, CleanupRuleMatch, CleanupTarget, CleanupTrigger, Level, SeriesMode,
        TaskStatus,
    };
    use serde_json::json;

    // ─── Helpers ────────────────────────────────────────────────────────────

    fn make_task(status: TaskStatus) -> Task {
        Task {
            id: "task_01".to_string(),
            r#type: Some("crawl".to_string()),
            status,
            params: None,
            result: None,
            error: None,
            metadata: None,
            created_at: 1_000_000.0,
            updated_at: 2_000_000.0,
            completed_at: Some(2_000_000.0),
            ttl: None,
            auth_config: None,
            webhooks: None,
            cleanup: None,
        }
    }

    fn make_rule() -> CleanupRule {
        CleanupRule {
            name: None,
            r#match: None,
            trigger: CleanupTrigger { after_ms: None },
            target: CleanupTarget::All,
            event_filter: None,
        }
    }

    fn make_event(index: u64, event_type: &str, level: Level, timestamp: f64) -> TaskEvent {
        TaskEvent {
            id: format!("evt_{}", index),
            task_id: "task_01".to_string(),
            index,
            timestamp,
            r#type: event_type.to_string(),
            level,
            data: json!(null),
            series_id: None,
            series_mode: None,
        }
    }

    // ─── matches_cleanup_rule ───────────────────────────────────────────────

    #[test]
    fn non_terminal_task_does_not_match() {
        let task = make_task(TaskStatus::Pending);
        let rule = make_rule();
        assert!(!matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn running_task_does_not_match() {
        let task = make_task(TaskStatus::Running);
        let rule = make_rule();
        assert!(!matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn terminal_task_matches_with_no_constraints() {
        let task = make_task(TaskStatus::Completed);
        let rule = make_rule();
        assert!(matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn terminal_task_failed_matches_with_no_constraints() {
        let task = make_task(TaskStatus::Failed);
        let rule = make_rule();
        assert!(matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn terminal_task_timeout_matches_with_no_constraints() {
        let task = make_task(TaskStatus::Timeout);
        let rule = make_rule();
        assert!(matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn terminal_task_cancelled_matches_with_no_constraints() {
        let task = make_task(TaskStatus::Cancelled);
        let rule = make_rule();
        assert!(matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    // ─── Status matching ────────────────────────────────────────────────────

    #[test]
    fn matching_status_returns_true() {
        let task = make_task(TaskStatus::Completed);
        let rule = CleanupRule {
            r#match: Some(CleanupRuleMatch {
                status: Some(vec![TaskStatus::Completed, TaskStatus::Failed]),
                task_types: None,
            }),
            ..make_rule()
        };
        assert!(matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn non_matching_status_returns_false() {
        let task = make_task(TaskStatus::Cancelled);
        let rule = CleanupRule {
            r#match: Some(CleanupRuleMatch {
                status: Some(vec![TaskStatus::Completed, TaskStatus::Failed]),
                task_types: None,
            }),
            ..make_rule()
        };
        assert!(!matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    // ─── Task type matching ─────────────────────────────────────────────────

    #[test]
    fn matching_task_type_exact() {
        let task = make_task(TaskStatus::Completed);
        let rule = CleanupRule {
            r#match: Some(CleanupRuleMatch {
                status: None,
                task_types: Some(vec!["crawl".to_string()]),
            }),
            ..make_rule()
        };
        assert!(matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn matching_task_type_wildcard() {
        let mut task = make_task(TaskStatus::Completed);
        task.r#type = Some("crawl.deep".to_string());
        let rule = CleanupRule {
            r#match: Some(CleanupRuleMatch {
                status: None,
                task_types: Some(vec!["crawl.*".to_string()]),
            }),
            ..make_rule()
        };
        assert!(matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn non_matching_task_type_returns_false() {
        let task = make_task(TaskStatus::Completed);
        let rule = CleanupRule {
            r#match: Some(CleanupRuleMatch {
                status: None,
                task_types: Some(vec!["render".to_string()]),
            }),
            ..make_rule()
        };
        assert!(!matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    #[test]
    fn task_with_no_type_does_not_match_task_types_rule() {
        let mut task = make_task(TaskStatus::Completed);
        task.r#type = None;
        let rule = CleanupRule {
            r#match: Some(CleanupRuleMatch {
                status: None,
                task_types: Some(vec!["crawl".to_string()]),
            }),
            ..make_rule()
        };
        assert!(!matches_cleanup_rule(&task, &rule, 99_999_999.0));
    }

    // ─── Trigger afterMs ────────────────────────────────────────────────────

    #[test]
    fn trigger_after_ms_elapsed_matches() {
        let task = make_task(TaskStatus::Completed); // completed_at = 2_000_000
        let rule = CleanupRule {
            trigger: CleanupTrigger {
                after_ms: Some(1_000_000),
            },
            ..make_rule()
        };
        // now = 3_000_001 => elapsed = 1_000_001 >= 1_000_000
        assert!(matches_cleanup_rule(&task, &rule, 3_000_001.0));
    }

    #[test]
    fn trigger_after_ms_exactly_elapsed_matches() {
        let task = make_task(TaskStatus::Completed); // completed_at = 2_000_000
        let rule = CleanupRule {
            trigger: CleanupTrigger {
                after_ms: Some(1_000_000),
            },
            ..make_rule()
        };
        // now = 3_000_000 => elapsed = 1_000_000 >= 1_000_000
        assert!(matches_cleanup_rule(&task, &rule, 3_000_000.0));
    }

    #[test]
    fn trigger_after_ms_not_elapsed_does_not_match() {
        let task = make_task(TaskStatus::Completed); // completed_at = 2_000_000
        let rule = CleanupRule {
            trigger: CleanupTrigger {
                after_ms: Some(1_000_000),
            },
            ..make_rule()
        };
        // now = 2_500_000 => elapsed = 500_000 < 1_000_000
        assert!(!matches_cleanup_rule(&task, &rule, 2_500_000.0));
    }

    #[test]
    fn trigger_after_ms_uses_updated_at_when_no_completed_at() {
        let mut task = make_task(TaskStatus::Failed);
        task.completed_at = None;
        task.updated_at = 5_000_000.0;
        let rule = CleanupRule {
            trigger: CleanupTrigger {
                after_ms: Some(1_000),
            },
            ..make_rule()
        };
        // now = 5_001_001 => elapsed = 1_001 >= 1_000
        assert!(matches_cleanup_rule(&task, &rule, 5_001_001.0));
        // now = 5_000_500 => elapsed = 500 < 1_000
        assert!(!matches_cleanup_rule(&task, &rule, 5_000_500.0));
    }

    // ─── Combined constraints ───────────────────────────────────────────────

    #[test]
    fn combined_status_and_type_and_trigger() {
        let task = make_task(TaskStatus::Completed); // type="crawl", completed_at=2_000_000
        let rule = CleanupRule {
            r#match: Some(CleanupRuleMatch {
                status: Some(vec![TaskStatus::Completed]),
                task_types: Some(vec!["crawl".to_string()]),
            }),
            trigger: CleanupTrigger {
                after_ms: Some(500_000),
            },
            ..make_rule()
        };
        // now = 2_600_000 => elapsed = 600_000 >= 500_000
        assert!(matches_cleanup_rule(&task, &rule, 2_600_000.0));
    }

    // ─── filter_events_for_cleanup ──────────────────────────────────────────

    #[test]
    fn no_event_filter_returns_all_events() {
        let events = vec![
            make_event(0, "log", Level::Info, 100.0),
            make_event(1, "progress", Level::Debug, 200.0),
        ];
        let rule = make_rule(); // event_filter = None
        let result = filter_events_for_cleanup(&events, &rule, 999.0, Some(500.0));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn type_filter_keeps_matching_events() {
        let events = vec![
            make_event(0, "log", Level::Info, 100.0),
            make_event(1, "progress", Level::Info, 200.0),
            make_event(2, "log.detail", Level::Info, 300.0),
        ];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: Some(vec!["log".to_string()]),
                levels: None,
                older_than_ms: None,
                series_mode: None,
            }),
            ..make_rule()
        };
        let result = filter_events_for_cleanup(&events, &rule, 999.0, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].r#type, "log");
    }

    #[test]
    fn type_filter_wildcard() {
        let events = vec![
            make_event(0, "log", Level::Info, 100.0),
            make_event(1, "log.detail", Level::Info, 200.0),
            make_event(2, "progress", Level::Info, 300.0),
        ];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: Some(vec!["log.*".to_string()]),
                levels: None,
                older_than_ms: None,
                series_mode: None,
            }),
            ..make_rule()
        };
        let result = filter_events_for_cleanup(&events, &rule, 999.0, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].r#type, "log.detail");
    }

    #[test]
    fn level_filter_keeps_matching_events() {
        let events = vec![
            make_event(0, "log", Level::Debug, 100.0),
            make_event(1, "log", Level::Info, 200.0),
            make_event(2, "log", Level::Error, 300.0),
        ];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: None,
                levels: Some(vec![Level::Debug, Level::Info]),
                older_than_ms: None,
                series_mode: None,
            }),
            ..make_rule()
        };
        let result = filter_events_for_cleanup(&events, &rule, 999.0, None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].level, Level::Debug);
        assert_eq!(result[1].level, Level::Info);
    }

    #[test]
    fn series_mode_filter_keeps_matching_events() {
        let mut evt0 = make_event(0, "log", Level::Info, 100.0);
        evt0.series_mode = Some(SeriesMode::KeepAll);
        let mut evt1 = make_event(1, "log", Level::Info, 200.0);
        evt1.series_mode = Some(SeriesMode::Latest);
        let evt2 = make_event(2, "log", Level::Info, 300.0); // no series_mode

        let events = vec![evt0, evt1, evt2];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: None,
                levels: None,
                older_than_ms: None,
                series_mode: Some(vec![SeriesMode::KeepAll]),
            }),
            ..make_rule()
        };
        let result = filter_events_for_cleanup(&events, &rule, 999.0, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].series_mode, Some(SeriesMode::KeepAll));
    }

    #[test]
    fn series_mode_filter_excludes_events_without_series_mode() {
        let evt = make_event(0, "log", Level::Info, 100.0); // series_mode = None
        let events = vec![evt];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: None,
                levels: None,
                older_than_ms: None,
                series_mode: Some(vec![SeriesMode::Accumulate]),
            }),
            ..make_rule()
        };
        let result = filter_events_for_cleanup(&events, &rule, 999.0, None);
        assert!(result.is_empty());
    }

    #[test]
    fn older_than_ms_filter_keeps_old_events() {
        // completed_at = 1000, older_than_ms = 500, cutoff = 1000 - 500 = 500
        // event at timestamp 400 < 500 => kept
        // event at timestamp 500 >= 500 => excluded
        // event at timestamp 800 >= 500 => excluded
        let events = vec![
            make_event(0, "log", Level::Info, 400.0),
            make_event(1, "log", Level::Info, 500.0),
            make_event(2, "log", Level::Info, 800.0),
        ];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: None,
                levels: None,
                older_than_ms: Some(500),
                series_mode: None,
            }),
            ..make_rule()
        };
        let result = filter_events_for_cleanup(&events, &rule, 2000.0, Some(1000.0));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].timestamp, 400.0);
    }

    #[test]
    fn older_than_ms_filter_without_completed_at_keeps_all() {
        let events = vec![
            make_event(0, "log", Level::Info, 100.0),
            make_event(1, "log", Level::Info, 200.0),
        ];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: None,
                levels: None,
                older_than_ms: Some(50),
                series_mode: None,
            }),
            ..make_rule()
        };
        // No completed_at means the olderThanMs check is skipped
        let result = filter_events_for_cleanup(&events, &rule, 999.0, None);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn combined_event_filters() {
        let events = vec![
            make_event(0, "log", Level::Debug, 100.0),
            make_event(1, "log", Level::Info, 200.0),
            make_event(2, "progress", Level::Debug, 300.0),
            make_event(3, "log", Level::Debug, 600.0),
        ];
        let rule = CleanupRule {
            event_filter: Some(CleanupEventFilter {
                types: Some(vec!["log".to_string()]),
                levels: Some(vec![Level::Debug]),
                older_than_ms: Some(500),
                series_mode: None,
            }),
            ..make_rule()
        };
        // completed_at=1000, older_than_ms=500, cutoff=500
        // event 0: type=log OK, level=Debug OK, timestamp 100 < 500 OK => kept
        // event 1: type=log OK, level=Info != Debug => excluded
        // event 2: type=progress != log => excluded
        // event 3: type=log OK, level=Debug OK, timestamp 600 >= 500 => excluded
        let result = filter_events_for_cleanup(&events, &rule, 2000.0, Some(1000.0));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].index, 0);
    }

    #[test]
    fn empty_events_returns_empty() {
        let events: Vec<TaskEvent> = vec![];
        let rule = make_rule();
        let result = filter_events_for_cleanup(&events, &rule, 999.0, None);
        assert!(result.is_empty());
    }
}
