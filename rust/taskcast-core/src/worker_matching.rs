use crate::filter::matches_type;
use crate::types::{TagMatcher, Task, WorkerMatchRule};

/// Checks if a set of task tags matches a `TagMatcher`.
///
/// - Empty `TagMatcher` (all fields `None`) matches anything.
/// - `all`: every tag in the list must exist in `task_tags` (AND).
/// - `any`: at least one tag must exist in `task_tags` (OR).
/// - `none`: none of these tags may exist in `task_tags` (NOT).
/// - All three conditions are ANDed together.
/// - If `task_tags` is `None`, treat as empty slice.
/// - Empty arrays in matcher fields are vacuous true.
pub fn matches_tag(task_tags: Option<&[String]>, matcher: &TagMatcher) -> bool {
    let tags = task_tags.unwrap_or(&[]);

    if let Some(ref all) = matcher.all {
        if !all.is_empty() && !all.iter().all(|tag| tags.contains(tag)) {
            return false;
        }
    }

    if let Some(ref any) = matcher.any {
        if !any.is_empty() && !any.iter().any(|tag| tags.contains(tag)) {
            return false;
        }
    }

    if let Some(ref none) = matcher.none {
        if !none.is_empty() && none.iter().any(|tag| tags.contains(tag)) {
            return false;
        }
    }

    true
}

