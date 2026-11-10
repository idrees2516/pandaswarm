//! API handlers — thin adapters over the `Cowork` engine.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use pandaswarm_core::{now_ms, SandboxProfile};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use pandaswarm_orchestrator::{is_running, Cowork};

#[derive(Clone)]
pub struct AppState {
    pub cowork: Arc<Cowork>,
}

fn ok_json(v: Value) -> axum::response::Response {
    Json(v).into_response()
}

// ---------- health / doctor ----------

pub async fn health(State(app): State<AppState>) -> axum::response::Response {
    ok_json(json!({
        "ok": true,
        "service": "pandaswarm",
        "version": env!("CARGO_PKG_VERSION"),
        "llm": app.cowork.llm.name(),
        "now_ms": now_ms(),
    }))
    .into_response()
}

pub async fn doctor(State(app): State<AppState>) -> axum::response::Response {
    ok_json(app.cowork.doctor().await).into_response()
}

// ---------- sessions ----------

pub async fn list_sessions(State(app): State<AppState>) -> axum::response::Response {
    match app.cowork.memory.list_sessions(100) {
        Ok(sessions) => ok_json(json!({ "sessions": sessions })).into_response(),
        Err(e) => err500(e),
    }
}

pub async fn create_session(
    State(app): State<AppState>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let prompt = body["prompt"].as_str().unwrap_or_default().trim().to_string();
    if prompt.is_empty() {
        return bad_request("prompt is required");
    }
    let title = body["title"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| prompt.chars().take(60).collect());
    let session_id = pandaswarm_core::new_id("sess");
    match app.cowork.run_task(&session_id, &prompt, &title).await {
        Ok(()) => ok_json(json!({ "session_id": session_id, "status": "planning" })).into_response(),
        Err(e) => err500(e),
    }
}

pub async fn get_session(State(app): State<AppState>, Path(id): Path<String>) -> axum::response::Response {
    let state = match app.cowork.session_state(&id).await {
        Ok(s) => s,
        Err(e) => return err500(e),
    };
    let messages = app.cowork.memory.list_messages(&id).unwrap_or_default();
    let artifacts = app.cowork.memory.list_artifacts(&id).unwrap_or_default();
    ok_json(json!({
        "session_id": id,
        "running": state.as_ref().map(is_running).unwrap_or(false),
        "swarm": state,
        "messages": messages,
        "artifacts": artifacts,
    }))
    .into_response()
}

pub async fn chat(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let content = body["content"].as_str().unwrap_or_default().to_string();
    if content.is_empty() {
        return bad_request("content is required");
    }
    match app.cowork.chat(&id, &content).await {
        Ok(reply) => ok_json(json!({ "reply": reply })).into_response(),
        Err(e) => err500(e),
    }
}

pub async fn cancel_session(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let reason = body["reason"].as_str().unwrap_or("user request").to_string();
    match app.cowork.cancel(&id, &reason).await {
        Ok(()) => ok_json(json!({ "cancelled": true })).into_response(),
        Err(e) => err500(e),
    }
}

pub async fn session_events(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let after: u64 = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let state = match app.cowork.session_state(&id).await {
        Ok(s) => s,
        Err(e) => return err500(e),
    };
    let events: Vec<Value> = state
        .map(|s| {
            s.events
                .iter()
                .filter(|e| e.seq > after)
                .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                .collect()
        })
        .unwrap_or_default();
    ok_json(json!({ "events": events, "now_ms": now_ms() })).into_response()
}

pub async fn artifact_content(
    State(app): State<AppState>,
    Path((id, aid)): Path<(String, String)>,
) -> axum::response::Response {
    match app.cowork.memory.artifact_content(&aid) {
        Ok(Some((path, content))) => ok_json(json!({ "session_id": id, "path": path, "content": content })).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "artifact not found").into_response(),
        Err(e) => err500(e),
    }
}

// ---------- sandbox ----------

