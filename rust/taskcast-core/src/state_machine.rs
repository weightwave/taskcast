use crate::types::TaskStatus;

pub const TERMINAL_STATUSES: &[TaskStatus] = &[
    TaskStatus::Completed,
    TaskStatus::Failed,
    TaskStatus::Timeout,
    TaskStatus::Cancelled,
];

pub const SUSPENDED_STATUSES: &[TaskStatus] = &[
    TaskStatus::Paused,
    TaskStatus::Blocked,
];

pub fn allowed_transitions(from: &TaskStatus) -> &'static [TaskStatus] {
    match from {
        TaskStatus::Pending => &[TaskStatus::Assigned, TaskStatus::Running, TaskStatus::Paused, TaskStatus::Cancelled],
        TaskStatus::Assigned => &[TaskStatus::Running, TaskStatus::Pending, TaskStatus::Paused, TaskStatus::Cancelled],
        TaskStatus::Running => &[
            TaskStatus::Paused,
            TaskStatus::Blocked,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Timeout,
            TaskStatus::Cancelled,
        ],
        TaskStatus::Paused => &[TaskStatus::Running, TaskStatus::Assigned, TaskStatus::Blocked, TaskStatus::Cancelled],
        TaskStatus::Blocked => &[TaskStatus::Running, TaskStatus::Assigned, TaskStatus::Paused, TaskStatus::Cancelled, TaskStatus::Failed],
        TaskStatus::Completed
        | TaskStatus::Failed
        | TaskStatus::Timeout
        | TaskStatus::Cancelled => &[],
    }
}

pub fn can_transition(from: &TaskStatus, to: &TaskStatus) -> bool {
    if from == to {
        return false;
    }
    allowed_transitions(from).contains(to)
}

pub fn apply_transition(from: &TaskStatus, to: TaskStatus) -> Result<TaskStatus, String> {
    if !can_transition(from, &to) {
        return Err(format!("Invalid transition: {:?} \u{2192} {:?}", from, to));
    }
    Ok(to)
}

pub fn is_terminal(status: &TaskStatus) -> bool {
    TERMINAL_STATUSES.contains(status)
}

