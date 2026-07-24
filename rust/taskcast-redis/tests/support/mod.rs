use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio::task::JoinHandle;

const MODE_OPEN: u8 = 0;
const MODE_REFUSE: u8 = 1;
const MODE_BLACKHOLE: u8 = 2;
const MAX_MATCHER_BUFFER_BYTES: usize = 64 * 1024;

type RequestMatcher = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

struct ResponseDropRule {
    matcher: RequestMatcher,
    max_connection_id: usize,
}

pub struct TcpFaultProxy {
    address: SocketAddr,
    upstream: SocketAddr,
    accepted: Arc<AtomicUsize>,
    blackholed_downstream_activity: Arc<AtomicUsize>,
    #[allow(dead_code)] // Used by reconnect and PostgreSQL integration binaries.
    matched: Arc<AtomicUsize>,
    mode: Arc<AtomicU8>,
    generation: Arc<Notify>,
    #[allow(dead_code)] // Used by reconnect and PostgreSQL integration binaries.
    response_drop_matcher: Arc<AsyncMutex<Option<ResponseDropRule>>>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
    listener_task: JoinHandle<()>,
}

impl TcpFaultProxy {
    pub async fn start(upstream: SocketAddr) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let accepted = Arc::new(AtomicUsize::new(0));
        let blackholed_downstream_activity = Arc::new(AtomicUsize::new(0));
        let matched = Arc::new(AtomicUsize::new(0));
        let mode = Arc::new(AtomicU8::new(MODE_OPEN));
        let generation = Arc::new(Notify::new());
        let response_drop_matcher = Arc::new(AsyncMutex::new(None));
        let connections = Arc::new(Mutex::new(Vec::new()));

        let accepted_task = Arc::clone(&accepted);
        let blackholed_activity_task = Arc::clone(&blackholed_downstream_activity);
        let matched_task = Arc::clone(&matched);
        let mode_task = Arc::clone(&mode);
        let generation_task = Arc::clone(&generation);
        let matcher_task = Arc::clone(&response_drop_matcher);
        let connections_task = Arc::clone(&connections);
        let listener_task = tokio::spawn(async move {
            loop {
                let Ok((downstream, _)) = listener.accept().await else {
                    break;
                };
                let connection_id = accepted_task.fetch_add(1, Ordering::SeqCst) + 1;
                match mode_task.load(Ordering::SeqCst) {
                    MODE_REFUSE => drop(downstream),
                    _ => {
                        let generation_connection = Arc::clone(&generation_task);
                        let mode_connection = Arc::clone(&mode_task);
                        let blackholed_activity_connection = Arc::clone(&blackholed_activity_task);
                        let matcher_connection = Arc::clone(&matcher_task);
                        let matched_connection = Arc::clone(&matched_task);
                        let handle = tokio::spawn(forward_connection(
                            downstream,
                            upstream,
                            generation_connection,
                            mode_connection,
                            blackholed_activity_connection,
                            matcher_connection,
                            matched_connection,
                            connection_id,
                        ));
                        connections_task.lock().unwrap().push(handle);
                    }
                }
            }
        });

        Ok(Self {
            address,
            upstream,
            accepted,
            blackholed_downstream_activity,
            matched,
            mode,
            generation,
            response_drop_matcher,
            connections,
            listener_task,
        })
    }

    #[allow(dead_code)] // Used by Redis integration binaries, not the PostgreSQL binary.
    pub fn redis_url(&self) -> String {
        format!("redis://{}", self.address)
    }

    #[allow(dead_code)] // Used when this helper module is shared by PostgreSQL tests.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    #[allow(dead_code)] // Used by Redis integration binaries, not the PostgreSQL binary.
    pub fn accepted_connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    #[allow(dead_code)] // Used by the Rust long-outage regression.
    pub fn blackholed_downstream_activity(&self) -> usize {
        self.blackholed_downstream_activity.load(Ordering::SeqCst)
    }

    #[allow(dead_code)] // Used by reconnect and PostgreSQL integration binaries.
    pub fn matched_commands(&self) -> usize {
        self.matched.load(Ordering::SeqCst)
    }

    pub async fn open(&self) {
        self.mode.store(MODE_OPEN, Ordering::SeqCst);
    }

    #[allow(dead_code)] // Used by the long-outage reconnect regression.
    pub async fn blackhole(&self) {
        self.mode.store(MODE_BLACKHOLE, Ordering::SeqCst);
    }

    #[allow(dead_code)] // Used by the Redis concurrent integration binary.
    pub fn pause_new_connections(&self) {
        self.mode.store(MODE_REFUSE, Ordering::SeqCst);
    }

    pub async fn refuse(&self) {
        self.mode.store(MODE_REFUSE, Ordering::SeqCst);
        self.close_sockets().await;
    }

    #[allow(dead_code)] // Used by reconnect and PostgreSQL no-replay regressions.
    pub async fn drop_next_response(
        &self,
        matcher: impl Fn(&[u8]) -> bool + Send + Sync + 'static,
    ) {
        let mut armed = self.response_drop_matcher.lock().await;
        assert!(armed.is_none(), "a response-drop matcher is already armed");
        let max_connection_id = self.accepted.load(Ordering::SeqCst);
        assert!(
            max_connection_id > 0,
            "response-drop matcher requires an established connection"
        );
        *armed = Some(ResponseDropRule {
            matcher: Arc::new(matcher),
            max_connection_id,
        });
    }

    #[allow(dead_code)] // Used by the Redis concurrent integration binary.
    pub async fn close_latest_connection(&self) {
        let mut connections = self.connections.lock().unwrap();
        while let Some(connection) = connections.pop() {
            if !connection.is_finished() {
                connection.abort();
                break;
            }
        }
    }

    #[allow(dead_code)] // Used internally and by Redis integration binaries.
    pub async fn close_sockets(&self) {
        self.generation.notify_waiters();
        let mut connections = self.connections.lock().unwrap();
        for connection in connections.drain(..) {
            connection.abort();
        }
    }

    pub async fn stop(&self) {
        self.refuse().await;
        self.listener_task.abort();
    }

    #[allow(dead_code)] // Used by the Redis concurrent integration binary.
    pub fn upstream(&self) -> SocketAddr {
        self.upstream
    }
}

