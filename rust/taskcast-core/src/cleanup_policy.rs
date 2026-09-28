use crate::config::{ConfigError, TaskcastConfig};
use crate::filter::matches_type;
use crate::state_machine::is_terminal;
use crate::types::{CleanupConfig, CleanupRule, CleanupTarget, Task};
use std::collections::HashMap;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Default)]
pub struct ResolvedCleanupConfig {
    pub enabled: bool,
    pub rules: Vec<CleanupRule>,
}

pub struct ResolvedTaskCleanupPolicy {
    pub cleanup: CleanupConfig,
    pub cleanup_policy_version: u8,
    pub cleanup_resolved_at: f64,
}

pub fn validate_cleanup_rules(rules: &[CleanupRule]) -> Result<(), ConfigError> {
    for rule in rules {
        if rule.target == CleanupTarget::Task || rule.event_filter.is_some() {
            return Err(ConfigError::InvalidValue(
                "Cleanup supports only whole events/all targets, without eventFilter".into(),
            ));
        }
        if rule.trigger.after_ms.is_some_and(|v| v > MAX_SAFE_INTEGER) {
            return Err(ConfigError::InvalidValue(
                "cleanup afterMs must be a nonnegative safe integer".into(),
            ));
        }
    }
    Ok(())
}

pub fn resolve_cleanup_config(
    config: &TaskcastConfig,
    env: &HashMap<String, String>,
) -> Result<ResolvedCleanupConfig, ConfigError> {
    let file = config.cleanup.as_ref();
    let enabled = match env.get("TASKCAST_CLEANUP_ENABLED").map(String::as_str) {
        Some("true") => true,
        Some("false") => false,
        None => file.and_then(|c| c.enabled).unwrap_or(false),
        _ => {
            return Err(ConfigError::InvalidValue(
                "TASKCAST_CLEANUP_ENABLED must be true or false".into(),
            ))
        }
    };
    if !enabled {
        return Ok(ResolvedCleanupConfig::default());
    }
    let mut rules = Vec::new();
    for value in file.and_then(|c| c.rules.as_ref()).into_iter().flatten() {
        // Reject even an explicit null filter rather than silently widening it.
        if value.get("eventFilter").is_some() {
            return Err(ConfigError::InvalidValue(
                "Cleanup eventFilter is unsupported".into(),
            ));
        }
        rules.push(serde_json::from_value(value.clone())?);
    }
    validate_cleanup_rules(&rules)?;
    Ok(ResolvedCleanupConfig { enabled, rules })
}

pub fn resolve_task_cleanup_policy(
    task_type: Option<&str>,
    override_policy: Option<&CleanupConfig>,
    config: &ResolvedCleanupConfig,
    now: f64,
) -> Result<Option<ResolvedTaskCleanupPolicy>, ConfigError> {
    if !config.enabled {
        return Ok(None);
    }
    let rules = override_policy.map(|p| &p.rules).unwrap_or(&config.rules);
    validate_cleanup_rules(rules)?;
    let rules = rules
        .iter()
        .filter(|rule| {
            override_policy.is_some()
                || rule
                    .r#match
                    .as_ref()
                    .and_then(|m| m.task_types.as_ref())
                    .is_none_or(|patterns| {
                        task_type.is_some_and(|ty| matches_type(ty, Some(patterns)))
                    })
        })
        .cloned()
        .collect();
    Ok(Some(ResolvedTaskCleanupPolicy {
        cleanup: CleanupConfig { rules },
        cleanup_policy_version: 1,
        cleanup_resolved_at: now,
    }))
}

pub fn cleanup_deadline(task: &Task, target: CleanupTarget) -> Option<f64> {
    if task.cleanup_policy_version != Some(1) || !is_terminal(&task.status) {
        return None;
    }
    let completed = task.completed_at?;
    if !completed.is_finite()
        || completed < 0.0
        || completed.fract() != 0.0
        || completed > MAX_SAFE_INTEGER as f64
    {
        return None;
    }
    task.cleanup
        .as_ref()?
        .rules
        .iter()
        .filter_map(|rule| {
            if rule.target != target || rule.event_filter.is_some() {
                return None;
            }
            if let Some(m) = &rule.r#match {
                if m.status.as_ref().is_some_and(|s| !s.contains(&task.status)) {
                    return None;
                }
                if let Some(patterns) = &m.task_types {
                    if !task
                        .r#type
                        .as_ref()
                        .is_some_and(|ty| matches_type(ty, Some(patterns)))
                    {
                        return None;
                    }
                }
            }
            let after = rule.trigger.after_ms.unwrap_or(0);
            let due = completed + after as f64;
            (after <= MAX_SAFE_INTEGER && due <= MAX_SAFE_INTEGER as f64).then_some(due)
        })
        .min_by(f64::total_cmp)
}