pub fn is_suspended(status: &TaskStatus) -> bool {
    SUSPENDED_STATUSES.contains(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── can_transition: valid transitions from Pending ──────────────────

    #[test]
    fn pending_to_running_is_valid() {
        assert!(can_transition(&TaskStatus::Pending, &TaskStatus::Running));
    }

    #[test]
    fn pending_to_cancelled_is_valid() {
        assert!(can_transition(&TaskStatus::Pending, &TaskStatus::Cancelled));
    }

    #[test]
    fn pending_to_paused_is_valid() {
        assert!(can_transition(&TaskStatus::Pending, &TaskStatus::Paused));
    }

    // ─── can_transition: invalid transitions from Pending ────────────────

    #[test]
    fn pending_to_completed_is_invalid() {
        assert!(!can_transition(&TaskStatus::Pending, &TaskStatus::Completed));
    }

    #[test]
    fn pending_to_failed_is_invalid() {
        assert!(!can_transition(&TaskStatus::Pending, &TaskStatus::Failed));
    }

    #[test]
    fn pending_to_timeout_is_invalid() {
        assert!(!can_transition(&TaskStatus::Pending, &TaskStatus::Timeout));
    }

    #[test]
    fn pending_to_blocked_is_invalid() {
        assert!(!can_transition(&TaskStatus::Pending, &TaskStatus::Blocked));
    }

    // ─── can_transition: valid transitions from Pending to Assigned ──────

    #[test]
    fn pending_to_assigned_is_valid() {
        assert!(can_transition(&TaskStatus::Pending, &TaskStatus::Assigned));
    }

    // ─── can_transition: valid transitions from Assigned ─────────────────

    #[test]
    fn assigned_to_running_is_valid() {
        assert!(can_transition(&TaskStatus::Assigned, &TaskStatus::Running));
    }

    #[test]
    fn assigned_to_pending_is_valid() {
        assert!(can_transition(&TaskStatus::Assigned, &TaskStatus::Pending));
    }

    #[test]
    fn assigned_to_paused_is_valid() {
        assert!(can_transition(&TaskStatus::Assigned, &TaskStatus::Paused));
    }

    #[test]
    fn assigned_to_cancelled_is_valid() {
        assert!(can_transition(
            &TaskStatus::Assigned,
            &TaskStatus::Cancelled
        ));
    }

    // ─── can_transition: invalid transitions from Assigned ───────────────

    #[test]
    fn assigned_to_completed_is_invalid() {
        assert!(!can_transition(
            &TaskStatus::Assigned,
            &TaskStatus::Completed
        ));
    }

    #[test]
    fn assigned_to_failed_is_invalid() {
        assert!(!can_transition(&TaskStatus::Assigned, &TaskStatus::Failed));
    }

    #[test]
    fn assigned_to_timeout_is_invalid() {
        assert!(!can_transition(&TaskStatus::Assigned, &TaskStatus::Timeout));
    }

    #[test]
    fn assigned_to_blocked_is_invalid() {
        assert!(!can_transition(&TaskStatus::Assigned, &TaskStatus::Blocked));
    }

    // ─── is_terminal: Assigned is not terminal ──────────────────────────

    #[test]
    fn assigned_is_not_terminal() {
        assert!(!is_terminal(&TaskStatus::Assigned));
    }

    // ─── can_transition: valid transitions from Running ──────────────────

    #[test]
    fn running_to_paused_is_valid() {
        assert!(can_transition(&TaskStatus::Running, &TaskStatus::Paused));
    }

    #[test]
    fn running_to_blocked_is_valid() {
        assert!(can_transition(&TaskStatus::Running, &TaskStatus::Blocked));
    }

    #[test]
    fn running_to_completed_is_valid() {
        assert!(can_transition(&TaskStatus::Running, &TaskStatus::Completed));
    }

    #[test]
    fn running_to_failed_is_valid() {
        assert!(can_transition(&TaskStatus::Running, &TaskStatus::Failed));
    }

    #[test]
    fn running_to_timeout_is_valid() {
        assert!(can_transition(&TaskStatus::Running, &TaskStatus::Timeout));
    }

    #[test]
    fn running_to_cancelled_is_valid() {
        assert!(can_transition(&TaskStatus::Running, &TaskStatus::Cancelled));
    }

    // ─── can_transition: invalid transitions from Running ────────────────

    #[test]
    fn running_to_pending_is_invalid() {
        assert!(!can_transition(&TaskStatus::Running, &TaskStatus::Pending));
    }

    // ─── can_transition: valid transitions from Paused ─────────────────

    #[test]
    fn paused_to_running_is_valid() {
        assert!(can_transition(&TaskStatus::Paused, &TaskStatus::Running));
    }

    #[test]
    fn paused_to_assigned_is_valid() {
        assert!(can_transition(&TaskStatus::Paused, &TaskStatus::Assigned));
    }

    #[test]
    fn paused_to_blocked_is_valid() {
        assert!(can_transition(&TaskStatus::Paused, &TaskStatus::Blocked));
    }

    #[test]
    fn paused_to_cancelled_is_valid() {
        assert!(can_transition(&TaskStatus::Paused, &TaskStatus::Cancelled));
    }

    // ─── can_transition: invalid transitions from Paused ───────────────

    #[test]
    fn paused_to_completed_is_invalid() {
        assert!(!can_transition(&TaskStatus::Paused, &TaskStatus::Completed));
    }

    #[test]
    fn paused_to_failed_is_invalid() {
        assert!(!can_transition(&TaskStatus::Paused, &TaskStatus::Failed));
    }

    #[test]
    fn paused_to_pending_is_invalid() {
        assert!(!can_transition(&TaskStatus::Paused, &TaskStatus::Pending));
    }

    #[test]
    fn paused_to_timeout_is_invalid() {
        assert!(!can_transition(&TaskStatus::Paused, &TaskStatus::Timeout));
    }

    // ─── can_transition: valid transitions from Blocked ─────────────────

    #[test]
    fn blocked_to_running_is_valid() {
        assert!(can_transition(&TaskStatus::Blocked, &TaskStatus::Running));
    }

    #[test]
    fn blocked_to_assigned_is_valid() {
        assert!(can_transition(&TaskStatus::Blocked, &TaskStatus::Assigned));
    }

    #[test]
    fn blocked_to_paused_is_valid() {
        assert!(can_transition(&TaskStatus::Blocked, &TaskStatus::Paused));
    }

    #[test]
    fn blocked_to_cancelled_is_valid() {
        assert!(can_transition(&TaskStatus::Blocked, &TaskStatus::Cancelled));
    }

    #[test]
    fn blocked_to_failed_is_valid() {
        assert!(can_transition(&TaskStatus::Blocked, &TaskStatus::Failed));
    }

    // ─── can_transition: invalid transitions from Blocked ───────────────

    #[test]
    fn blocked_to_completed_is_invalid() {
        assert!(!can_transition(&TaskStatus::Blocked, &TaskStatus::Completed));
    }

    #[test]
    fn blocked_to_pending_is_invalid() {
        assert!(!can_transition(&TaskStatus::Blocked, &TaskStatus::Pending));
    }

    #[test]
    fn blocked_to_timeout_is_invalid() {
        assert!(!can_transition(&TaskStatus::Blocked, &TaskStatus::Timeout));
    }

    // ─── can_transition: terminal states cannot transition ────────────────

    #[test]
    fn completed_to_any_is_invalid() {
        assert!(!can_transition(&TaskStatus::Completed, &TaskStatus::Pending));
        assert!(!can_transition(&TaskStatus::Completed, &TaskStatus::Running));
        assert!(!can_transition(&TaskStatus::Completed, &TaskStatus::Failed));
        assert!(!can_transition(&TaskStatus::Completed, &TaskStatus::Timeout));
        assert!(!can_transition(
            &TaskStatus::Completed,
            &TaskStatus::Cancelled
        ));
    }

    #[test]
    fn failed_to_any_is_invalid() {
        assert!(!can_transition(&TaskStatus::Failed, &TaskStatus::Pending));
        assert!(!can_transition(&TaskStatus::Failed, &TaskStatus::Running));
        assert!(!can_transition(&TaskStatus::Failed, &TaskStatus::Completed));
        assert!(!can_transition(&TaskStatus::Failed, &TaskStatus::Timeout));
        assert!(!can_transition(&TaskStatus::Failed, &TaskStatus::Cancelled));
    }

    #[test]
    fn timeout_to_any_is_invalid() {
        assert!(!can_transition(&TaskStatus::Timeout, &TaskStatus::Pending));
        assert!(!can_transition(&TaskStatus::Timeout, &TaskStatus::Running));
        assert!(!can_transition(&TaskStatus::Timeout, &TaskStatus::Completed));
        assert!(!can_transition(&TaskStatus::Timeout, &TaskStatus::Failed));
        assert!(!can_transition(
            &TaskStatus::Timeout,
            &TaskStatus::Cancelled
        ));
    }

    #[test]
    fn cancelled_to_any_is_invalid() {
        assert!(!can_transition(&TaskStatus::Cancelled, &TaskStatus::Pending));
        assert!(!can_transition(
            &TaskStatus::Cancelled,
            &TaskStatus::Running
        ));
        assert!(!can_transition(
            &TaskStatus::Cancelled,
            &TaskStatus::Completed
        ));
        assert!(!can_transition(&TaskStatus::Cancelled, &TaskStatus::Failed));
        assert!(!can_transition(
            &TaskStatus::Cancelled,
            &TaskStatus::Timeout
        ));
    }

    // ─── can_transition: same status → same status is invalid ────────────

    #[test]
    fn same_status_transition_is_invalid() {
        assert!(!can_transition(&TaskStatus::Pending, &TaskStatus::Pending));
        assert!(!can_transition(
            &TaskStatus::Assigned,
            &TaskStatus::Assigned
        ));
        assert!(!can_transition(&TaskStatus::Running, &TaskStatus::Running));
        assert!(!can_transition(&TaskStatus::Paused, &TaskStatus::Paused));
        assert!(!can_transition(&TaskStatus::Blocked, &TaskStatus::Blocked));
        assert!(!can_transition(
            &TaskStatus::Completed,
            &TaskStatus::Completed
        ));
        assert!(!can_transition(&TaskStatus::Failed, &TaskStatus::Failed));
        assert!(!can_transition(&TaskStatus::Timeout, &TaskStatus::Timeout));
        assert!(!can_transition(
            &TaskStatus::Cancelled,
            &TaskStatus::Cancelled
        ));
    }

    // ─── is_terminal ─────────────────────────────────────────────────────

    #[test]
    fn pending_is_not_terminal() {
        assert!(!is_terminal(&TaskStatus::Pending));
    }

    #[test]
    fn running_is_not_terminal() {
        assert!(!is_terminal(&TaskStatus::Running));
    }

    #[test]
    fn completed_is_terminal() {
        assert!(is_terminal(&TaskStatus::Completed));
    }

    #[test]
    fn failed_is_terminal() {
        assert!(is_terminal(&TaskStatus::Failed));
    }

    #[test]
    fn timeout_is_terminal() {
        assert!(is_terminal(&TaskStatus::Timeout));
    }

    #[test]
    fn cancelled_is_terminal() {
        assert!(is_terminal(&TaskStatus::Cancelled));
    }

    // ─── is_suspended ───────────────────────────────────────────────────

    #[test]
    fn paused_is_suspended() {
        assert!(is_suspended(&TaskStatus::Paused));
    }

    #[test]
    fn blocked_is_suspended() {
        assert!(is_suspended(&TaskStatus::Blocked));
    }

    #[test]
    fn pending_is_not_suspended() {
        assert!(!is_suspended(&TaskStatus::Pending));
    }

    #[test]
    fn running_is_not_suspended() {
        assert!(!is_suspended(&TaskStatus::Running));
    }

    #[test]
    fn completed_is_not_suspended() {
        assert!(!is_suspended(&TaskStatus::Completed));
    }

    // ─── apply_transition: success cases ─────────────────────────────────

    #[test]
    fn apply_transition_pending_to_running_succeeds() {
        let result = apply_transition(&TaskStatus::Pending, TaskStatus::Running);
        assert_eq!(result.unwrap(), TaskStatus::Running);
    }

    #[test]
    fn apply_transition_pending_to_assigned_succeeds() {
        let result = apply_transition(&TaskStatus::Pending, TaskStatus::Assigned);
        assert_eq!(result.unwrap(), TaskStatus::Assigned);
    }

    #[test]
    fn apply_transition_pending_to_paused_succeeds() {
        let result = apply_transition(&TaskStatus::Pending, TaskStatus::Paused);
        assert_eq!(result.unwrap(), TaskStatus::Paused);
    }

    #[test]
    fn apply_transition_pending_to_cancelled_succeeds() {
        let result = apply_transition(&TaskStatus::Pending, TaskStatus::Cancelled);
        assert_eq!(result.unwrap(), TaskStatus::Cancelled);
    }

    #[test]
    fn apply_transition_assigned_to_running_succeeds() {
        let result = apply_transition(&TaskStatus::Assigned, TaskStatus::Running);
        assert_eq!(result.unwrap(), TaskStatus::Running);
    }

    #[test]
    fn apply_transition_assigned_to_pending_succeeds() {
        let result = apply_transition(&TaskStatus::Assigned, TaskStatus::Pending);
        assert_eq!(result.unwrap(), TaskStatus::Pending);
    }

    #[test]
    fn apply_transition_assigned_to_paused_succeeds() {
        let result = apply_transition(&TaskStatus::Assigned, TaskStatus::Paused);
        assert_eq!(result.unwrap(), TaskStatus::Paused);
    }

    #[test]
    fn apply_transition_assigned_to_cancelled_succeeds() {
        let result = apply_transition(&TaskStatus::Assigned, TaskStatus::Cancelled);
        assert_eq!(result.unwrap(), TaskStatus::Cancelled);
    }

    #[test]
    fn apply_transition_running_to_completed_succeeds() {
        let result = apply_transition(&TaskStatus::Running, TaskStatus::Completed);
        assert_eq!(result.unwrap(), TaskStatus::Completed);
    }

    #[test]
    fn apply_transition_running_to_failed_succeeds() {
        let result = apply_transition(&TaskStatus::Running, TaskStatus::Failed);
        assert_eq!(result.unwrap(), TaskStatus::Failed);
    }

    #[test]
    fn apply_transition_running_to_timeout_succeeds() {
        let result = apply_transition(&TaskStatus::Running, TaskStatus::Timeout);
        assert_eq!(result.unwrap(), TaskStatus::Timeout);
    }

    #[test]
    fn apply_transition_running_to_cancelled_succeeds() {
        let result = apply_transition(&TaskStatus::Running, TaskStatus::Cancelled);
        assert_eq!(result.unwrap(), TaskStatus::Cancelled);
    }

    #[test]
    fn apply_transition_paused_to_assigned_succeeds() {
        let result = apply_transition(&TaskStatus::Paused, TaskStatus::Assigned);
        assert_eq!(result.unwrap(), TaskStatus::Assigned);
    }

    #[test]
    fn apply_transition_paused_to_running_succeeds() {
        let result = apply_transition(&TaskStatus::Paused, TaskStatus::Running);
        assert_eq!(result.unwrap(), TaskStatus::Running);
    }

    #[test]
    fn apply_transition_paused_to_blocked_succeeds() {
        let result = apply_transition(&TaskStatus::Paused, TaskStatus::Blocked);
        assert_eq!(result.unwrap(), TaskStatus::Blocked);
    }

    #[test]
    fn apply_transition_paused_to_cancelled_succeeds() {
        let result = apply_transition(&TaskStatus::Paused, TaskStatus::Cancelled);
        assert_eq!(result.unwrap(), TaskStatus::Cancelled);
    }

    #[test]
    fn apply_transition_blocked_to_assigned_succeeds() {
        let result = apply_transition(&TaskStatus::Blocked, TaskStatus::Assigned);
        assert_eq!(result.unwrap(), TaskStatus::Assigned);
    }

    #[test]
    fn apply_transition_blocked_to_running_succeeds() {
        let result = apply_transition(&TaskStatus::Blocked, TaskStatus::Running);
        assert_eq!(result.unwrap(), TaskStatus::Running);
    }

    #[test]
    fn apply_transition_blocked_to_paused_succeeds() {
        let result = apply_transition(&TaskStatus::Blocked, TaskStatus::Paused);
        assert_eq!(result.unwrap(), TaskStatus::Paused);
    }

    #[test]
    fn apply_transition_blocked_to_cancelled_succeeds() {
        let result = apply_transition(&TaskStatus::Blocked, TaskStatus::Cancelled);
        assert_eq!(result.unwrap(), TaskStatus::Cancelled);
    }

    #[test]
    fn apply_transition_blocked_to_failed_succeeds() {
        let result = apply_transition(&TaskStatus::Blocked, TaskStatus::Failed);
        assert_eq!(result.unwrap(), TaskStatus::Failed);
    }

    // ─── apply_transition: error cases ───────────────────────────────────

    #[test]
    fn apply_transition_invalid_returns_error() {
        let result = apply_transition(&TaskStatus::Pending, TaskStatus::Completed);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid transition"));
        assert!(err.contains("Pending"));
        assert!(err.contains("Completed"));
    }

    #[test]
    fn apply_transition_from_terminal_returns_error() {
        let result = apply_transition(&TaskStatus::Completed, TaskStatus::Running);
        assert!(result.is_err());
    }

    #[test]
    fn apply_transition_same_status_returns_error() {
        let result = apply_transition(&TaskStatus::Running, TaskStatus::Running);
        assert!(result.is_err());
    }

    // ─── allowed_transitions ─────────────────────────────────────────────

    #[test]
    fn allowed_transitions_from_pending() {
        let transitions = allowed_transitions(&TaskStatus::Pending);
        assert_eq!(transitions.len(), 4);
        assert!(transitions.contains(&TaskStatus::Assigned));
        assert!(transitions.contains(&TaskStatus::Running));
        assert!(transitions.contains(&TaskStatus::Paused));
        assert!(transitions.contains(&TaskStatus::Cancelled));
    }

    #[test]
    fn allowed_transitions_from_assigned() {
        let transitions = allowed_transitions(&TaskStatus::Assigned);
        assert_eq!(transitions.len(), 4);
        assert!(transitions.contains(&TaskStatus::Running));
        assert!(transitions.contains(&TaskStatus::Pending));
        assert!(transitions.contains(&TaskStatus::Paused));
        assert!(transitions.contains(&TaskStatus::Cancelled));
    }

    #[test]
    fn allowed_transitions_from_running() {
        let transitions = allowed_transitions(&TaskStatus::Running);
        assert_eq!(transitions.len(), 6);
        assert!(transitions.contains(&TaskStatus::Paused));
        assert!(transitions.contains(&TaskStatus::Blocked));
        assert!(transitions.contains(&TaskStatus::Completed));
        assert!(transitions.contains(&TaskStatus::Failed));
        assert!(transitions.contains(&TaskStatus::Timeout));
        assert!(transitions.contains(&TaskStatus::Cancelled));
    }

    #[test]
    fn allowed_transitions_from_paused() {
        let transitions = allowed_transitions(&TaskStatus::Paused);
        assert_eq!(transitions.len(), 4);
        assert!(transitions.contains(&TaskStatus::Running));
        assert!(transitions.contains(&TaskStatus::Assigned));
        assert!(transitions.contains(&TaskStatus::Blocked));
        assert!(transitions.contains(&TaskStatus::Cancelled));
    }

    #[test]
    fn allowed_transitions_from_blocked() {
        let transitions = allowed_transitions(&TaskStatus::Blocked);
        assert_eq!(transitions.len(), 5);
        assert!(transitions.contains(&TaskStatus::Running));
        assert!(transitions.contains(&TaskStatus::Assigned));
        assert!(transitions.contains(&TaskStatus::Paused));
        assert!(transitions.contains(&TaskStatus::Cancelled));
        assert!(transitions.contains(&TaskStatus::Failed));
    }

    #[test]
    fn allowed_transitions_from_terminal_states_are_empty() {
        assert!(allowed_transitions(&TaskStatus::Completed).is_empty());
        assert!(allowed_transitions(&TaskStatus::Failed).is_empty());
        assert!(allowed_transitions(&TaskStatus::Timeout).is_empty());
        assert!(allowed_transitions(&TaskStatus::Cancelled).is_empty());
    }

    // ─── TERMINAL_STATUSES constant ──────────────────────────────────────

    #[test]
    fn terminal_statuses_contains_exactly_four() {
        assert_eq!(TERMINAL_STATUSES.len(), 4);
        assert!(TERMINAL_STATUSES.contains(&TaskStatus::Completed));
        assert!(TERMINAL_STATUSES.contains(&TaskStatus::Failed));
        assert!(TERMINAL_STATUSES.contains(&TaskStatus::Timeout));
        assert!(TERMINAL_STATUSES.contains(&TaskStatus::Cancelled));
    }

    // ─── SUSPENDED_STATUSES constant ─────────────────────────────────────

    #[test]
    fn suspended_statuses_contains_exactly_two() {
        assert_eq!(SUSPENDED_STATUSES.len(), 2);
        assert!(SUSPENDED_STATUSES.contains(&TaskStatus::Paused));
        assert!(SUSPENDED_STATUSES.contains(&TaskStatus::Blocked));
    }
}