impl Drop for TcpFaultProxy {
    fn drop(&mut self) {
        self.listener_task.abort();
        for connection in self.connections.lock().unwrap().drain(..) {
            connection.abort();
        }
    }
}

async fn forward_connection(
    mut downstream: TcpStream,
    upstream: SocketAddr,
    generation: Arc<Notify>,
    mode: Arc<AtomicU8>,
    blackholed_downstream_activity: Arc<AtomicUsize>,
    response_drop_matcher: Arc<AsyncMutex<Option<ResponseDropRule>>>,
    matched: Arc<AtomicUsize>,
    connection_id: usize,
) {
    let Ok(mut upstream) = TcpStream::connect(upstream).await else {
        return;
    };
    let mut downstream_buffer = [0_u8; 8 * 1024];
    let mut upstream_buffer = [0_u8; 8 * 1024];
    let mut matcher_buffer = Vec::new();
    let mut drop_response = false;

    loop {
        tokio::select! {
            _ = generation.notified() => return,
            result = downstream.read(&mut downstream_buffer) => {
                let count = match result {
                    Ok(0) | Err(_) => return,
                    Ok(count) => count,
                };
                match mode.load(Ordering::SeqCst) {
                    MODE_BLACKHOLE => {
                        blackholed_downstream_activity.fetch_add(1, Ordering::SeqCst);
                        continue;
                    }
                    MODE_OPEN => {}
                    _ => return,
                }
                {
                    let mut armed = response_drop_matcher.lock().await;
                    if let Some(rule) = armed.as_ref().filter(|rule| {
                        connection_id <= rule.max_connection_id
                    }) {
                        matcher_buffer.extend_from_slice(&downstream_buffer[..count]);
                        if matcher_buffer.len() > MAX_MATCHER_BUFFER_BYTES {
                            let excess = matcher_buffer.len() - MAX_MATCHER_BUFFER_BYTES;
                            matcher_buffer.drain(..excess);
                        }
                        if (rule.matcher)(&matcher_buffer) {
                            matcher_buffer.fill(0);
                            matcher_buffer.clear();
                            matcher_buffer.shrink_to_fit();
                            matched.fetch_add(1, Ordering::SeqCst);
                            drop_response = true;
                            *armed = None;
                        }
                    }
                }
                if upstream.write_all(&downstream_buffer[..count]).await.is_err() {
                    return;
                }
            }
            result = upstream.read(&mut upstream_buffer) => {
                let count = match result {
                    Ok(0) | Err(_) => return,
                    Ok(count) => count,
                };
                match mode.load(Ordering::SeqCst) {
                    MODE_BLACKHOLE => continue,
                    MODE_OPEN => {}
                    _ => return,
                }
                if drop_response {
                    return;
                }
                if downstream.write_all(&upstream_buffer[..count]).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[allow(dead_code)] // Used by the Redis no-replay integration binary.
pub fn redis_command_matches(request: &[u8], expected: &[&[u8]]) -> bool {
    let Some((parts, _)) = parse_resp_command(request, 0) else {
        return false;
    };
    parts.len() == expected.len()
        && parts
            .iter()
            .zip(expected)
            .enumerate()
            .all(|(index, (actual, expected))| {
                if index == 0 {
                    actual.eq_ignore_ascii_case(expected)
                } else {
                    actual == expected
                }
            })
}

#[allow(dead_code)] // Called by redis_command_matches in its integration binary.
fn parse_resp_command(request: &[u8], offset: usize) -> Option<(Vec<&[u8]>, usize)> {
    if request.get(offset) != Some(&b'*') {
        return None;
    }
    let (array_count, mut cursor) = read_resp_number(request, offset + 1)?;
    let array_count = usize::try_from(array_count).ok()?;
    let mut parts = Vec::with_capacity(array_count);

    for _ in 0..array_count {
        if request.get(cursor) != Some(&b'$') {
            return None;
        }
        let (length, content_start) = read_resp_number(request, cursor + 1)?;
        let length = usize::try_from(length).ok()?;
        let content_end = content_start.checked_add(length)?;
        if request.get(content_end..content_end + 2)? != b"\r\n" {
            return None;
        }
        parts.push(request.get(content_start..content_end)?);
        cursor = content_end + 2;
    }
    Some((parts, cursor))
}

#[allow(dead_code)] // Called by redis_command_matches in its integration binary.
fn read_resp_number(request: &[u8], offset: usize) -> Option<(i64, usize)> {
    let line_end = request
        .get(offset..)?
        .windows(2)
        .position(|window| window == b"\r\n")?
        + offset;
    let number = std::str::from_utf8(request.get(offset..line_end)?)
        .ok()?
        .parse()
        .ok()?;
    Some((number, line_end + 2))
}
