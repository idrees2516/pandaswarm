//! Sub-agent turn execution: build the prompt (with machine-readable
//! `<task-hints>`), call the LLM, dispatch tool calls against the real
//! services (files, sandbox, RAG, web), and return the events to fold.

use anyhow::Result;
use pandaswarm_core::{
    new_id, prefix, AgentSpec, AgentStatus, Artifact, ExecResult, SandboxProfile, SwarmEvent,
    TaskRecord, TestReport,
};
use pandaswarm_llm::{ChatMessage, ChatRequest, ToolCallReq};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::swarm::SwarmServices;

/// One agent's runtime state, carried inside the swarm object.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentRuntime {
    pub spec: AgentSpec,
    pub status: AgentStatus,
    pub turns_used: u32,
    pub artifacts: Vec<String>, // paths in the layered workspace
    pub last_error: Option<String>,
    pub finished_ms: Option<u64>,
}

impl AgentRuntime {
    pub fn from_spec(spec: AgentSpec) -> Self {
        Self {
            spec,
            status: AgentStatus::Pending,
            turns_used: 0,
            artifacts: vec![],
            last_error: None,
            finished_ms: None,
        }
    }
}

/// Execute one turn of `agent` and return the events it produced.
/// This is the *only* place that talks to LLM/sandbox/rag — results come
/// back as events, keeping the reducer pure.
pub async fn run_turn(
    services: &SwarmServices,
    task: &TaskRecord,
    agent: &AgentRuntime,
    workspace_root: &Path,
    session_id: &str,
    seq: &mut u64,
) -> Vec<SwarmEvent> {
    let mut events = Vec::new();
    let workspace = workspace_root.join(&agent.spec.workspace);

    let workspace_files = list_workspace_files(&workspace);
    let hints = json!({
        "role": match agent.spec.role {
            pandaswarm_core::AgentRole::Coder => "coder",
            pandaswarm_core::AgentRole::Tester => "tester",
            pandaswarm_core::AgentRole::Reviewer => "reviewer",
            pandaswarm_core::AgentRole::Planner => "planner",
            pandaswarm_core::AgentRole::Integrator => "integrator",
            pandaswarm_core::AgentRole::Researcher => "researcher",
            pandaswarm_core::AgentRole::ComputerUser => "computer_user",
            _ => "coder",
        },
        "goal": agent.spec.goal,
        "agent_name": agent.spec.name,
        "language": services.default_language,
        "workspace_files": workspace_files,
        "task": task.prompt,
        "turn": agent.turns_used,
    });

    let req = ChatRequest {
        model: agent.spec.model.clone(),
        temperature: 0.2,
        max_tokens: 4096,
        tools: pandaswarm_llm::coding_tools(),
        messages: vec![
            ChatMessage::system(format!(
                "You are {} (role: {}) in a PandaSwarm swarm. Goal: {}. \
                 Use the provided tools; validate code with sandbox_run before finishing. \
                 A machine-readable hint block follows.",
                agent.spec.name,
                agent.spec.role.as_str(),
                agent.spec.goal
            )),
            ChatMessage::user(format!("<task-hints>{hints}</task-hints>")),
        ],
    };

    events.push(SwarmEvent::agent_status(
        bump(seq),
        &agent.spec.id,
        AgentStatus::Running,
        "turn started",
    ));

    let resp = match services.llm.chat(&req).await {
        Ok(r) => r,
        Err(e) => {
            events.push(SwarmEvent::agent_status(
                bump(seq),
                &agent.spec.id,
                AgentStatus::Failed,
                &format!("llm error: {e}"),
            ));
            return events;
        }
    };

    if !resp.content.is_empty() {
        events.push(SwarmEvent::log(
            bump(seq),
            "info",
            &format!("{}: {}", agent.spec.name, first_line(&resp.content, 300)),
        ));
    }

    if resp.tool_calls.is_empty() {
        // No tools requested → the agent is done talking.
        events.push(SwarmEvent::agent_status(
            bump(seq),
            &agent.spec.id,
            AgentStatus::Done,
            "no further tool calls",
        ));
        return events;
    }

    for tc in &resp.tool_calls {
        let evs = dispatch_tool(services, agent, &workspace, session_id, seq, tc).await;
        events.extend(evs);
    }

    events
}

