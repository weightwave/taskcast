use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

pub struct TcpFaultProxy {
    address: SocketAddr,
    upstream: SocketAddr,
    accepted: Arc<AtomicUsize>,
    refusing: Arc<AtomicBool>,
    generation: Arc<Notify>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
    listener_task: JoinHandle<()>,
}

impl TcpFaultProxy {
    pub async fn start(upstream: SocketAddr) -> io::Result<Self> {
        let first_port = 20_000 + (std::process::id() % 20_000) as u16;
        let mut listener = None;
        for port in first_port..=40_000 {
            match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
                Ok(bound) => {
                    listener = Some(bound);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
                Err(error) => return Err(error),
            }
        }
        let listener = listener
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no proxy port available"))?;
        let address = listener.local_addr()?;
        let accepted = Arc::new(AtomicUsize::new(0));
        let refusing = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(Notify::new());
        let connections = Arc::new(Mutex::new(Vec::new()));

        let accepted_task = Arc::clone(&accepted);
        let refusing_task = Arc::clone(&refusing);
        let generation_task = Arc::clone(&generation);
        let connections_task = Arc::clone(&connections);
        let listener_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    accepted_socket = listener.accept() => {
                        let Ok((mut downstream, _)) = accepted_socket else { break };
                        accepted_task.fetch_add(1, Ordering::SeqCst);
                        if refusing_task.load(Ordering::SeqCst) {
                            drop(downstream);
                            continue;
                        }
                        let generation_connection = Arc::clone(&generation_task);
                        let handle = tokio::spawn(async move {
                            let Ok(mut upstream_socket) = TcpStream::connect(upstream).await else {
                                return;
                            };
                            tokio::select! {
                                _ = copy_bidirectional(&mut downstream, &mut upstream_socket) => {}
                                _ = generation_connection.notified() => {}
                            }
                        });
                        connections_task.lock().await.push(handle);
                    }
                }
            }
        });

        Ok(Self {
            address,
            upstream,
            accepted,
            refusing,
            generation,
            connections,
            listener_task,
        })
    }

    pub fn redis_url(&self) -> String {
        format!("redis://{}", self.address)
    }

    pub fn accepted_connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    pub async fn open(&self) {
        self.refusing.store(false, Ordering::SeqCst);
    }

    pub fn pause_new_connections(&self) {
        self.refusing.store(true, Ordering::SeqCst);
    }

    pub async fn refuse(&self) {
        self.refusing.store(true, Ordering::SeqCst);
        self.close_sockets().await;
    }

    pub async fn close_latest_connection(&self) {
        let mut connections = self.connections.lock().await;
        while let Some(connection) = connections.pop() {
            if !connection.is_finished() {
                connection.abort();
                break;
            }
        }
    }

    pub async fn close_sockets(&self) {
        self.generation.notify_waiters();
        let mut connections = self.connections.lock().await;
        for connection in connections.drain(..) {
            connection.abort();
        }
    }

    pub async fn stop(&self) {
        self.refuse().await;
        self.listener_task.abort();
    }

    pub fn upstream(&self) -> SocketAddr {
        self.upstream
    }
}

impl Drop for TcpFaultProxy {
    fn drop(&mut self) {
        self.listener_task.abort();
    }
}
