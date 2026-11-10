//! `pandaswarm` CLI — serve, task, chat, demo, rag, memory, sandbox, doctor.

use anyhow::Result;
use clap::{Parser, Subcommand};
use pandaswarm_core::PandaSwarmConfig;
use pandaswarm_orchestrator::{is_running, Cowork};
use serde_json::json;
use std::io::Write;

#[derive(Parser)]
#[command(name = "pandaswarm", version, about = "🐼 PandaSwarm — the most agentic coding-agent orchestration platform")]
struct Cli {
    /// Path to pandaswarm.toml
    #[arg(long, global = true)]
    config: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the REST + WebSocket server.
    Serve,
    /// Run a task through a full swarm, printing events live.
    Task {
        prompt: String,
        /// Print only the final summary.
        #[arg(long)]
        quiet: bool,
    },
    /// Interactive chat with the master agent (reads stdin).
    Chat {
        #[arg(long, default_value = "sess-cli-chat")]
        session: String,
    },
    /// Offline end-to-end demo (mock LLM — no keys, no network).
    Demo {
        #[arg(long, default_value = "implement fibonacci and palindrome utilities with tests")]
        task: String,
    },
    /// RAG index operations.
    Rag {
        #[command(subcommand)]
        cmd: RagCmd,
    },
    /// Memory operations.
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
    },
    /// Run a command inside a sandbox (files staged with --file).
    Sandbox {
        #[arg(long)]
        cmd: String,
        /// stage file as path=content or path:@localfile
        #[arg(long = "file")]
        files: Vec<String>,
        #[arg(long)]
        backend: Option<String>,
        #[arg(long, default_value = "120")]
        timeout: u64,
    },
    /// Environment report.
    Doctor,
}

