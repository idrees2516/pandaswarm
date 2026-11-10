//! The shared vocabulary of PandaSwarm: tasks, plans, agent specs, artifacts,
//! sandbox profiles and the ubiquitous `SwarmEvent`.
//!
//! `SwarmEvent` is deliberately schemaless in its payload (a `serde_json::Value`):
//! the durable-object framework treats events as opaque facts to append and
//! replay, while typed helpers (`SwarmEvent::agent_spawned` etc.) keep call
//! sites honest. The frontend consumes the same JSON over REST/WebSocket.

use serde::{Deserialize, Serialize};
use serde_json::json;

/// Functional role an agent plays inside a swarm (CoworkOS-style tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Tier 1 — talks to the human, owns the plan, receives results.
    Master,
    /// Tier 2 — schedules and routes work between sub-agents.
    Coordinator,
    /// Tier 3 — does the actual work.
    Planner,
    Coder,
    Reviewer,
    Tester,
    Researcher,
    Integrator,
    /// Drives a browser/desktop through CDP.
    ComputerUser,
}

impl AgentRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentRole::Master => "master",
            AgentRole::Coordinator => "coordinator",
            AgentRole::Planner => "planner",
            AgentRole::Coder => "coder",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Tester => "tester",
            AgentRole::Researcher => "researcher",
            AgentRole::Integrator => "integrator",
            AgentRole::ComputerUser => "computer_user",
        }
    }
}

/// Lifecycle of an agent inside a swarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    #[default]
    Pending,
    Running,
    /// Blocked on a tool/sandbox call.
    WaitingTool,
    WaitingSandbox,
    Done,
    Failed,
    Cancelled,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Pending => "pending",
            AgentStatus::Running => "running",
            AgentStatus::WaitingTool => "waiting_tool",
            AgentStatus::WaitingSandbox => "waiting_sandbox",
            AgentStatus::Done => "done",
            AgentStatus::Failed => "failed",
            AgentStatus::Cancelled => "cancelled",
        }
    }
}

/// A spawning recipe for one sub-agent (agents are *pure data* between turns).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub role: AgentRole,
    pub goal: String,
    /// Layered workspace dir (relative to the session root), e.g. `swarm/coder-1`.
    pub workspace: String,
    /// LLM model hint, e.g. `mock`, `gpt-4o-mini`, `glm-4.6`.
    pub model: String,
    /// Tool names the agent may call: write_file, read_file, sandbox_run,
    /// rag_search, web_search, computer_use, mcp.
    pub tools: Vec<String>,
    /// Agents that must reach `Done` before this one starts.
    pub depends_on: Vec<String>,
    pub max_turns: u32,
}

/// One milestone of a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub id: String,
    pub title: String,
    pub acceptance: Vec<String>,
    pub agent_ids: Vec<String>,
}

/// A plan decomposing a task into milestones + a swarm of agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub task_id: String,
    pub objective: String,
    pub milestones: Vec<Milestone>,
    pub created_ms: u64,
}

/// Top-level task record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub session_id: String,
    pub prompt: String,
    pub status: TaskStatus,
    pub plan_id: Option<String>,
    pub repo: Option<String>,
    pub created_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Queued,
    Planning,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Planning => "planning",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

/// A file produced or modified by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub agent_id: String,
    pub path: String,
    pub language: String,
    pub content: String,
    pub bytes: u64,
    pub created_ms: u64,
}

/// Structured outcome of running a validation command in a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestReport {
    pub passed: u32,
    pub failed: u32,
    pub errors: u32,
    pub skipped: u32,
    pub duration_ms: u64,
    pub backend: String,
    pub raw_output: String,
    /// Recognised failure signatures, e.g. "AssertionError", "compile_error".
    pub failure_patterns: Vec<String>,
}

impl TestReport {
    pub fn is_green(&self) -> bool {
        self.failed == 0 && self.errors == 0
    }
}

/// Where a sandbox run happens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    /// bubblewrap jail (ro root + rw workdir + no net).
    Bwrap,
    /// `unshare --net --pid --fork` fallback.
    Unshare,
    /// No isolation — plain `timeout` + tempdir (dev machines only).
    #[default]
    Plain,
    /// instabox.sh cloud VM (4 vCPU / 8 GB, templates like `ci`).
    Instabox,
    /// Any E2B-compatible REST gateway (CubeSandbox, sandstorm, nvx…).
    E2bCompat,
}

