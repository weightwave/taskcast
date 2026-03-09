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
    use std::sync::Arc;
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
            series_acc_field: None,
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

    #[tokio::test]
    async fn send_fails_after_retries_on_server_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let mock_app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move || {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let delivery = WebhookDelivery::new();
        let event = make_test_event();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: None,
            wrap: None,
            retry: Some(RetryConfig {
                retries: 2,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 1,
                max_delay_ms: 1,
                timeout_ms: 5000,
            }),
        };

        let result = delivery.send(&event, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let WebhookError::DeliveryFailed { attempts, .. } = err;
        assert_eq!(attempts, 3); // 1 initial + 2 retries
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn send_succeeds_after_transient_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let mock_app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move || {
                let count = call_count_clone.clone();
                async move {
                    let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n < 2 {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        axum::http::StatusCode::OK
                    }
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let delivery = WebhookDelivery::new();
        let event = make_test_event();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: None,
            wrap: None,
            retry: Some(RetryConfig {
                retries: 3,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 1,
                max_delay_ms: 1,
                timeout_ms: 5000,
            }),
        };

        let result = delivery.send(&event, &config).await;
        assert!(result.is_ok());
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn send_timeout_counts_as_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let mock_app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move || {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let delivery = WebhookDelivery::new();
        let event = make_test_event();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: None,
            wrap: None,
            retry: Some(RetryConfig {
                retries: 1,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 1,
                max_delay_ms: 1,
                timeout_ms: 50, // Very short timeout
            }),
        };

        let result = delivery.send(&event, &config).await;
        assert!(result.is_err());
        // Should have attempted 2 times (initial + 1 retry)
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn send_dns_failure_retries_and_fails() {
        let delivery = WebhookDelivery::new();
        let event = make_test_event();
        let config = WebhookConfig {
            url: "http://nonexistent.invalid:9999/hook".to_string(),
            filter: None,
            secret: None,
            wrap: None,
            retry: Some(RetryConfig {
                retries: 1,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 1,
                max_delay_ms: 1,
                timeout_ms: 1000,
            }),
        };

        let result = delivery.send(&event, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_without_secret_omits_signature_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let had_signature = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let had_signature_clone = had_signature.clone();

        let mock_app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move |headers: axum::http::HeaderMap| {
                let sig = had_signature_clone.clone();
                async move {
                    if headers.contains_key("x-taskcast-signature") {
                        sig.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let delivery = WebhookDelivery::new();
        let event = make_test_event();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: None, // No secret
            wrap: None,
            retry: Some(RetryConfig {
                retries: 0,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 0,
                max_delay_ms: 0,
                timeout_ms: 5000,
            }),
        };

        delivery.send(&event, &config).await.unwrap();
        assert!(!had_signature.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn send_retries_and_succeeds_on_second_attempt() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let mock_app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move || {
                let count = call_count_clone.clone();
                async move {
                    let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        axum::http::StatusCode::OK
                    }
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let delivery = WebhookDelivery::new();
        let event = make_test_event();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: None,
            wrap: None,
            retry: Some(RetryConfig {
                retries: 3,
                backoff: BackoffStrategy::Exponential,
                initial_delay_ms: 1,
                max_delay_ms: 1,
                timeout_ms: 5000,
            }),
        };

        let result = delivery.send(&event, &config).await;
        assert!(result.is_ok());
        // First attempt fails, second succeeds
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    // ─── Edge case tests ─────────────────────────────────────────────────────

    #[test]
    fn sign_empty_body() {
        let sig = WebhookDelivery::sign("", "secret");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig[7..].len(), 64);
    }

    #[test]
    fn sign_empty_secret() {
        let sig = WebhookDelivery::sign("body", "");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig[7..].len(), 64);
    }

    #[test]
    fn backoff_exponential_large_attempt_clamps_to_max() {
        let retry = RetryConfig {
            retries: 100,
            backoff: BackoffStrategy::Exponential,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            timeout_ms: 5000,
        };
        // 2^30 * 1000 would overflow u64 at very high attempts, but .min(max_delay_ms)
        // should clamp. With attempt=30, 2^29 * 1000 = huge, clamped to 30000.
        let result = WebhookDelivery::backoff_ms(&retry, 30);
        assert_eq!(result, 30000);
    }

    #[test]
    fn backoff_linear_large_attempt() {
        let retry = RetryConfig {
            retries: 100,
            backoff: BackoffStrategy::Linear,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            timeout_ms: 5000,
        };
        assert_eq!(WebhookDelivery::backoff_ms(&retry, 50), 50000);
    }

    #[tokio::test]
    async fn send_retries_zero_means_single_attempt() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = call_count.clone();

        let mock_app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move || {
                let c = cc.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let delivery = WebhookDelivery::new();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: None,
            wrap: None,
            retry: Some(RetryConfig {
                retries: 0,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 0,
                max_delay_ms: 0,
                timeout_ms: 5000,
            }),
        };

        let result = delivery.send(&make_test_event(), &config).await;
        assert!(result.is_err());
        let WebhookError::DeliveryFailed { attempts, .. } = result.unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn send_with_secret_includes_signature_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let captured_sig = Arc::new(tokio::sync::Mutex::new(String::new()));
        let cs = captured_sig.clone();

        let mock_app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move |headers: axum::http::HeaderMap, _body: axum::body::Bytes| {
                let sig = cs.clone();
                async move {
                    if let Some(val) = headers.get("x-taskcast-signature") {
                        *sig.lock().await = val.to_str().unwrap().to_string();
                    }
                    axum::http::StatusCode::OK
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        let delivery = WebhookDelivery::new();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: Some("test-secret".to_string()),
            wrap: None,
            retry: Some(RetryConfig {
                retries: 0,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 0,
                max_delay_ms: 0,
                timeout_ms: 5000,
            }),
        };

        delivery.send(&make_test_event(), &config).await.unwrap();
        let sig = captured_sig.lock().await;
        assert!(sig.starts_with("sha256="), "expected sha256 signature, got: {sig}");
    }

    #[tokio::test]
    async fn send_connection_refused_retries_and_fails() {
        // Bind a port then drop the listener to guarantee connection refused
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let delivery = WebhookDelivery::new();
        let config = WebhookConfig {
            url: format!("http://{addr}/hook"),
            filter: None,
            secret: None,
            wrap: None,
            retry: Some(RetryConfig {
                retries: 1,
                backoff: BackoffStrategy::Fixed,
                initial_delay_ms: 1,
                max_delay_ms: 1,
                timeout_ms: 1000,
            }),
        };

        let result = delivery.send(&make_test_event(), &config).await;
        assert!(result.is_err());
        let WebhookError::DeliveryFailed { attempts, .. } = result.unwrap_err();
        assert_eq!(attempts, 2); // 1 initial + 1 retry
    }
}
