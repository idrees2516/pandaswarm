//! PandaSwarm HTTP server: REST over the cowork engine + a WebSocket
//! telemetry stream. The Next.js frontend (or `curl`) drives everything
//! through these endpoints.

mod api;

use anyhow::Result;
use api::AppState;
use axum::routing::{get, post};
use axum::Router;
use pandaswarm_core::PandaSwarmConfig;
use pandaswarm_orchestrator::Cowork;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pandaswarm=info,tower_http=warn".into()),
        )
        .init();

    let cfg = PandaSwarmConfig::load(None)?;
    std::fs::create_dir_all(&cfg.workspace_root)?;
    std::fs::create_dir_all(&cfg.data_dir)?;
    let cowork = Cowork::new(cfg.clone())?;
    cowork.start().await;

    let state = AppState { cowork: cowork.clone() };
    let app = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/doctor", get(api::doctor))
        .route("/api/sessions", get(api::list_sessions).post(api::create_session))
        .route("/api/sessions/:id", get(api::get_session))
        .route("/api/sessions/:id/chat", post(api::chat))
        .route("/api/sessions/:id/cancel", post(api::cancel_session))
        .route("/api/sessions/:id/events", get(api::session_events))
        .route("/api/sessions/:id/artifacts/:aid", get(api::artifact_content))
        .route("/api/sandbox/run", post(api::sandbox_run))
        .route("/api/sandbox/backends", get(api::sandbox_backends))
        .route("/api/rag/search", get(api::rag_search))
        .route("/api/rag/index", post(api::rag_index))
        .route("/api/rag/status", get(api::rag_status))
        .route("/api/memory/facts", get(api::list_facts).post(api::remember_fact))
        .route("/api/memory/recall", get(api::recall_facts))
        .route("/api/memory/history", get(api::search_history))
        .route("/api/computer-use/status", get(api::computer_use_status))
        .route("/ws", get(api::ws_telemetry))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let bind = cfg.server.bind.clone();
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("🐼 PandaSwarm server listening on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