#[derive(Subcommand)]
enum RagCmd {
    /// (Re)index the workspace.
    Index,
    /// Hybrid search.
    Search { query: String, #[arg(long, default_value = "8")] top_k: usize },
}

#[derive(Subcommand)]
enum MemoryCmd {
    /// List semantic facts.
    Facts,
    /// Remember a fact.
    Remember { content: String, #[arg(long, default_value = "note")] kind: String },
    /// Recall by query.
    Recall { query: String },
    /// Search message history.
    History { query: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg_path = cli.config.as_deref().map(std::path::Path::new);
    match cli.cmd {
        Cmd::Serve => {
            // Delegate to the server binary (installed alongside `pandaswarm`).
            let cfg = PandaSwarmConfig::load(cfg_path)?;
            let bin = std::env::var("PANDASWARM_SERVER_BIN").unwrap_or_else(|_| "pandaswarm-server".into());
            let mut cmd = std::process::Command::new(bin);
            cmd.env("PANDASWARM_BIND", cfg.server.bind.clone());
            if let Some(p) = cli.config {
                cmd.env("PANDASWARM_CONFIG", p);
            }
            let status = cmd.status()?;
            if !status.success() {
                anyhow::bail!("pandaswarm-server exited with {status} (is it installed / on PATH?)");
            }
            Ok(())
        }
        Cmd::Task { prompt, quiet } => {
            let cfg = PandaSwarmConfig::load(cfg_path)?;
            // force the mock provider if no key is present so `task` always works
            let cfg = if cfg.llm.api_key.is_empty() && cfg.llm.provider != "mock" && cfg.llm.provider != "ollama" {
                let mut c = cfg;
                c.llm.provider = "mock".into();
                c
            } else {
                cfg
            };
            let cowork = Cowork::new(cfg)?;
            cowork.start().await;
            let session_id = pandaswarm_core::new_id("sess");
            cowork.run_task(&session_id, &prompt, &prompt.chars().take(60).collect::<String>()).await?;
            follow_session(&cowork, &session_id, !quiet).await
        }
        Cmd::Chat { session } => {
            let cfg = PandaSwarmConfig::load(cfg_path)?;
            let cowork = Cowork::new(cfg)?;
            cowork.start().await;
            let _ = cowork.memory.create_session(&session, "cli chat", "chat");
            let stdin = std::io::stdin();
            loop {
                print!("you> ");
                std::io::stdout().flush()?;
                let mut line = String::new();
                if stdin.read_line(&mut line)? == 0 {
                    break;
                }
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line == "exit" || line == "quit" {
                    break;
                }
                let reply = cowork.chat(&session, line).await?;
                println!("🐼 {reply}");
            }
            Ok(())
        }
        Cmd::Demo { task } => {
            let mut cfg = PandaSwarmConfig::load(cfg_path)?;
            cfg.llm.provider = "mock".into(); // offline by design
            let cowork = Cowork::new(cfg)?;
            cowork.start().await;
            let session_id = pandaswarm_core::new_id("sess");
            println!("🐼 demo task: {task}");
            cowork.run_task(&session_id, &task, "demo").await?;
            follow_session(&cowork, &session_id, true).await
        }
        Cmd::Rag { cmd } => {
            let cfg = PandaSwarmConfig::load(cfg_path)?;
            let cowork = Cowork::new(cfg)?;
            match cmd {
                RagCmd::Index => {
                    let mut rag = cowork.rag.lock().await;
                    let (scanned, reindexed) = rag.reindex()?;
                    println!("indexed: {} files scanned, {} re-read, {} chunks", scanned, reindexed, rag.chunk_count());
                }
                RagCmd::Search { query, top_k } => {
                    let rag = cowork.rag.lock().await;
                    for (i, h) in rag.search(&query, top_k).iter().enumerate() {
                        println!("{}. {}:{}-{}  (score {:.4}, bm25 {:?}, vec {:?})", i + 1, h.path, h.line_start, h.line_end, h.score, h.bm25_rank, h.vector_rank);
                        for line in h.snippet.lines().take(4) {
                            println!("   | {line}");
                        }
                    }
                }
            }
            Ok(())
        }
        Cmd::Memory { cmd } => {
            let cfg = PandaSwarmConfig::load(cfg_path)?;
            let cowork = Cowork::new(cfg)?;
            match cmd {
                MemoryCmd::Facts => {
                    for f in cowork.memory.list_facts(50)? {
                        println!("[{:.2} ×{}] {}: {}", f.salience, f.hits, f.kind, f.content);
                    }
                }
                MemoryCmd::Remember { content, kind } => {
                    let id = cowork.memory.remember_fact(None, &kind, &content)?;
                    println!("remembered {id}");
                }
                MemoryCmd::Recall { query } => {
                    for f in cowork.memory.recall(&query, 10, 0.0)? {
                        println!("[{:.2} ×{}] {}", f.salience, f.hits, f.content);
                    }
                }
                MemoryCmd::History { query } => {
                    for m in cowork.memory.search_history(None, &query, 20)? {
                        println!("{} [{}] {}", m.created_ms, m.role, m.content.chars().take(120).collect::<String>());
                    }
                }
            }
            Ok(())
        }
        Cmd::Sandbox { cmd, files, backend, timeout } => {
            let mut cfg = PandaSwarmConfig::load(cfg_path)?;
            if let Some(b) = backend {
                cfg.sandbox.backend = b;
            }
            let cowork = Cowork::new(cfg)?;
            let staged: Vec<(String, String)> = files
                .iter()
                .filter_map(|f| {
                    let (path, rest) = f.split_once('=')?;
                    let content = rest.strip_prefix('@').map(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| Some(rest.to_string()))?;
                    Some((path.to_string(), content))
                })
                .collect();
            let profile = pandaswarm_core::SandboxProfile {
                fs_ro: vec![],
                fs_rw: String::new(),
                net_enabled: false,
                timeout_secs: timeout,
                env_allowlist: vec![],
                backend: cowork.sandbox.backend(),
            };
            let res = cowork.sandbox.exec(&profile, &staged, &cmd).await?;
            println!("{}", res.stdout);
            eprintln!("{}", res.stderr);
            println!("── exit {} ({}, {} ms)", res.exit_code, res.backend, res.duration_ms);
            Ok(())
        }
        Cmd::Doctor => {
            let cfg = PandaSwarmConfig::load(cfg_path)?;
            let cowork = Cowork::new(cfg)?;
            let report = cowork.doctor().await;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

/// Poll the swarm and stream its events to stdout until it finishes.
async fn follow_session(cowork: &Cowork, session_id: &str, verbose: bool) -> Result<()> {
    let mut seen = 0usize;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let Some(state) = cowork.session_state(session_id).await? else {
            anyhow::bail!("session vanished");
        };
        if verbose {
            for ev in state.events.iter().skip(seen) {
                println!(
                    "  #{:<3} {:<22} {}",
                    ev.seq,
                    ev.kind,
                    serde_json::to_string(&ev.payload).unwrap_or_default().chars().take(160).collect::<String>()
                );
            }
        }
        seen = state.events.len();
        if !is_running(&state) {
            println!();
            println!("═══ task {} ═══", state.status.as_str());
            println!("{}", state.summary);
            for a in &state.agents {
                println!(
                    "  {} [{}/{}] {} turn(s), artifacts: {:?}",
                    a.spec.name,
                    a.spec.role.as_str(),
                    a.status.as_str(),
                    a.turns_used,
                    a.artifacts
                );
            }
            for r in &state.reports {
                println!(
                    "  report: {}✅ {}❌ ({})",
                    r.passed, r.failed + r.errors, r.backend
                );
            }
            let ws = cowork.cfg.workspace_root.join(session_id);
            if let Ok(commits) = pandaswarm_orchestrator::gitops::log(&ws, 5) {
                for c in commits {
                    println!("  git: {c}");
                }
            }
            return Ok(());
        }
    }
}

// keep json! in scope for future commands
#[allow(dead_code)]
fn _unused() -> serde_json::Value {
    json!({})
}