pub async fn sandbox_run(State(app): State<AppState>, Json(body): Json<Value>) -> axum::response::Response {
    let cmd = body["cmd"].as_str().unwrap_or_default().to_string();
    if cmd.is_empty() {
        return bad_request("cmd is required");
    }
    let files: Vec<(String, String)> = body["files"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some((
                        f["path"].as_str()?.to_string(),
                        f["content"].as_str().unwrap_or_default().to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let timeout = body["timeout_secs"].as_u64().unwrap_or(app.cowork.cfg.sandbox.default_timeout_secs);
    let profile = SandboxProfile {
        fs_ro: vec![],
        fs_rw: String::new(),
        net_enabled: body["net"].as_bool().unwrap_or(false),
        timeout_secs: timeout,
        env_allowlist: vec![],
        backend: app.cowork.sandbox.backend(),
    };
    let started = std::time::Instant::now();
    match app.cowork.sandbox.exec(&profile, &files, &cmd).await {
        Ok(res) => {
            let report = pandaswarm_sandbox::parse_test_report(&res);
            ok_json(json!({
                "result": res,
                "report": report,
                "suggestions": pandaswarm_sandbox::suggest_harness_patches(&report, &[]),
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }))
            .into_response()
        }
        Err(e) => err500(e),
    }
}

pub async fn sandbox_backends(State(app): State<AppState>) -> axum::response::Response {
    let layers = pandaswarm_sandbox::detect_local_layers();
    ok_json(json!({
        "active": app.cowork.sandbox.backend().as_str(),
        "description": app.cowork.sandbox.describe(),
        "available_layers": layers.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
        "instabox": {
            "key_set": !app.cowork.cfg.sandbox.instabox.api_key.is_empty(),
            "template": app.cowork.cfg.sandbox.instabox.template,
            "mode": app.cowork.cfg.sandbox.instabox.mode,
        },
    }))
    .into_response()
}

// ---------- rag ----------

pub async fn rag_search(
    State(app): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let query = q.get("q").cloned().unwrap_or_default();
    let top_k = q.get("top_k").and_then(|v| v.parse().ok()).unwrap_or(8);
    let mut rag = app.cowork.rag.lock().await;
    let hits = rag.search(&query, top_k);
    let frame = pandaswarm_rag::frame_pack(&hits).ok();
    ok_json(json!({ "query": query, "hits": hits, "frame": frame })).into_response()
}

pub async fn rag_index(State(app): State<AppState>) -> axum::response::Response {
    let mut rag = app.cowork.rag.lock().await;
    match rag.reindex() {
        Ok((scanned, reindexed)) => ok_json(json!({
            "files": rag.file_count(),
            "chunks": rag.chunk_count(),
            "scanned": scanned,
            "reindexed": reindexed,
        }))
        .into_response(),
        Err(e) => err500(e),
    }
}

pub async fn rag_status(State(app): State<AppState>) -> axum::response::Response {
    let rag = app.cowork.rag.lock().await;
    ok_json(json!({ "files": rag.file_count(), "chunks": rag.chunk_count() })).into_response()
}

// ---------- memory ----------

pub async fn list_facts(
    State(app): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50);
    match app.cowork.memory.list_facts(limit) {
        Ok(facts) => ok_json(json!({ "facts": facts })).into_response(),
        Err(e) => err500(e),
    }
}

pub async fn remember_fact(State(app): State<AppState>, Json(body): Json<Value>) -> axum::response::Response {
    let content = body["content"].as_str().unwrap_or_default().to_string();
    let kind = body["kind"].as_str().unwrap_or("note").to_string();
    if content.is_empty() {
        return bad_request("content is required");
    }
    match app.cowork.memory.remember_fact(None, &kind, &content) {
        Ok(id) => ok_json(json!({ "id": id })).into_response(),
        Err(e) => err500(e),
    }
}

pub async fn recall_facts(
    State(app): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let query = q.get("q").cloned().unwrap_or_default();
    match app.cowork.memory.recall(&query, 10, 0.0) {
        Ok(facts) => ok_json(json!({ "facts": facts })).into_response(),
        Err(e) => err500(e),
    }
}

pub async fn search_history(
    State(app): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let query = q.get("q").cloned().unwrap_or_default();
    match app.cowork.memory.search_history(None, &query, 20) {
        Ok(rows) => ok_json(json!({ "messages": rows })).into_response(),
        Err(e) => err500(e),
    }
}

// ---------- computer use ----------

pub async fn computer_use_status(State(app): State<AppState>) -> axum::response::Response {
    let endpoint = app.cowork.cfg.search.cdp_endpoint.clone();
    if endpoint.is_empty() {
        return ok_json(json!({
            "configured": false,
            "hint": "set search.cdp_endpoint (e.g. http://127.0.0.1:9222) to drive a lightpanda/chromium browser",
        }))
        .into_response();
    }
    match reqwest::get(format!("{}/json/version", endpoint.trim_end_matches('/'))).await {
        Ok(resp) if resp.status().is_success() => {
            let v: Value = resp.json().await.unwrap_or(json!({}));
            ok_json(json!({ "configured": true, "reachable": true, "browser": v["Browser"] })).into_response()
        }
        Ok(resp) => ok_json(json!({ "configured": true, "reachable": false, "status": resp.status().as_u16() })).into_response(),
        Err(e) => ok_json(json!({ "configured": true, "reachable": false, "error": e.to_string() })).into_response(),
    }
}

// ---------- websocket telemetry ----------

pub async fn ws_telemetry(
    State(app): State<AppState>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| telemetry_loop(app, socket))
}

async fn telemetry_loop(app: AppState, mut socket: WebSocket) {
    loop {
        let sessions = app.cowork.memory.list_sessions(20).unwrap_or_default();
        let mut payload = Vec::new();
        for s in sessions {
            if let Ok(Some(state)) = app.cowork.session_state(&s.id).await {
                payload.push(json!({
                    "session_id": s.id,
                    "title": s.title,
                    "status": state.status.as_str(),
                    "running": is_running(&state),
                    "agents": state.agents.iter().map(|a| json!({
                        "name": a.spec.name,
                        "role": a.spec.role.as_str(),
                        "status": a.status.as_str(),
                    })).collect::<Vec<_>>(),
                    "last_event": state.events.last().map(|e| e.kind.clone()),
                }));
            }
        }
        let msg = json!({ "type": "telemetry", "now_ms": now_ms(), "sessions": payload });
        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

// ---------- errors ----------

fn err500(e: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

fn bad_request(msg: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({ "error": msg })),
    )
        .into_response()
}
