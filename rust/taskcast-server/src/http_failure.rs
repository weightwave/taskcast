use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use regex::Regex;
use serde::Serialize;

const MAX_ERROR_SCALARS: usize = 2048;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        let normalized = value.unwrap_or("info").trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" | "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "invalid TASKCAST_LOG_LEVEL \"{}\"; expected debug, info, warn, or error",
                value.unwrap_or_default()
            )),
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Debug => 10,
            Self::Info => 20,
            Self::Warn => 30,
            Self::Error => 40,
        }
    }

    pub fn allows_error(self) -> bool {
        self.priority() <= Self::Error.priority()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpFailureKind {
    Store,
    Archive,
    Internal,
}

#[derive(Clone, Debug)]
pub(crate) struct HttpFailureDetail {
    pub(crate) error_kind: HttpFailureKind,
    pub(crate) error: String,
}

impl HttpFailureDetail {
    pub(crate) fn new(error_kind: HttpFailureKind, error: impl Into<String>) -> Self {
        Self {
            error_kind,
            error: error.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpFailureLog {
    pub timestamp: String,
    pub level: &'static str,
    pub event: &'static str,
    pub method: String,
    pub path: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<HttpFailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub trait HttpFailureLogger: Send + Sync + 'static {
    fn log(&self, record: &HttpFailureLog);
}

pub struct StderrHttpFailureLogger {
    level: LogLevel,
}

impl StderrHttpFailureLogger {
    pub fn new(level: LogLevel) -> Self {
        Self { level }
    }
}

impl HttpFailureLogger for StderrHttpFailureLogger {
    fn log(&self, record: &HttpFailureLog) {
        if self.level.allows_error() {
            eprintln!(
                "{}",
                serde_json::to_string(record)
                    .expect("HttpFailureLog contains only serializable fields")
            );
        }
    }
}

#[derive(Clone, Default)]
pub struct CollectingHttpFailureLogger {
    records: Arc<Mutex<Vec<HttpFailureLog>>>,
}

impl CollectingHttpFailureLogger {
    pub fn records(&self) -> Vec<HttpFailureLog> {
        self.records.lock().unwrap().clone()
    }
}

impl HttpFailureLogger for CollectingHttpFailureLogger {
    fn log(&self, record: &HttpFailureLog) {
        self.records.lock().unwrap().push(record.clone());
    }
}

pub fn sanitize_error_message(value: &str) -> Option<String> {
    static URL_USERINFO: OnceLock<Regex> = OnceLock::new();
    let regex = URL_USERINFO.get_or_init(|| {
        Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)[^@\s/]+@").expect("URL userinfo regex must compile")
    });
    let redacted = regex.replace_all(value, "${1}***@");
    let truncated: String = redacted.chars().take(MAX_ERROR_SCALARS).collect();
    (!truncated.is_empty()).then_some(truncated)
}

pub async fn http_failure_logger_middleware(
    State(logger): State<Arc<dyn HttpFailureLogger>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;

    if response.status().is_server_error() {
        let detail = response.extensions().get::<HttpFailureDetail>();
        let record = HttpFailureLog {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level: "error",
            event: "http_request_failed",
            method,
            path,
            status: response.status().as_u16(),
            error_kind: detail.map(|value| value.error_kind),
            error: detail.and_then(|value| sanitize_error_message(&value.error)),
        };
        logger.log(&record);
    }

    response
}
