//! `SwarmObject` — the coordinator as a Durable Object (tier 2).
//!
//! State is pure data (task, plan, agents, milestones, event ring, stats).
//! `Tick` commands (self-scheduled via the alarm pattern) pick the next
//! runnable agent and execute one full turn — LLM, tools, sandbox — inside
//! `handle`; the outcome comes back as a `TurnCompleted` event that `reduce`
//! folds deterministically. Stop the process at any point: the log replays
//! and the next alarm resumes the swarm.

use async_trait::async_trait;
use pandaswarm_core::{
    now_ms, AgentStatus, Plan, SandboxBackend, TaskRecord, TaskStatus, TestReport,
};
use pandaswarm_durable::{Behaviour, Ctx};
use pandaswarm_llm::LlmProvider;
use pandaswarm_memory::MemoryStore;
use pandaswarm_rag::RagIndex;
use pandaswarm_search::web::WebSearchConfig;
use pandaswarm_sandbox::SandboxProvider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::agent::{run_turn, AgentRuntime};
use crate::planner;
use crate::tuner;

/// Shared services the swarm behaviour instance carries (Arc'd).
#[derive(Clone)]
pub struct SwarmServices {
    pub llm: Arc<dyn LlmProvider>,
    pub memory: Arc<MemoryStore>,
    pub rag: Arc<Mutex<RagIndex>>,
    pub sandbox: Arc<dyn SandboxProvider>,
    pub web_cfg: Arc<WebSearchConfig>,
    pub workspace_root: PathBuf,
    pub default_language: String,
    pub default_timeout_secs: u64,
    pub sandbox_net: bool,
    pub sandbox_backend: SandboxBackend,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SwarmState {
    pub session_id: String,
    pub task: Option<TaskRecord>,
    pub plan: Option<Plan>,
    pub agents: Vec<AgentRuntime>,
    pub milestone_done: Vec<String>,
    pub status: TaskStatus,
    pub events: Vec<pandaswarm_core::SwarmEvent>,
    pub reports: Vec<TestReport>,
    pub next_tick_ms: u64,
    pub summary: String,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
}

impl SwarmState {
    fn push_event(&mut self, mut ev: pandaswarm_core::SwarmEvent) {
        ev.seq = self.events.len() as u64 + 1;
        self.events.push(ev);
        if self.events.len() > 400 {
            let drop = self.events.len() - 400;
            self.events.drain(0..drop);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SwarmCommand {
    /// Kick off the swarm for a task (idempotent: ignored if already started).
    Start { session_id: String, task: TaskRecord },
    /// Advance the swarm by one unit of work (alarm-driven).
    Tick,
    /// Cancel everything.
    Cancel { reason: String },
    /// Force a rescan of the RAG index.
    Reindex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SwarmEventKind {
    TaskStarted { task: TaskRecord },
    PlanReady { plan: Plan, agents: Vec<pandaswarm_core::AgentSpec> },
    TickScheduled { at_ms: u64 },
    /// One agent turn finished; `events` are the raw SwarmEvents it produced.
    TurnCompleted {
        agent_id: String,
        events: Vec<pandaswarm_core::SwarmEvent>,
        agent_done: bool,
        agent_failed: bool,
    },
    TaskFinished { status: TaskStatus, summary: String },
    Cancelled { reason: String },
}

pub struct SwarmObject {
    pub services: SwarmServices,
}

const TICK_INTERVAL_MS: u64 = 50;

#[async_trait]
impl Behaviour for SwarmObject {
    type State = SwarmState;
    type Event = SwarmEventKind;
    type Command = SwarmCommand;

    fn reduce(state: &mut Self::State, event: &Self::Event) {
        match event {
            SwarmEventKind::TaskStarted { task } => {
                state.session_id = task.session_id.clone();
                state.task = Some(task.clone());
                state.status = TaskStatus::Planning;
                state.started_ms = Some(now_ms());
                let ev = pandaswarm_core::SwarmEvent::task_created(0, state.task.as_ref().unwrap());
                state.push_event(ev);
            }
            SwarmEventKind::PlanReady { plan, agents } => {
                state.plan = Some(plan.clone());
                state.agents = agents.iter().cloned().map(AgentRuntime::from_spec).collect();
                state.status = TaskStatus::Running;
                for spec in agents {
                    state.push_event(pandaswarm_core::SwarmEvent::agent_spawned(0, spec));
                }
                state.push_event(pandaswarm_core::SwarmEvent::plan_generated(0, plan, &[]));
                // re-emit plan.generated with the full agent list via a log event
                state.push_event(pandaswarm_core::SwarmEvent::log(
                    0,
                    "info",
                    &format!("swarm of {} agents scheduled across {} milestone(s)", state.agents.len(), plan.milestones.len()),
                ));
            }
            SwarmEventKind::TickScheduled { at_ms } => {
                state.next_tick_ms = *at_ms;
            }
            SwarmEventKind::TurnCompleted { agent_id, events, agent_done, agent_failed } => {
                // Phase 1: harvest the payload without holding any borrows.
                let mut reports: Vec<TestReport> = Vec::new();
                let mut new_artifacts: Vec<String> = Vec::new();
                for ev in events {
                    match ev.kind.as_str() {
                        "artifact.written" => {
                            new_artifacts.push(ev.payload["path"].as_str().unwrap_or_default().to_string());
                        }
                        "sandbox.test_report" => {
                            if let Ok(rep) =
                                serde_json::from_value::<TestReport>(ev.payload["report"].clone())
                            {
                                reports.push(rep);
                            }
                        }
                        _ => {}
                    }
                }

                // Phase 2: update the agent record (scoped mutable borrow).
                let mut completed = false;
                {
                    let Some(agent) = state.agents.iter_mut().find(|a| &a.spec.id == agent_id) else {
                        return;
                    };
                    agent.turns_used += 1;
                    agent.artifacts.extend(new_artifacts);
                    if *agent_failed {
                        agent.status = AgentStatus::Failed;
                        agent.last_error = Some("turn failed".into());
                        agent.finished_ms = Some(now_ms());
                    } else if *agent_done || agent.turns_used >= agent.spec.max_turns {
                        agent.status = AgentStatus::Done;
                        agent.finished_ms = Some(now_ms());
                        completed = true;
                    } else {
                        agent.status = AgentStatus::Running;
                    }
                }

                // Phase 3: append the raw event stream + reports.
                for ev in events {
                    state.push_event(ev.clone());
                }
                state.reports.extend(reports);

                // Phase 4: milestone bookkeeping once the agent is done.
                if completed {
                    let plan = state.plan.clone();
                    if let Some(plan) = plan {
                        for m in &plan.milestones {
                            if m.agent_ids.contains(agent_id) && !state.milestone_done.contains(&m.id) {
                                state.milestone_done.push(m.id.clone());
                                state.push_event(pandaswarm_core::SwarmEvent::milestone(0, &m.id, &m.title));
                            }
                        }
                    }
                }
            }
            SwarmEventKind::TaskFinished { status, summary } => {
                state.status = *status;
                state.summary = summary.clone();
                state.finished_ms = Some(now_ms());
                state.next_tick_ms = 0;
                state.push_event(pandaswarm_core::SwarmEvent::task_completed(0, *status, summary));
            }
            SwarmEventKind::Cancelled { reason } => {
                state.status = TaskStatus::Cancelled;
                state.next_tick_ms = 0;
                for a in state.agents.iter_mut() {
                    if matches!(a.status, AgentStatus::Pending | AgentStatus::Running) {
                        a.status = AgentStatus::Cancelled;
                    }
                }
                state.push_event(pandaswarm_core::SwarmEvent::log(0, "warn", &format!("cancelled: {reason}")));
            }
        }
    }

    async fn handle(&self, state: &Self::State, _ctx: &mut Ctx<'_>, command: &Self::Command) -> Vec<Self::Event> {
        match command {
            SwarmCommand::Start { session_id, task } => {
                if state.task.is_some() {
                    return vec![]; // idempotent under at-least-once delivery
                }
                let mut events = vec![SwarmEventKind::TaskStarted { task: task.clone() }];
                let (plan, agents) =
                    planner::plan_swarm(task, self.services.llm.clone(), &self.services.default_language)
                        .await
                        .unwrap_or_else(|_| planner::heuristic_plan(task, &self.services.default_language));
                let _ = self.services.memory.remember_fact(
                    Some(session_id),
                    "task",
                    &format!("task started: {}", task.prompt),
                );
                events.push(SwarmEventKind::PlanReady { plan, agents });
                events.push(SwarmEventKind::TickScheduled { at_ms: now_ms() + TICK_INTERVAL_MS });
                events
            }
            SwarmCommand::Tick => {
                if state.status != TaskStatus::Running {
                    return vec![];
                }
                let next = state
                    .agents
                    .iter()
                    .find(|a| {
                        a.status == AgentStatus::Pending
                            && a.spec.depends_on.iter().all(|dep| {
                                state.agents.iter().any(|d| {
                                    &d.spec.id == dep
                                        && matches!(d.status, AgentStatus::Done | AgentStatus::Cancelled)
                                })
                            })
                    })
                    .cloned()
                    .or_else(|| {
                        state
                            .agents
                            .iter()
                            .find(|a| a.status == AgentStatus::Running && a.turns_used < a.spec.max_turns)
                            .cloned()
                    });

                let Some(agent) = next else {
                    let all_terminal = state.agents.iter().all(|a| {
                        matches!(a.status, AgentStatus::Done | AgentStatus::Failed | AgentStatus::Cancelled)
                    });
                    if all_terminal {
                        let failed = state.agents.iter().filter(|a| a.status == AgentStatus::Failed).count();
                        let artifacts: usize = state.agents.iter().map(|a| a.artifacts.len()).sum();
                        let suggestions = tuner::aggregate(&state.reports);
                        let status = if failed > 0 { TaskStatus::Failed } else { TaskStatus::Completed };
                        let summary = format!(
                            "{} agents finished, {} artifact(s), {} sandbox report(s), {} harness suggestion(s)",
                            state.agents.len(),
                            artifacts,
                            state.reports.len(),
                            suggestions.len()
                        );
                        if let Some(t) = &state.task {
                            let ws = self.services.workspace_root.join(&t.session_id);
                            let _ = crate::gitops::commit_all(&ws, &format!("pandaswarm: {}", t.prompt));
                        }
                        return vec![SwarmEventKind::TaskFinished { status, summary }];
                    }
                    return vec![SwarmEventKind::TaskFinished {
                        status: TaskStatus::Failed,
                        summary: "no runnable agent and not all terminal (dependency cycle?)".into(),
                    }];
                };

                // ---- execute one agent turn (tier 3) ----
                let workspace_root = self.services.workspace_root.join(&state.session_id);
                std::fs::create_dir_all(workspace_root.join(&agent.spec.workspace)).ok();

                // Layered-workspace merge (CoworkOS tier-3 semantics): before
                // an agent runs, artifacts from its dependencies are staged
                // into its own workspace so validation sees the whole picture.
                for dep_id in &agent.spec.depends_on {
                    if let Some(dep) = state.agents.iter().find(|a| &a.spec.id == dep_id) {
                        let src = workspace_root.join(&dep.spec.workspace);
                        let dst = workspace_root.join(&agent.spec.workspace);
                        if let Ok(rd) = std::fs::read_dir(&src) {
                            for e in rd.flatten() {
                                if e.path().is_file() {
                                    let name = e.file_name();
                                    let to = dst.join(&name);
                                    if !to.exists() {
                                        let _ = std::fs::copy(e.path(), &to);
                                    }
                                }
                            }
                        }
                    }
                }
                let mut seq = state.events.len() as u64;
                let turn_events = run_turn(
                    &self.services,
                    state.task.as_ref().expect("task present while running"),
                    &agent,
                    &workspace_root,
                    &state.session_id,
                    &mut seq,
                )
                .await;

                let agent_failed = turn_events.iter().any(|e| {
                    e.kind == "agent.status" && e.payload["status"] == "failed"
                });
                let agent_done = turn_events
                    .iter()
                    .any(|e| e.kind == "agent.status" && e.payload["status"] == "done")
                    || turn_events.is_empty();

                vec![
                    SwarmEventKind::TurnCompleted {
                        agent_id: agent.spec.id.clone(),
                        events: turn_events,
                        agent_done,
                        agent_failed,
                    },
                    SwarmEventKind::TickScheduled { at_ms: now_ms() + TICK_INTERVAL_MS },
                ]
            }
            SwarmCommand::Cancel { reason } => {
                if matches!(state.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled) {
                    return vec![];
                }
                vec![SwarmEventKind::Cancelled { reason: reason.clone() }]
            }
            SwarmCommand::Reindex => {
                let mut rag = self.services.rag.lock().await;
                let _ = rag.reindex();
                vec![]
            }
        }
    }

    fn pending_alarms(state: &Self::State, now: u64) -> Vec<Self::Command> {
        if state.status == TaskStatus::Running && state.next_tick_ms > 0 && now >= state.next_tick_ms {
            vec![SwarmCommand::Tick]
        } else {
            vec![]
        }
    }
}