impl SandboxBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxBackend::Bwrap => "bwrap",
            SandboxBackend::Unshare => "unshare",
            SandboxBackend::Plain => "plain",
            SandboxBackend::Instabox => "instabox",
            SandboxBackend::E2bCompat => "e2b_compat",
        }
    }
}

/// The contract every sandbox run is executed under.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxProfile {
    /// Host paths mounted read-only (best effort on `plain`).
    pub fs_ro: Vec<String>,
    /// Host dir mounted read-write as the working directory.
    pub fs_rw: String,
    /// Network access inside the sandbox.
    pub net_enabled: bool,
    pub timeout_secs: u64,
    /// Only these env vars are forwarded (plus PATH/HOME/LANG).
    pub env_allowlist: Vec<String>,
    pub backend: SandboxBackend,
}

impl Default for SandboxProfile {
    fn default() -> Self {
        Self {
            fs_ro: vec![],
            fs_rw: String::new(),
            net_enabled: false,
            timeout_secs: 60,
            env_allowlist: vec![],
            backend: SandboxBackend::Plain,
        }
    }
}

/// Result of one sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub backend: String,
}

impl ExecResult {
    pub fn ok(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }
}

/// One opaque event in a swarm's append-only log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmEvent {
    pub id: String,
    /// Monotonic per-swarm sequence number.
    pub seq: u64,
    pub ts_ms: u64,
    /// Dotted kind, e.g. `agent.turn_completed`, `sandbox.run`.
    pub kind: String,
    pub payload: serde_json::Value,
}

impl SwarmEvent {
    fn base(kind: &str, seq: u64, payload: serde_json::Value) -> Self {
        Self {
            id: crate::ids::new_id(crate::ids::prefix::EVENT),
            seq,
            ts_ms: crate::now_ms(),
            kind: kind.to_string(),
            payload,
        }
    }

    pub fn task_created(seq: u64, task: &TaskRecord) -> Self {
        Self::base("task.created", seq, json!({ "task": task }))
    }
    pub fn plan_generated(seq: u64, plan: &Plan, agents: &[AgentSpec]) -> Self {
        Self::base(
            "plan.generated",
            seq,
            json!({ "plan": plan, "agents": agents }),
        )
    }
    pub fn agent_spawned(seq: u64, agent: &AgentSpec) -> Self {
        Self::base("agent.spawned", seq, json!({ "agent": agent }))
    }
    pub fn agent_status(seq: u64, agent_id: &str, status: AgentStatus, detail: &str) -> Self {
        Self::base(
            "agent.status",
            seq,
            json!({ "agent_id": agent_id, "status": status.as_str(), "detail": detail }),
        )
    }
    pub fn tool_call(seq: u64, agent_id: &str, tool: &str, input: serde_json::Value, output: serde_json::Value) -> Self {
        Self::base("tool.call", seq, json!({
            "agent_id": agent_id, "tool": tool, "input": input, "output": output
        }))
    }
    pub fn sandbox_run(seq: u64, agent_id: &str, cmd: &str, result: &ExecResult) -> Self {
        Self::base("sandbox.run", seq, json!({ "agent_id": agent_id, "cmd": cmd, "result": result }))
    }
    pub fn test_report(seq: u64, agent_id: &str, report: &TestReport) -> Self {
        Self::base("sandbox.test_report", seq, json!({ "agent_id": agent_id, "report": report }))
    }
    pub fn artifact_written(seq: u64, agent_id: &str, path: &str, bytes: u64) -> Self {
        Self::base("artifact.written", seq, json!({ "agent_id": agent_id, "path": path, "bytes": bytes }))
    }
    pub fn harness_suggestion(seq: u64, pattern: &str, suggestion: &str, priority: u8) -> Self {
        Self::base(
            "harness.suggestion",
            seq,
            json!({ "pattern": pattern, "suggestion": suggestion, "priority": priority }),
        )
    }
    pub fn log(seq: u64, level: &str, message: &str) -> Self {
        Self::base("log", seq, json!({ "level": level, "message": message }))
    }
    pub fn milestone(seq: u64, milestone_id: &str, title: &str) -> Self {
        Self::base("milestone.completed", seq, json!({ "milestone_id": milestone_id, "title": title }))
    }
    pub fn task_completed(seq: u64, status: TaskStatus, summary: &str) -> Self {
        Self::base("task.completed", seq, json!({ "status": status.as_str(), "summary": summary }))
    }
}
