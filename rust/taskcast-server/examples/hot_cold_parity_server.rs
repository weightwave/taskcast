use std::sync::Arc;

use taskcast_core::{
    BroadcastProvider, LongTermStore, MemoryBroadcastProvider, MemoryLongTermStore,
    MemoryShortTermStore, ShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

#[tokio::main]
async fn main() {
    let port = std::env::var("TASKCAST_PARITY_PORT")
        .expect("TASKCAST_PARITY_PORT is required")
        .parse::<u16>()
        .expect("TASKCAST_PARITY_PORT must be a port");
    let short_term_store: Arc<dyn ShortTermStore> = Arc::new(MemoryShortTermStore::new());
    let long_term_store: Arc<dyn LongTermStore> = Arc::new(MemoryLongTermStore::new());
    let broadcast: Arc<dyn BroadcastProvider> = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store,
        long_term_store: Some(long_term_store),
        broadcast,
        hooks: None,
    }));
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("failed to bind parity server");
    axum::serve(listener, app)
        .await
        .expect("parity server failed");
}
