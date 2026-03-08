use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use std::time::Instant;

/// Trait for receiving verbose log lines.
pub trait VerboseLogger: Send + Sync + 'static {
    fn log(&self, line: &str);
}

/// Default logger that prints to stderr (matches `eprintln!` used elsewhere in the CLI).
pub struct StderrLogger;

impl VerboseLogger for StderrLogger {
    fn log(&self, line: &str) {
        eprintln!("{line}");
    }
}

/// A logger that collects lines into a shared Vec for testing.
#[derive(Clone, Default)]
pub struct CollectingLogger {
    pub lines: Arc<std::sync::Mutex<Vec<String>>>,
}

impl VerboseLogger for CollectingLogger {
    fn log(&self, line: &str) {
        self.lines.lock().unwrap().push(line.to_string());
    }
}

/// Axum middleware that logs every HTTP request with timing and context.
///
/// Use with `axum::middleware::from_fn_with_state`:
/// ```ignore
/// let logger: Arc<dyn VerboseLogger> = Arc::new(StderrLogger);
/// app.layer(axum::middleware::from_fn_with_state(logger, verbose_logger_middleware))
/// ```
///
/// Log format:
/// ```text
/// [2026-03-07 14:32:01] POST   /tasks                    -> 201  12ms  (task created)
/// [2026-03-07 14:32:02] PATCH  /tasks/01JXXXXX/status    -> 200   3ms  (-> running)
/// [2026-03-07 14:32:02] POST   /tasks/01JXXXXX/events    -> 201   2ms  (type: llm.delta)
/// [2026-03-07 14:32:03] GET    /tasks/01JXXXXX/events    -> SSE   0ms  (subscriber connected)
/// ```
pub async fn verbose_logger_middleware(
    State(logger): State<Arc<dyn VerboseLogger>>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    const MAX_BODY_SIZE: usize = 64 * 1024;

    // Check Content-Length to decide if we should parse the body
    let content_length = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());

    let body_too_large = content_length.is_some_and(|len| len > MAX_BODY_SIZE);

    let (request_body, req) =
        if !body_too_large && matches!(method.as_str(), "POST" | "PATCH" | "PUT") {
            let (parts, body) = req.into_parts();
            match axum::body::to_bytes(body, MAX_BODY_SIZE).await {
                Ok(bytes) => {
                    let parsed: Option<serde_json::Value> =
                        serde_json::from_slice(&bytes).ok();
                    let req = Request::from_parts(parts, axum::body::Body::from(bytes));
                    (parsed, req)
                }
                Err(_) => {
                    // Body exceeded limit during read (no Content-Length or incorrect).
                    // The body stream is consumed; reconstruct empty.
                    let req = Request::from_parts(parts, axum::body::Body::empty());
                    (None, req)
                }
            }
        } else {
            (None, req)
        };

    let start = Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_millis();

    let status = response.status().as_u16();

    // Determine if SSE
    let is_sse = (path.ends_with("/events") && method == "GET" && path.contains("/tasks/"))
        || (path == "/events" && method == "GET");
    let status_str = if is_sse {
        "SSE".to_string()
    } else {
        status.to_string()
    };

    let context =
        extract_context(&method, &path, status, request_body.as_ref(), body_too_large);

    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S");

    let context_suffix = if context.is_empty() {
        String::new()
    } else {
        format!("  ({context})")
    };

    logger.log(&format!(
        "[{now}] {method:<6} {path:<35} \u{2192} {status_str:>3}  {duration:>3}ms{context_suffix}"
    ));

    response
}

fn extract_context(
    method: &str,
    path: &str,
    status: u16,
    body: Option<&serde_json::Value>,
    body_too_large: bool,
) -> String {
    // If the body was too large to parse, note it in context for methods that normally show body info
    if body_too_large && matches!(method, "POST" | "PATCH" | "PUT") {
        return "body too large to log".to_string();
    }

    // POST /tasks -> task created
    if method == "POST" && path == "/tasks" && status == 201 {
        return "task created".to_string();
    }

    // PATCH /tasks/:id/status -> show target status
    if method == "PATCH" && path.ends_with("/status") && path.contains("/tasks/") {
        if let Some(target) = body.and_then(|b| b.get("status")).and_then(|s| s.as_str()) {
            return format!("\u{2192} {target}");
        }
        return "status transition".to_string();
    }

    // POST /tasks/:id/events -> show event type
    if method == "POST" && path.ends_with("/events") && path.contains("/tasks/") {
        if let Some(event_type) = body.and_then(|b| b.get("type")).and_then(|s| s.as_str()) {
            return format!("type: {event_type}");
        }
        if body.is_some_and(|b| b.is_array()) {
            let count = body.unwrap().as_array().map_or(0, |a| a.len());
            return format!("{count} events");
        }
        return "event published".to_string();
    }

    // GET /tasks/:id/events -> SSE subscriber
    if method == "GET" && path.ends_with("/events") && path.contains("/tasks/") {
        return "subscriber connected".to_string();
    }

    // GET /events -> global SSE
    if method == "GET" && path == "/events" {
        return "global subscriber connected".to_string();
    }

    // POST /tasks/:id/resolve
    if method == "POST" && path.ends_with("/resolve") && path.contains("/tasks/") {
        return "resolve".to_string();
    }

    String::new()
}
