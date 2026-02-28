use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use taskcast_core::{matches_filter, BackoffStrategy, RetryConfig, TaskEvent, WebhookConfig};

// ─── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("Webhook delivery failed after {attempts} attempts: {message}")]
    DeliveryFailed { attempts: u32, message: String },
}

// ─── Default Retry Config ───────────────────────────────────────────────────

fn default_retry() -> RetryConfig {
    RetryConfig {
        retries: 3,
        backoff: BackoffStrategy::Exponential,
        initial_delay_ms: 1000,
        max_delay_ms: 30000,
        timeout_ms: 5000,
    }
}

fn merge_retry(config_retry: Option<&RetryConfig>) -> RetryConfig {
    match config_retry {
        Some(r) => r.clone(),
        None => default_retry(),
    }
}

// ─── WebhookDelivery ────────────────────────────────────────────────────────

pub struct WebhookDelivery {
    client: reqwest::Client,
}

impl WebhookDelivery {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub async fn send(
        &self,
        event: &TaskEvent,
        config: &WebhookConfig,
    ) -> Result<(), WebhookError> {
        // Check filter
        if let Some(ref filter) = config.filter {
            if !matches_filter(event, filter) {
                return Ok(());
            }
        }

        let retry = merge_retry(config.retry.as_ref());
        let body = serde_json::to_string(event).unwrap();
        let timestamp = format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        let signature = config.secret.as_ref().map(|s| Self::sign(&body, s));

        let mut last_error: Option<String> = None;

        for attempt in 0..=retry.retries {
            if attempt > 0 {
                let delay = Self::backoff_ms(&retry, attempt);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }

            let mut req = self
                .client
                .post(&config.url)
                .header("Content-Type", "application/json")
                .header("X-Taskcast-Event", &event.r#type)
                .header("X-Taskcast-Timestamp", &timestamp)
                .timeout(Duration::from_millis(retry.timeout_ms))
                .body(body.clone());

            if let Some(ref sig) = signature {
                req = req.header("X-Taskcast-Signature", sig);
            }

            match req.send().await {
                Ok(res) if res.status().is_success() => return Ok(()),
                Ok(res) => {
                    last_error = Some(format!("HTTP {}", res.status().as_u16()));
                }
                Err(err) => {
                    last_error = Some(err.to_string());
                }
            }
        }

        Err(WebhookError::DeliveryFailed {
            attempts: retry.retries + 1,
            message: last_error.unwrap_or_else(|| "Unknown error".to_string()),
        })
    }

    fn sign(body: &str, secret: &str) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(body.as_bytes());
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn backoff_ms(retry: &RetryConfig, attempt: u32) -> u64 {
        match retry.backoff {
            BackoffStrategy::Fixed => retry.initial_delay_ms,
            BackoffStrategy::Linear => retry.initial_delay_ms * attempt as u64,
            BackoffStrategy::Exponential => {
                (retry.initial_delay_ms * 2u64.pow(attempt - 1)).min(retry.max_delay_ms)
            }
        }
    }
}

impl Default for WebhookDelivery {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use taskcast_core::{Level, SubscribeFilter};

    #[test]
    fn sign_produces_correct_hmac_sha256() {
        let body = r#"{"type":"progress","data":{"percent":50}}"#;
        let secret = "my-secret-key";
        let result = WebhookDelivery::sign(body, secret);
        assert!(result.starts_with("sha256="));
        // Verify it's a valid hex string after the prefix
        let hex_part = &result[7..];
        assert_eq!(hex_part.len(), 64); // SHA-256 produces 32 bytes = 64 hex chars
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sign_different_secrets_produce_different_signatures() {
        let body = r#"{"type":"test"}"#;
        let sig1 = WebhookDelivery::sign(body, "secret1");
        let sig2 = WebhookDelivery::sign(body, "secret2");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn sign_same_input_produces_same_signature() {
        let body = r#"{"type":"test"}"#;
        let sig1 = WebhookDelivery::sign(body, "secret");
        let sig2 = WebhookDelivery::sign(body, "secret");
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn backoff_fixed_returns_initial_delay() {
        let retry = RetryConfig {
            retries: 3,
            backoff: BackoffStrategy::Fixed,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            timeout_ms: 5000,
        };
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 1), 1000);
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 2), 1000);
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 3), 1000);
    }

    #[test]
    fn backoff_linear_scales_with_attempt() {
        let retry = RetryConfig {
            retries: 3,
            backoff: BackoffStrategy::Linear,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            timeout_ms: 5000,
        };
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 1), 1000);
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 2), 2000);
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 3), 3000);
    }

    #[test]
    fn backoff_exponential_doubles_each_attempt() {
        let retry = RetryConfig {
            retries: 5,
            backoff: BackoffStrategy::Exponential,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            timeout_ms: 5000,
        };
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 1), 1000); // 1000 * 2^0
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 2), 2000); // 1000 * 2^1
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 3), 4000); // 1000 * 2^2
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 4), 8000); // 1000 * 2^3
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 5), 16000); // 1000 * 2^4
    }

    #[test]
    fn backoff_exponential_respects_max_delay() {
        let retry = RetryConfig {
            retries: 10,
            backoff: BackoffStrategy::Exponential,
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
            timeout_ms: 5000,
        };
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 1), 1000);
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 2), 2000);
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 3), 4000);
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 4), 5000); // capped at max_delay_ms
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 5), 5000); // still capped
    }

    #[test]
    fn default_retry_has_expected_values() {
        let retry = default_retry();
        assert_eq!(retry.retries, 3);
        assert_eq!(retry.backoff, BackoffStrategy::Exponential);
        assert_eq!(retry.initial_delay_ms, 1000);
        assert_eq!(retry.max_delay_ms, 30000);
        assert_eq!(retry.timeout_ms, 5000);
    }

    fn make_test_event() -> TaskEvent {
        TaskEvent {
            id: "evt_01".to_string(),
            task_id: "task_01".to_string(),
            index: 0,
            timestamp: 1700000000000.0,
            r#type: "progress".to_string(),
            level: Level::Info,
            data: serde_json::json!({ "percent": 50 }),
            series_id: None,
            series_mode: None,
        }
    }

    #[tokio::test]
    async fn send_skips_when_filter_does_not_match() {
        let delivery = WebhookDelivery::new();
        let event = make_test_event();
        let config = WebhookConfig {
            url: "http://localhost:9999/hook".to_string(),
            filter: Some(SubscribeFilter {
                types: Some(vec!["log".to_string()]), // does NOT match "progress"
                levels: None,
                include_status: None,
                wrap: None,
                since: None,
            }),
            secret: None,
            wrap: None,
            retry: None,
        };
        // Should return Ok(()) without attempting to send because filter doesn't match
        let result = delivery.send(&event, &config).await;
        assert!(result.is_ok());
    }
}