async fn dispatch_tool(
    services: &SwarmServices,
    agent: &AgentRuntime,
    workspace: &Path,
    session_id: &str,
    seq: &mut u64,
    tc: &ToolCallReq,
) -> Vec<SwarmEvent> {
    let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
    match tc.name.as_str() {
        "write_file" => {
            let path = args["path"].as_str().unwrap_or_default().to_string();
            let content = args["content"].as_str().unwrap_or_default().to_string();
            if path.is_empty() {
                return vec![SwarmEvent::log(bump(seq), "warn", "write_file: empty path")];
            }
            let safe = sanitize_rel_path(&path);
            let full = workspace.join(&safe);
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&full, &content).is_err() {
                return vec![SwarmEvent::agent_status(
                    bump(seq),
                    &agent.spec.id,
                    AgentStatus::Failed,
                    &format!("cannot write {safe}"),
                )];
            }
            let artifact = Artifact {
                id: new_id(prefix::ARTIFACT),
                agent_id: agent.spec.id.clone(),
                path: safe.clone(),
                language: language_of(&safe),
                content: content.clone(),
                bytes: content.len() as u64,
                created_ms: pandaswarm_core::now_ms(),
            };
            let _ = services
                .memory
                .upsert_artifact(session_id, &agent.spec.id, &safe, &artifact.language, &content);
            // index the new artifact for RAG
            {
                let mut rag = services.rag.lock().await;
                rag.add_document(&format!("{}/{}", agent.spec.workspace, safe), &content);
            }
            vec![SwarmEvent::artifact_written(
                bump(seq),
                &agent.spec.id,
                &safe,
                content.len() as u64,
            )]
        }
        "read_file" => {
            let path = args["path"].as_str().unwrap_or_default().to_string();
            let safe = sanitize_rel_path(&path);
            let content = std::fs::read_to_string(workspace.join(&safe)).unwrap_or_default();
            vec![SwarmEvent::tool_call(
                bump(seq),
                &agent.spec.id,
                "read_file",
                json!({"path": safe}),
                json!({"bytes": content.len()}),
            )]
        }
        "sandbox_run" => {
            let cmd = args["cmd"].as_str().unwrap_or("true").to_string();
            let timeout = args["timeout_secs"].as_u64().unwrap_or(services.default_timeout_secs);
            let profile = SandboxProfile {
                fs_ro: vec![],
                fs_rw: workspace.display().to_string(),
                net_enabled: services.sandbox_net,
                timeout_secs: timeout,
                env_allowlist: vec![],
                backend: services.sandbox_backend.clone(),
            };
            // Stage every artifact written so far in this workspace.
            let files = collect_workspace_files(workspace);
            let result: Result<ExecResult> = services.sandbox.exec(&profile, &files, &cmd).await;
            match result {
                Ok(res) => {
                    let report = pandaswarm_sandbox::parse_test_report(&res);
                    let mut evs = vec![
                        SwarmEvent::sandbox_run(bump(seq), &agent.spec.id, &cmd, &res),
                        SwarmEvent::test_report(bump(seq), &agent.spec.id, &report),
                    ];
                    if !report.is_green() {
                        for s in pandaswarm_sandbox::suggest_harness_patches(&report, &artifact_paths_of(agent)) {
                            evs.push(SwarmEvent::harness_suggestion(
                                bump(seq),
                                &s.pattern,
                                &s.suggestion,
                                s.priority,
                            ));
                        }
                    }
                    let _ = services.memory.add_tool_call(
                        session_id,
                        &agent.spec.id,
                        "sandbox_run",
                        &json!({"cmd": cmd}),
                        &serde_json::to_value(&report).unwrap_or(json!({})),
                    );
                    evs
                }
                Err(e) => vec![SwarmEvent::log(
                    bump(seq),
                    "error",
                    &format!("sandbox error: {e}"),
                )],
            }
        }
        "rag_search" => {
            let query = args["query"].as_str().unwrap_or_default().to_string();
            let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;
            let rag = services.rag.lock().await;
            let hits = rag.search(&query, top_k);
            drop(rag);
            vec![SwarmEvent::tool_call(
                bump(seq),
                &agent.spec.id,
                "rag_search",
                json!({"query": query}),
                json!({"hits": hits.iter().map(|h| json!({"path": h.path, "lines": format!("{}-{}", h.line_start, h.line_end), "score": h.score})).collect::<Vec<_>>()}),
            )]
        }
        "web_search" => {
            let query = args["query"].as_str().unwrap_or_default().to_string();
            match pandaswarm_search::web_search(&services.web_cfg, &query, 5).await {
                Ok(results) => vec![SwarmEvent::tool_call(
                    bump(seq),
                    &agent.spec.id,
                    "web_search",
                    json!({"query": query}),
                    json!({"results": results}),
                )],
                Err(e) => vec![SwarmEvent::log(bump(seq), "warn", &format!("web_search: {e}"))],
            }
        }
        other => vec![SwarmEvent::log(
            bump(seq),
            "warn",
            &format!("unknown tool `{other}` requested by {}", agent.spec.name),
        )],
    }
}

fn bump(seq: &mut u64) -> u64 {
    *seq += 1;
    *seq
}

fn first_line(s: &str, max: usize) -> String {
    let l = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    l.chars().take(max).collect()
}

fn sanitize_rel_path(p: &str) -> String {
    let mut out = PathBuf::new();
    for part in p.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        out.push(part);
    }
    out.display().to_string()
}

fn language_of(path: &str) -> String {
    match path.rsplit('.').next().unwrap_or("") {
        "py" => "python".into(),
        "js" | "mjs" | "cjs" => "javascript".into(),
        "ts" => "typescript".into(),
        "rs" => "rust".into(),
        "go" => "go".into(),
        "sh" => "bash".into(),
        "md" => "markdown".into(),
        _ => "text".into(),
    }
}

fn list_workspace_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_file() {
                out.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    out
}

fn collect_workspace_files(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(content) = std::fs::read_to_string(&p) {
                    if let Ok(rel) = p.strip_prefix(dir) {
                        out.push((rel.display().to_string(), content));
                    }
                }
            }
        }
    }
    out
}

fn artifact_paths_of(agent: &AgentRuntime) -> Vec<String> {
    agent.artifacts.clone()
}
