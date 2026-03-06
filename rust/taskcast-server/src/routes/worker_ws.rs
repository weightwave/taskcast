use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::Extension;
use serde::{Deserialize, Serialize};
use taskcast_core::worker_manager::{
    ClaimResult, DeclineOptions, WorkerManager, WorkerRegistration, WorkerUpdate,
    WorkerUpdateStatus,
};
use taskcast_core::{ConnectionMode, PermissionScope, Task, WorkerMatchRule};
use tokio::sync::mpsc;

use crate::auth::{check_scope, AuthContext};
use crate::error::AppError;

// ─── WS Registry ────────────────────────────────────────────────────────────

/// Message that can be sent TO a connected WebSocket worker.
#[derive(Debug, Clone)]
pub enum WorkerCommand {
    Offer { task: TaskSummary, task_id: String },
    Available { task: TaskSummary, task_id: String },
}

/// Registry tracking all active WebSocket worker connections.
#[derive(Clone, Default)]
pub struct WsRegistry {
    senders: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WorkerCommand>>>>,
}

impl WsRegistry {
    pub fn new() -> Self {
        Self {
            senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(&self, worker_id: &str, sender: mpsc::UnboundedSender<WorkerCommand>) {
        self.senders
            .write()
            .unwrap()
            .insert(worker_id.to_string(), sender);
    }

    pub fn unregister(&self, worker_id: &str) {
        self.senders.write().unwrap().remove(worker_id);
    }

    pub fn send(&self, worker_id: &str, cmd: WorkerCommand) -> bool {
        if let Some(sender) = self.senders.read().unwrap().get(worker_id) {
            sender.send(cmd).is_ok()
        } else {
            false
        }
    }

    pub fn worker_ids(&self) -> Vec<String> {
        self.senders.read().unwrap().keys().cloned().collect()
    }
}

// ─── Client → Server Messages ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientMessage {
    Register {
        #[serde(rename = "matchRule")]
        match_rule: WorkerMatchRule,
        capacity: u32,
        #[serde(default, rename = "workerId")]
        worker_id: Option<String>,
        #[serde(default)]
        weight: Option<u32>,
    },
    Update {
        #[serde(default)]
        weight: Option<u32>,
        #[serde(default)]
        capacity: Option<u32>,
        #[serde(default, rename = "matchRule")]
        match_rule: Option<WorkerMatchRule>,
    },
    Accept {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Decline {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(default)]
        blacklist: Option<bool>,
    },
    Claim {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Drain,
    Pong,
}

// ─── Server → Client Messages ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMessage {
    Registered {
        #[serde(rename = "workerId")]
        worker_id: String,
    },
    Offer {
        #[serde(rename = "taskId")]
        task_id: String,
        task: TaskSummary,
    },
    Available {
        #[serde(rename = "taskId")]
        task_id: String,
        task: TaskSummary,
    },
    Assigned {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Claimed {
        #[serde(rename = "taskId")]
        task_id: String,
        success: bool,
    },
    Declined {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Ping,
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
}

// ─── WebSocket Handler ──────────────────────────────────────────────────────

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State((manager, registry)): State<(Arc<WorkerManager>, WsRegistry)>,
    Extension(auth): Extension<AuthContext>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, PermissionScope::WorkerConnect, None) {
        return Err(AppError::Forbidden);
    }

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, manager, registry, auth)))
}

// ─── Socket Loop ────────────────────────────────────────────────────────────

