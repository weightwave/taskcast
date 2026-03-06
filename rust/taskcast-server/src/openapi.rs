use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::routes::{sse, tasks, workers};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Taskcast API",
        version = "0.3.0",
        description = "Unified long-lifecycle task tracking service for LLM streaming, agents, and async workloads."
    ),
    paths(
        tasks::list_tasks,
        tasks::create_task,
        tasks::get_task,
        tasks::transition_task,
        tasks::publish_events,
        tasks::get_event_history,
        sse::sse_events,
        workers::list_workers,
        workers::pull_task,
        workers::get_worker,
        workers::delete_worker,
        workers::update_worker_status,
        workers::decline_task,
    ),
    components(schemas(
        taskcast_core::Task,
        taskcast_core::TaskStatus,
        taskcast_core::TaskError,
        taskcast_core::TaskEvent,
        taskcast_core::Level,
        taskcast_core::SeriesMode,
        taskcast_core::Worker,
        taskcast_core::WorkerStatus,
        taskcast_core::AssignMode,
        taskcast_core::DisconnectPolicy,
        taskcast_core::SSEEnvelope,
        tasks::CreateTaskBody,
        tasks::TransitionBody,
        tasks::TaskErrorBody,
        tasks::PublishEventBody,
        workers::DeclineBody,
        workers::WorkerStatusUpdateBody,
        workers::WorkerStatusUpdateValue,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "Tasks", description = "Task lifecycle management"),
        (name = "Events", description = "Task event publishing and streaming"),
        (name = "Workers", description = "Worker management and task assignment"),
    )
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "Bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some("JWT Bearer token"))
                    .build(),
            ),
        );
    }
}
