use std::sync::Arc;

use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use axum::Router;
use taskcast_core::config::TaskcastConfig;
use taskcast_core::heartbeat_monitor::{HeartbeatMonitor, HeartbeatMonitorOptions};
use taskcast_core::scheduler::{TaskScheduler, TaskSchedulerOptions};
use taskcast_core::state_machine::is_terminal;
use taskcast_core::worker_manager::{DispatchResult, WorkerManager};
use taskcast_core::{
    AssignMode, ConnectionMode, DisconnectPolicy, ShortTermStore, Task, TaskEngine, TaskStatus,
    WorkerStatus,
};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use crate::auth::{auth_middleware, AuthMode};
use crate::openapi::ApiDoc;
use crate::routes::worker_ws::{task_to_summary, WorkerCommand, WsRegistry};
use crate::routes::{admin, sse, tasks};

/// Shared application state available to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<TaskEngine>,
    pub auth_mode: Arc<AuthMode>,
}

/// Create the Axum router with all taskcast routes mounted.
///
/// Returns the router and an optional `WsRegistry` (present when a `WorkerManager`
/// is provided) that can be used to send commands to connected WebSocket workers.
pub fn create_app(
    engine: Arc<TaskEngine>,
    auth_mode: AuthMode,
    worker_manager: Option<Arc<WorkerManager>>,
    config: Option<TaskcastConfig>,
) -> (Router, Option<WsRegistry>) {
    let auth_mode = Arc::new(auth_mode);

    let task_routes = Router::new()
        .route("/", get(tasks::list_tasks).post(tasks::create_task))
        .route("/{task_id}", get(tasks::get_task))
        .route("/{task_id}/status", patch(tasks::transition_task))
        .route("/{task_id}/resolve", post(tasks::resolve_task))
        .route("/{task_id}/request", get(tasks::get_blocked_request))
        .route(
            "/{task_id}/events",
            post(tasks::publish_events).get(sse::sse_events),
        )
        .route("/{task_id}/events/history", get(tasks::get_event_history))
        .with_state(Arc::clone(&engine));

    let mut app = Router::new()
        .route("/health", get(health))
        .nest("/tasks", task_routes);

    // Conditionally mount worker routes if a WorkerManager is provided
    let mut ws_registry_out: Option<WsRegistry> = None;

    if let Some(manager) = worker_manager {
        let ws_registry = WsRegistry::new();

        // Auto-release worker capacity on terminal transitions
        {
            let wm_clone = Arc::clone(&manager);
            engine.add_transition_listener(Box::new(move |task, _from, to| {
                if is_terminal(to) {
                    let wm = Arc::clone(&wm_clone);
                    let task_id = task.id.clone();
                    tokio::spawn(async move {
                        auto_release_worker(&wm, &task_id).await;
                    });
                }
            }));
        }

        // Wire ws-offer/ws-race dispatch via transition listener
        {
            let wm_clone = Arc::clone(&manager);
            let registry_clone = ws_registry.clone();
            engine.add_transition_listener(Box::new(move |task, _from, to| {
                if *to != TaskStatus::Pending {
                    return;
                }
                let assign_mode = match &task.assign_mode {
                    Some(m) => m.clone(),
                    None => return,
                };
                match assign_mode {
                    AssignMode::WsOffer => {
                        let wm = Arc::clone(&wm_clone);
                        let registry = registry_clone.clone();
                        let task_clone = task.clone();
                        tokio::spawn(async move {
                            dispatch_ws_offer(&wm, &registry, &task_clone).await;
                        });
                    }
                    AssignMode::WsRace => {
                        let wm = Arc::clone(&wm_clone);
                        let registry = registry_clone.clone();
                        let task_clone = task.clone();
                        tokio::spawn(async move {
                            dispatch_ws_race(&wm, &registry, &task_clone).await;
                        });
                    }
                    _ => {}
                }
            }));
        }

        let worker_routes = crate::routes::workers::workers_router()
            .with_state(Arc::clone(&manager));

        // Mount WS route at top level for /workers/ws (with registry)
        app = app
            .route(
                "/workers/ws",
                get(crate::routes::worker_ws::ws_handler)
                    .with_state((Arc::clone(&manager), ws_registry.clone())),
            )
            .nest("/workers", worker_routes);

        ws_registry_out = Some(ws_registry);
    }

    // OpenAPI spec and Scalar UI
    let openapi_spec = ApiDoc::openapi();
    app = app
        .route(
            "/openapi.json",
            get({
                let spec = openapi_spec.clone();
                move || async move { axum::Json(spec) }
            }),
        )
        .merge(Scalar::with_url("/docs", openapi_spec));

    // Auth middleware must be applied AFTER routes are mounted but BEFORE
    // admin routes are merged, so admin routes bypass auth.
    let app_with_auth = app.layer(middleware::from_fn_with_state(
        Arc::clone(&auth_mode),
        auth_middleware,
    ));

    // Admin route is merged AFTER the auth layer so it bypasses JWT/custom auth.
    // It authenticates via admin token independently.
    let final_app = if let Some(cfg) = config {
        let admin_state = Arc::new(admin::AdminState {
            config: Arc::new(cfg),
            auth_mode: Arc::clone(&auth_mode),
        });
        let admin_routes = Router::new()
            .route("/admin/token", post(admin::admin_token))
            .with_state(admin_state);
        app_with_auth.merge(admin_routes)
    } else {
        app_with_auth
    };

    (final_app, ws_registry_out)
}