/// Checks if a task matches a `WorkerMatchRule`.
///
/// - If rule has `task_types`: task.type must match using wildcard matching
///   via `crate::filter::matches_type`.
/// - If rule has `tags`: use `matches_tag(task.tags, rule.tags)`.
/// - Both conditions are ANDed.
/// - Empty/no rule matches everything.
/// - Task with no `type` does not match if rule has `task_types`.
pub fn matches_worker_rule(task: &Task, rule: &WorkerMatchRule) -> bool {
    if let Some(ref task_types) = rule.task_types {
        if !task_types.is_empty() {
            match task.r#type {
                None => return false,
                Some(ref task_type) => {
                    if !matches_type(task_type, Some(task_types)) {
                        return false;
                    }
                }
            }
        }
    }

    if let Some(ref tags) = rule.tags {
        if !matches_tag(task.tags.as_deref(), tags) {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TagMatcher, Task, TaskStatus, WorkerMatchRule};

    // ─── Helper ──────────────────────────────────────────────────────────

    fn make_task(task_type: Option<&str>, tags: Option<Vec<&str>>) -> Task {
        Task {
            id: "test-task".to_string(),
            r#type: task_type.map(|s| s.to_string()),
            status: TaskStatus::Pending,
            params: None,
            result: None,
            error: None,
            metadata: None,
            created_at: 1000.0,
            updated_at: 1000.0,
            completed_at: None,
            ttl: None,
            auth_config: None,
            webhooks: None,
            cleanup: None,
            tags: tags.map(|t| t.into_iter().map(|s| s.to_string()).collect()),
            assign_mode: None,
            cost: None,
            assigned_worker: None,
            disconnect_policy: None,
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ─── matches_tag: empty matcher matches everything ───────────────────

    #[test]
    fn empty_matcher_matches_any_tags() {
        let matcher = TagMatcher::default();
        assert!(matches_tag(Some(&strs(&["gpu", "fast"])), &matcher));
    }

    #[test]
    fn empty_matcher_matches_none_tags() {
        let matcher = TagMatcher::default();
        assert!(matches_tag(None, &matcher));
    }

    #[test]
    fn empty_matcher_matches_empty_tags() {
        let matcher = TagMatcher::default();
        assert!(matches_tag(Some(&[]), &matcher));
    }

    // ─── matches_tag: all condition ──────────────────────────────────────

    #[test]
    fn all_matches_when_all_present() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu", "fast"])),
            ..Default::default()
        };
        assert!(matches_tag(
            Some(&strs(&["gpu", "fast", "large"])),
            &matcher
        ));
    }

    #[test]
    fn all_fails_when_one_missing() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu", "fast"])),
            ..Default::default()
        };
        assert!(!matches_tag(Some(&strs(&["gpu"])), &matcher));
    }

    #[test]
    fn all_fails_when_tags_empty() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu"])),
            ..Default::default()
        };
        assert!(!matches_tag(Some(&[]), &matcher));
    }

    #[test]
    fn all_fails_when_tags_none() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu"])),
            ..Default::default()
        };
        assert!(!matches_tag(None, &matcher));
    }

    #[test]
    fn empty_all_array_is_vacuous_true() {
        let matcher = TagMatcher {
            all: Some(vec![]),
            ..Default::default()
        };
        assert!(matches_tag(Some(&[]), &matcher));
        assert!(matches_tag(None, &matcher));
    }

    // ─── matches_tag: any condition ──────────────────────────────────────

    #[test]
    fn any_matches_when_at_least_one_present() {
        let matcher = TagMatcher {
            any: Some(strs(&["gpu", "tpu"])),
            ..Default::default()
        };
        assert!(matches_tag(Some(&strs(&["gpu", "slow"])), &matcher));
    }

    #[test]
    fn any_fails_when_none_present() {
        let matcher = TagMatcher {
            any: Some(strs(&["gpu", "tpu"])),
            ..Default::default()
        };
        assert!(!matches_tag(Some(&strs(&["cpu"])), &matcher));
    }

    #[test]
    fn any_fails_when_tags_none() {
        let matcher = TagMatcher {
            any: Some(strs(&["gpu"])),
            ..Default::default()
        };
        assert!(!matches_tag(None, &matcher));
    }

    #[test]
    fn empty_any_array_is_vacuous_true() {
        let matcher = TagMatcher {
            any: Some(vec![]),
            ..Default::default()
        };
        assert!(matches_tag(Some(&strs(&["anything"])), &matcher));
        assert!(matches_tag(None, &matcher));
    }

    // ─── matches_tag: none condition ─────────────────────────────────────

    #[test]
    fn none_matches_when_excluded_tags_absent() {
        let matcher = TagMatcher {
            none: Some(strs(&["slow"])),
            ..Default::default()
        };
        assert!(matches_tag(Some(&strs(&["gpu", "fast"])), &matcher));
    }

    #[test]
    fn none_fails_when_excluded_tag_present() {
        let matcher = TagMatcher {
            none: Some(strs(&["slow"])),
            ..Default::default()
        };
        assert!(!matches_tag(Some(&strs(&["gpu", "slow"])), &matcher));
    }

    #[test]
    fn none_matches_when_tags_none() {
        let matcher = TagMatcher {
            none: Some(strs(&["slow"])),
            ..Default::default()
        };
        assert!(matches_tag(None, &matcher));
    }

    #[test]
    fn empty_none_array_is_vacuous_true() {
        let matcher = TagMatcher {
            none: Some(vec![]),
            ..Default::default()
        };
        assert!(matches_tag(Some(&strs(&["anything"])), &matcher));
    }

    // ─── matches_tag: combined conditions ────────────────────────────────

    #[test]
    fn combined_all_and_any_passes_when_both_satisfied() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu"])),
            any: Some(strs(&["fast", "medium"])),
            none: None,
        };
        assert!(matches_tag(
            Some(&strs(&["gpu", "fast"])),
            &matcher
        ));
    }

    #[test]
    fn combined_all_and_any_fails_when_all_unsatisfied() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu"])),
            any: Some(strs(&["fast", "medium"])),
            none: None,
        };
        assert!(!matches_tag(Some(&strs(&["fast"])), &matcher));
    }

    #[test]
    fn combined_all_and_none_fails_when_none_violated() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu"])),
            any: None,
            none: Some(strs(&["slow"])),
        };
        assert!(!matches_tag(
            Some(&strs(&["gpu", "slow"])),
            &matcher
        ));
    }

    #[test]
    fn combined_all_any_none_passes_when_all_conditions_met() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu"])),
            any: Some(strs(&["fast", "medium"])),
            none: Some(strs(&["slow"])),
        };
        assert!(matches_tag(
            Some(&strs(&["gpu", "fast"])),
            &matcher
        ));
    }

    #[test]
    fn combined_all_any_none_fails_when_none_violated() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu"])),
            any: Some(strs(&["fast", "medium"])),
            none: Some(strs(&["slow"])),
        };
        assert!(!matches_tag(
            Some(&strs(&["gpu", "fast", "slow"])),
            &matcher
        ));
    }

    // ─── matches_worker_rule: empty rule matches everything ──────────────

    #[test]
    fn empty_rule_matches_any_task() {
        let rule = WorkerMatchRule::default();
        let task = make_task(Some("llm.generate"), Some(vec!["gpu"]));
        assert!(matches_worker_rule(&task, &rule));
    }

    #[test]
    fn empty_rule_matches_task_with_no_type() {
        let rule = WorkerMatchRule::default();
        let task = make_task(None, None);
        assert!(matches_worker_rule(&task, &rule));
    }

    // ─── matches_worker_rule: task_types matching ────────────────────────

    #[test]
    fn task_types_exact_match() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["llm.generate"])),
            tags: None,
        };
        let task = make_task(Some("llm.generate"), None);
        assert!(matches_worker_rule(&task, &rule));
    }

    #[test]
    fn task_types_wildcard_match() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["llm.*"])),
            tags: None,
        };
        let task = make_task(Some("llm.generate"), None);
        assert!(matches_worker_rule(&task, &rule));
    }

    #[test]
    fn task_types_wildcard_does_not_match_exact_prefix() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["llm.*"])),
            tags: None,
        };
        let task = make_task(Some("llm"), None);
        assert!(!matches_worker_rule(&task, &rule));
    }

    #[test]
    fn task_types_no_match() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["render.*"])),
            tags: None,
        };
        let task = make_task(Some("llm.generate"), None);
        assert!(!matches_worker_rule(&task, &rule));
    }

    #[test]
    fn task_types_fails_when_task_has_no_type() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["llm.*"])),
            tags: None,
        };
        let task = make_task(None, None);
        assert!(!matches_worker_rule(&task, &rule));
    }

    #[test]
    fn task_types_star_matches_all() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["*"])),
            tags: None,
        };
        let task = make_task(Some("anything"), None);
        assert!(matches_worker_rule(&task, &rule));
    }

    #[test]
    fn task_types_empty_array_matches_everything() {
        // Empty task_types array: matches_type with empty patterns returns false,
        // but we only enter the filter if !task_types.is_empty()
        let rule = WorkerMatchRule {
            task_types: Some(vec![]),
            tags: None,
        };
        let task = make_task(Some("anything"), None);
        assert!(matches_worker_rule(&task, &rule));
    }

    // ─── matches_worker_rule: tag matching ───────────────────────────────

    #[test]
    fn tags_match() {
        let rule = WorkerMatchRule {
            task_types: None,
            tags: Some(TagMatcher {
                all: Some(strs(&["gpu"])),
                ..Default::default()
            }),
        };
        let task = make_task(None, Some(vec!["gpu", "fast"]));
        assert!(matches_worker_rule(&task, &rule));
    }

    #[test]
    fn tags_no_match() {
        let rule = WorkerMatchRule {
            task_types: None,
            tags: Some(TagMatcher {
                all: Some(strs(&["gpu"])),
                ..Default::default()
            }),
        };
        let task = make_task(None, Some(vec!["cpu"]));
        assert!(!matches_worker_rule(&task, &rule));
    }

    #[test]
    fn tags_with_none_task_tags_uses_empty() {
        let rule = WorkerMatchRule {
            task_types: None,
            tags: Some(TagMatcher {
                any: Some(strs(&["gpu"])),
                ..Default::default()
            }),
        };
        let task = make_task(None, None);
        assert!(!matches_worker_rule(&task, &rule));
    }

    // ─── matches_worker_rule: combined task_types and tags ───────────────

    #[test]
    fn combined_types_and_tags_both_match() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["llm.*"])),
            tags: Some(TagMatcher {
                all: Some(strs(&["gpu"])),
                ..Default::default()
            }),
        };
        let task = make_task(Some("llm.generate"), Some(vec!["gpu"]));
        assert!(matches_worker_rule(&task, &rule));
    }

    #[test]
    fn combined_types_match_but_tags_dont() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["llm.*"])),
            tags: Some(TagMatcher {
                all: Some(strs(&["gpu"])),
                ..Default::default()
            }),
        };
        let task = make_task(Some("llm.generate"), Some(vec!["cpu"]));
        assert!(!matches_worker_rule(&task, &rule));
    }

    #[test]
    fn combined_tags_match_but_types_dont() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["render.*"])),
            tags: Some(TagMatcher {
                all: Some(strs(&["gpu"])),
                ..Default::default()
            }),
        };
        let task = make_task(Some("llm.generate"), Some(vec!["gpu"]));
        assert!(!matches_worker_rule(&task, &rule));
    }

    // ─── matches_worker_rule: multiple task_types patterns ───────────────

    #[test]
    fn multiple_task_types_matches_any() {
        let rule = WorkerMatchRule {
            task_types: Some(strs(&["llm.*", "render.*"])),
            tags: None,
        };
        let task1 = make_task(Some("llm.generate"), None);
        let task2 = make_task(Some("render.video"), None);
        let task3 = make_task(Some("process.data"), None);
        assert!(matches_worker_rule(&task1, &rule));
        assert!(matches_worker_rule(&task2, &rule));
        assert!(!matches_worker_rule(&task3, &rule));
    }

    // ─── matches_tag: edge case — duplicate tags ─────────────────────────

    #[test]
    fn duplicate_tags_handled_correctly() {
        let matcher = TagMatcher {
            all: Some(strs(&["gpu", "gpu"])),
            ..Default::default()
        };
        assert!(matches_tag(Some(&strs(&["gpu"])), &matcher));
    }

    // ─── matches_tag: edge case — empty string tags ──────────────────────

    #[test]
    fn empty_string_tags_match() {
        let matcher = TagMatcher {
            all: Some(strs(&[""])),
            ..Default::default()
        };
        assert!(matches_tag(Some(&strs(&[""])), &matcher));
        assert!(!matches_tag(Some(&strs(&["gpu"])), &matcher));
    }
}