async fn handle_socket(
    mut socket: WebSocket,
    manager: Arc<WorkerManager>,
    registry: WsRegistry,
    auth: AuthContext,
) {
    let mut worker_id: Option<String> = None;
    let interval_ms = manager.heartbeat_interval_ms();
    let mut ping_interval = tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));
    // Skip the first immediate tick
    ping_interval.tick().await;
    let mut registered = false;

    // Channel for receiving commands from the WsRegistry
    let (cmd_sender, mut cmd_receiver) = mpsc::unbounded_channel::<WorkerCommand>();

    loop {
        let msg = if registered {
            tokio::select! {
                msg = socket.recv() => msg,
                _ = ping_interval.tick() => {
                    let _ = send_message(&mut socket, &ServerMessage::Ping).await;
                    continue;
                }
                cmd = cmd_receiver.recv() => {
                    match cmd {
                        Some(WorkerCommand::Offer { task, task_id }) => {
                            let _ = send_message(&mut socket, &ServerMessage::Offer { task_id, task }).await;
                        }
                        Some(WorkerCommand::Available { task, task_id }) => {
                            let _ = send_message(&mut socket, &ServerMessage::Available { task_id, task }).await;
                        }
                        None => break, // Channel closed
                    }
                    continue;
                }
            }
        } else {
            socket.recv().await
        };

        let msg = match msg {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(_)) => continue,
            Some(Err(_)) => break,
        };

        let client_msg: ClientMessage = match serde_json::from_str(&msg) {
            Ok(m) => m,
            Err(e) => {
                let _ = send_message(
                    &mut socket,
                    &ServerMessage::Error {
                        message: format!("Invalid message: {}", e),
                        code: Some("PARSE_ERROR".to_string()),
                    },
                )
                .await;
                continue;
            }
        };

        match client_msg {
            ClientMessage::Register {
                match_rule,
                capacity,
                worker_id: requested_id,
                weight,
            } => {
                // Enforce auth.worker_id matches requested workerId
                if let Some(ref token_worker_id) = auth.worker_id {
                    if let Some(ref req_id) = requested_id {
                        if token_worker_id != req_id {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Error {
                                    message: "Forbidden: worker ID mismatch".to_string(),
                                    code: Some("FORBIDDEN".to_string()),
                                },
                            )
                            .await;
                            continue;
                        }
                    }
                }

                let registration = WorkerRegistration {
                    worker_id: requested_id,
                    match_rule,
                    capacity,
                    weight,
                    connection_mode: ConnectionMode::Websocket,
                    metadata: None,
                };

                match manager.register_worker(registration).await {
                    Ok(worker) => {
                        worker_id = Some(worker.id.clone());
                        registered = true;
                        registry.register(&worker.id, cmd_sender.clone());
                        let _ = send_message(
                            &mut socket,
                            &ServerMessage::Registered {
                                worker_id: worker.id,
                            },
                        )
                        .await;
                    }
                    Err(e) => {
                        let _ = send_message(
                            &mut socket,
                            &ServerMessage::Error {
                                message: e.to_string(),
                                code: Some("REGISTER_ERROR".to_string()),
                            },
                        )
                        .await;
                    }
                }
            }

            ClientMessage::Update {
                weight,
                capacity,
                match_rule,
            } => {
                if let Some(ref wid) = worker_id {
                    let update = WorkerUpdate {
                        weight,
                        capacity,
                        match_rule,
                        status: None,
                    };
                    if let Err(e) = manager.update_worker(wid, update).await {
                        let _ = send_message(
                            &mut socket,
                            &ServerMessage::Error {
                                message: e.to_string(),
                                code: Some("UPDATE_ERROR".to_string()),
                            },
                        )
                        .await;
                    }
                } else {
                    let _ = send_message(
                        &mut socket,
                        &ServerMessage::Error {
                            message: "Not registered".to_string(),
                            code: Some("NOT_REGISTERED".to_string()),
                        },
                    )
                    .await;
                }
            }

            ClientMessage::Accept { task_id } => {
                if let Some(ref wid) = worker_id {
                    match manager.claim_task(&task_id, wid).await {
                        Ok(ClaimResult::Claimed) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Assigned {
                                    task_id: task_id.clone(),
                                },
                            )
                            .await;
                        }
                        Ok(ClaimResult::Failed { reason }) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Error {
                                    message: reason,
                                    code: Some("CLAIM_FAILED".to_string()),
                                },
                            )
                            .await;
                        }
                        Err(e) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Error {
                                    message: e.to_string(),
                                    code: Some("CLAIM_ERROR".to_string()),
                                },
                            )
                            .await;
                        }
                    }
                } else {
                    let _ = send_message(
                        &mut socket,
                        &ServerMessage::Error {
                            message: "Not registered".to_string(),
                            code: Some("NOT_REGISTERED".to_string()),
                        },
                    )
                    .await;
                }
            }

            ClientMessage::Claim { task_id } => {
                if let Some(ref wid) = worker_id {
                    match manager.claim_task(&task_id, wid).await {
                        Ok(ClaimResult::Claimed) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Claimed {
                                    task_id: task_id.clone(),
                                    success: true,
                                },
                            )
                            .await;
                        }
                        Ok(ClaimResult::Failed { .. }) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Claimed {
                                    task_id: task_id.clone(),
                                    success: false,
                                },
                            )
                            .await;
                        }
                        Err(e) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Error {
                                    message: e.to_string(),
                                    code: Some("CLAIM_ERROR".to_string()),
                                },
                            )
                            .await;
                        }
                    }
                } else {
                    let _ = send_message(
                        &mut socket,
                        &ServerMessage::Error {
                            message: "Not registered".to_string(),
                            code: Some("NOT_REGISTERED".to_string()),
                        },
                    )
                    .await;
                }
            }

            ClientMessage::Decline { task_id, blacklist } => {
                if let Some(ref wid) = worker_id {
                    let opts = blacklist.map(|b| DeclineOptions { blacklist: b });
                    match manager.decline_task(&task_id, wid, opts).await {
                        Ok(()) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Declined {
                                    task_id: task_id.clone(),
                                },
                            )
                            .await;
                        }
                        Err(e) => {
                            let _ = send_message(
                                &mut socket,
                                &ServerMessage::Error {
                                    message: e.to_string(),
                                    code: Some("DECLINE_ERROR".to_string()),
                                },
                            )
                            .await;
                        }
                    }
                } else {
                    let _ = send_message(
                        &mut socket,
                        &ServerMessage::Error {
                            message: "Not registered".to_string(),
                            code: Some("NOT_REGISTERED".to_string()),
                        },
                    )
                    .await;
                }
            }

            ClientMessage::Drain => {
                if let Some(ref wid) = worker_id {
                    let update = WorkerUpdate {
                        status: Some(WorkerUpdateStatus::Draining),
                        ..Default::default()
                    };
                    if let Err(e) = manager.update_worker(wid, update).await {
                        let _ = send_message(
                            &mut socket,
                            &ServerMessage::Error {
                                message: e.to_string(),
                                code: Some("DRAIN_ERROR".to_string()),
                            },
                        )
                        .await;
                    }
                } else {
                    let _ = send_message(
                        &mut socket,
                        &ServerMessage::Error {
                            message: "Not registered".to_string(),
                            code: Some("NOT_REGISTERED".to_string()),
                        },
                    )
                    .await;
                }
            }

            ClientMessage::Pong => {
                if let Some(ref wid) = worker_id {
                    let _ = manager.heartbeat(wid).await;
                }
            }
        }
    }

    // On disconnect: unregister worker from registry and manager
    if let Some(ref wid) = worker_id {
        registry.unregister(wid);
        let _ = manager.unregister_worker(wid).await;
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

async fn send_message(
    socket: &mut WebSocket,
    msg: &ServerMessage,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(text.into())).await
}

/// Convert a Task to a TaskSummary for WebSocket messages.
pub fn task_to_summary(task: &Task) -> TaskSummary {
    TaskSummary {
        id: task.id.clone(),
        r#type: task.r#type.clone(),
        tags: task.tags.clone(),
        cost: task.cost,
        params: task.params.clone(),
    }
}
