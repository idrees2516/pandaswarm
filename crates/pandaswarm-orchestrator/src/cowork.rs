//! `Cowork` — the tier-1 master-agent façade the server and CLI drive.
//!
//! Owns the durable runtime, the services, and the per-session swarm
//! objects. A swarm's object class is `swarm:<session_id>`; sessions are
//! registered lazily and recovered from disk after restarts.

use anyhow::Result;
use pandaswarm_core::{now_ms, AgentStatus, PandaSwarmConfig, TaskStatus};
use pandaswarm_durable::DurableRuntime;
use pandaswarm_llm::build_provider;
use pandaswarm_memory::MemoryStore;
use pandaswarm_rag::RagIndex;
use pandaswarm_search::web::WebSearchConfig;
use pandaswarm_sandbox::{resolve_backend, SandboxProvider};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::planner::new_task;
use crate::swarm::{SwarmCommand, SwarmObject, SwarmServices, SwarmState};

pub struct Cowork {
    pub cfg: PandaSwarmConfig,
    pub runtime: Arc<DurableRuntime>,
    pub memory: Arc<MemoryStore>,
    pub rag: Arc<Mutex<RagIndex>>,
    pub llm: Arc<dyn pandaswarm_llm::LlmProvider>,
    pub sandbox: Arc<dyn SandboxProvider>,
    pub web_cfg: Arc<WebSearchConfig>,
}