async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({ "ok": true }))
}

// ─── Extracted dispatch helpers (testable without closures) ─────────────────

/// Release worker capacity when a task reaches a terminal status.
///
/// Called from the auto-release transition listener. Ignoring errors is
/// intentional — a missing assignment is not a problem.
pub async fn auto_release_worker(wm: &WorkerManager, task_id: &str) {
    let _ = wm.release_task(task_id).await;
}

/// Dispatch a ws-offer task to the best matching worker.
///
/// Uses `WorkerManager::dispatch_task` to find and assign a worker, then
/// sends an `Offer` command via the `WsRegistry`.
pub async fn dispatch_ws_offer(
    wm: &WorkerManager,
    registry: &WsRegistry,
    task: &Task,
) {
    if let Ok(DispatchResult::Dispatched { worker_id }) = wm.dispatch_task(&task.id).await {
        let summary = task_to_summary(task);
        registry.send(
            &worker_id,
            WorkerCommand::Offer {
                task_id: task.id.clone(),
                task: summary,
            },
        );
    }
}

/// Broadcast a ws-race task to all eligible WebSocket workers.
///
/// Sends an `Available` command to every connected WebSocket worker that is
/// in `Idle` or `Busy` status (skips `Draining`/`Offline`).
pub async fn dispatch_ws_race(
    wm: &WorkerManager,
    registry: &WsRegistry,
    task: &Task,
) {
    if let Ok(workers) = wm.list_workers(None).await {
        let summary = task_to_summary(task);
        for worker in workers {
            if worker.connection_mode != ConnectionMode::Websocket {
                continue;
            }
            if worker.status != WorkerStatus::Idle && worker.status != WorkerStatus::Busy {
                continue;
            }
            registry.send(
                &worker.id,
                WorkerCommand::Available {
                    task_id: task.id.clone(),
                    task: summary.clone(),
                },
            );
        }
    }
}

// ─── Background Services ────────────────────────────────────────────────────

/// Holds optional background services that run alongside the HTTP server.
pub struct BackgroundServices {
    pub scheduler: Option<TaskScheduler>,
    pub heartbeat_monitor: Option<HeartbeatMonitor>,
}

impl BackgroundServices {
    /// Stop all running background services.
    pub fn stop(&mut self) {
        if let Some(ref mut s) = self.scheduler {
            s.stop();
        }
        if let Some(ref mut h) = self.heartbeat_monitor {
            h.stop();
        }
    }
}

/// Create and start background services (scheduler + heartbeat monitor).
///
/// The caller owns the returned `BackgroundServices` and should call `.stop()`
/// on shutdown.
pub fn start_background_services(
    engine: Arc<TaskEngine>,
    store: Arc<dyn ShortTermStore>,
    worker_manager: Option<Arc<WorkerManager>>,
) -> BackgroundServices {
    let mut scheduler = TaskScheduler::new(TaskSchedulerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store),
        check_interval_ms: 60_000,
        paused_cold_after_ms: None,
        blocked_cold_after_ms: None,
    });
    scheduler.start();

    let heartbeat_monitor = worker_manager.map(|wm| {
        let mut monitor = HeartbeatMonitor::new(HeartbeatMonitorOptions {
            worker_manager: wm,
            engine,
            short_term_store: store,
            check_interval_ms: 30_000,
            heartbeat_timeout_ms: 90_000,
            default_disconnect_policy: DisconnectPolicy::Reassign,
            disconnect_grace_ms: 30_000,
        });
        monitor.start();
        monitor
    });

    BackgroundServices {
        scheduler: Some(scheduler),
        heartbeat_monitor,
    }
}
