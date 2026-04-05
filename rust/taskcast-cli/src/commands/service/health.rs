use std::time::{Duration, Instant};

pub async fn poll_health(port: u16, timeout_ms: u64, interval_ms: u64) -> bool {
    let url = format!("http://localhost:{port}/health");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    while Instant::now() < deadline {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => return true,
            _ => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let sleep_dur = Duration::from_millis(interval_ms).min(remaining);
        if sleep_dur.is_zero() {
            break;
        }
        tokio::time::sleep(sleep_dur).await;
    }

    false
}

pub async fn fetch_health_detail(port: u16) -> Option<(Option<f64>, Option<String>)> {
    let url = format!("http://localhost:{port}/health/detail");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;
    let uptime = body.get("uptime").and_then(|v| v.as_f64());
    let storage = body
        .get("adapters")
        .and_then(|a| a.get("shortTermStore"))
        .and_then(|s| s.get("provider"))
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());

    Some((uptime, storage))
}

pub fn format_uptime(seconds: f64) -> String {
    let total_secs = seconds as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_hours_and_minutes() {
        assert_eq!(format_uptime(9000.0), "2h 30m");
    }

    #[test]
    fn format_uptime_minutes_only() {
        assert_eq!(format_uptime(300.0), "5m");
    }

    #[test]
    fn format_uptime_zero() {
        assert_eq!(format_uptime(0.0), "0m");
    }

    #[test]
    fn format_uptime_less_than_minute() {
        assert_eq!(format_uptime(45.0), "0m");
    }

    #[test]
    fn format_uptime_exact_hour() {
        assert_eq!(format_uptime(3600.0), "1h 0m");
    }

    #[tokio::test]
    async fn poll_health_returns_false_on_no_server() {
        let result = poll_health(19999, 500, 100).await;
        assert!(!result);
    }
}