impl Cowork {
    /// Wire everything up from a config (creates dirs, opens stores).
    pub fn new(cfg: PandaSwarmConfig) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.workspace_root)?;
        std::fs::create_dir_all(&cfg.data_dir)?;
        let memory = Arc::new(MemoryStore::open(
            cfg.data_dir.join("memory.sqlite3"),
            cfg.rag.vector_dim,
        )?);
        let rag = Arc::new(Mutex::new(RagIndex::new(
            cfg.workspace_root.clone(),
            cfg.rag.chunk_lines,
            cfg.rag.chunk_overlap,
            cfg.rag.vector_dim,
            cfg.rag.rrf_k,
        )));
        let llm = build_provider(&cfg)?;
        let sandbox: Arc<dyn SandboxProvider> = {
            let want = cfg.sandbox.backend.clone();
            let backend = resolve_backend(&want);
            match backend {
                pandaswarm_core::SandboxBackend::Instabox => {
                    Arc::new(pandaswarm_sandbox::instabox::InstaboxSandbox::new(&cfg.sandbox.instabox))
                }
                pandaswarm_core::SandboxBackend::E2bCompat => Arc::new(
                    pandaswarm_sandbox::e2b::E2bCompatSandbox::new(&cfg.sandbox.e2b.api_key, &cfg.sandbox.e2b.base_url),
                ),
                b => Arc::new(pandaswarm_sandbox::local::LocalSandbox::new(b)),
            }
        };
        let web_cfg = Arc::new(WebSearchConfig::from_core(&cfg));
        let runtime = DurableRuntime::new(cfg.data_dir.join("durable"));
        Ok(Arc::new(Self {
            cfg,
            runtime,
            memory,
            rag,
            llm,
            sandbox,
            web_cfg,
        }))
    }

    /// Start the alarm ticker + a swarm-driver loop.
    pub async fn start(self: &Arc<Self>) {
        self.runtime.clone().start_ticker();
        let cw = self.clone();
        tokio::spawn(async move {
            // Drive ticks in addition to the 250ms alarm wheel so long agent
            // turns don't starve: the runtime tick fires due alarms, which
            // includes next_tick scheduling. Nothing else needed — but we
            // keep a heartbeat for reindexing.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let mut rag = cw.rag.lock().await;
                let _ = rag.reindex();
            }
        });
    }

    fn services(&self) -> SwarmServices {
        SwarmServices {
            llm: self.llm.clone(),
            memory: self.memory.clone(),
            rag: self.rag.clone(),
            sandbox: self.sandbox.clone(),
            web_cfg: self.web_cfg.clone(),
            workspace_root: self.cfg.workspace_root.clone(),
            default_language: "python".into(),
            default_timeout_secs: self.cfg.sandbox.default_timeout_secs,
            sandbox_net: false,
            sandbox_backend: self.sandbox.backend(),
        }
    }

    fn class_for(session_id: &str) -> String {
        format!("swarm:{session_id}")
    }

    /// Tier 1 → tier 2: create a session and dispatch the task to its swarm.
    pub async fn run_task(self: &Arc<Self>, session_id: &str, prompt: &str, title: &str) -> Result<()> {
        self.memory.create_session(session_id, title, "task")?;
        self.memory.add_message(session_id, "user", prompt)?;
        let class = Self::class_for(session_id);
        let cw = self.clone();
        let class2 = class.clone();
        self.runtime
            .register::<SwarmObject, _>(&class, move || {
                Ok(SwarmObject {
                    services: cw.services(),
                })
            })
            .await;
        let task = new_task(session_id, prompt, None);
        self.runtime
            .send(
                &class2,
                serde_json::to_value(SwarmCommand::Start {
                    session_id: session_id.to_string(),
                    task,
                })?,
            )
            .await?;
        Ok(())
    }

    /// Plain chat turn (master agent answering from memory + RAG).
    pub async fn chat(self: &Arc<Self>, session_id: &str, content: &str) -> Result<String> {
        use pandaswarm_llm::{ChatMessage, ChatRequest};
        self.memory.add_message(session_id, "user", content)?;
        let facts = self.memory.recall(content, 4, 0.0).unwrap_or_default();
        let hits = {
            let rag = self.rag.lock().await;
            rag.search(content, 4)
        };
        let context = format!(
            "Semantic memory:\n{}\n\nRelevant indexed code:\n{}",
            facts
                .iter()
                .map(|f| format!("- ({:.2}) {}", f.salience, f.content))
                .collect::<Vec<_>>()
                .join("\n"),
            hits.iter()
                .map(|h| format!("- {}:{}-{} ({:.3})", h.path, h.line_start, h.line_end, h.score))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let req = ChatRequest::new(
            "",
            vec![
                ChatMessage::system(format!(
                    "You are the PandaSwarm master agent. Answer using the context when relevant.\n{context}"
                )),
                ChatMessage::user(content),
            ],
        );
        let resp = self.llm.chat(&req).await?;
        self.memory.add_message(session_id, "assistant", &resp.content)?;
        Ok(resp.content)
    }

    /// Full session snapshot for the API: task, agents, events, artifacts.
    pub async fn session_state(&self, session_id: &str) -> Result<Option<SwarmState>> {
        let class = Self::class_for(session_id);
        let obj = match self.runtime.get(&class).await {
            Ok(o) => o,
            Err(_) => return Ok(None),
        };
        let v = obj.state_json();
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(v)?))
    }

    /// Cancel a session's swarm.
    pub async fn cancel(&self, session_id: &str, reason: &str) -> Result<()> {
        let class = Self::class_for(session_id);
        self.runtime
            .send(
                &class,
                serde_json::to_value(SwarmCommand::Cancel { reason: reason.into() })?,
            )
            .await?;
        Ok(())
    }

    /// Doctor-style environment report.
    pub async fn doctor(&self) -> Value {
        let layers = pandaswarm_sandbox::detect_local_layers();
        let instabox_ok = std::process::Command::new("instabox")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let rag_files = {
            let r = self.rag.lock().await;
            r.file_count()
        };
        json!({
            "llm_provider": self.llm.name(),
            "sandbox_backend": self.sandbox.describe(),
            "local_layers": layers.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
            "instabox_cli": instabox_ok,
            "instabox_key_set": !self.cfg.sandbox.instabox.api_key.is_empty(),
            "web_provider": self.web_cfg.provider,
            "rag_files": rag_files,
            "durable_dir": self.cfg.data_dir.join("durable").display().to_string(),
            "now_ms": now_ms(),
        })
    }
}

/// Summarize a finished swarm state for humans.
pub fn summarize(state: &SwarmState) -> Value {
    json!({
        "status": state.status.as_str(),
        "summary": state.summary,
        "agents": state.agents.iter().map(|a| json!({
            "name": a.spec.name,
            "role": a.spec.role.as_str(),
            "status": a.status.as_str(),
            "turns": a.turns_used,
            "artifacts": a.artifacts,
        })).collect::<Vec<_>>(),
        "reports": state.reports.len(),
        "last_event": state.events.last().map(|e| e.kind.clone()),
    })
}

/// Helper for servers: agents that are still active.
pub fn active_agents(state: &SwarmState) -> usize {
    state
        .agents
        .iter()
        .filter(|a| !matches!(a.status, AgentStatus::Done | AgentStatus::Failed | AgentStatus::Cancelled))
        .count()
}

/// Helper for servers: is the task still in flight?
pub fn is_running(state: &SwarmState) -> bool {
    matches!(state.status, TaskStatus::Queued | TaskStatus::Planning | TaskStatus::Running)
}

/// Absolute workspace path of a session (used by the server for file APIs).
pub fn session_workspace(cfg: &PandaSwarmConfig, session_id: &str) -> PathBuf {
    cfg.workspace_root.join(session_id)
}
