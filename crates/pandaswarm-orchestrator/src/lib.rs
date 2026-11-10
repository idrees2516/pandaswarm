//! # pandaswarm-orchestrator
//!
//! The three-tier cowork engine (CoworkOS-inspired), built on
//! `pandaswarm-durable`:
//!
//! ```text
//! Tier 1  MasterAgent      talks to the human, owns the plan, receives results
//! Tier 2  Coordinator      (the SwarmObject) schedules work, routes messages,
//!                          keeps agents as pure data between turns
//! Tier 3  Sub-agents       coder / tester / reviewer / researcher execute in
//!                          layered workspaces, validated inside sandboxes
//! ```
//!
//! * every swarm is a **Durable Object** with an append-only event log,
//!   snapshots, KV and alarm-driven ticks — kill the process anywhere and
//!   `pandaswarm` resumes the swarm exactly where it stopped;
//! * sub-agents run **sync or async**: the coordinator awaits inline turns
//!   or schedules them as resumable background jobs;
//! * every sandbox run flows through the **harness** (`TestReport` → failure
//!   patterns → harness-patch suggestions) so the loop learns;
//! * finished work lands in **git worktrees** per agent branch, merged by
//!   the integrator (cezar-style PR flow).

pub mod agent;
pub mod cowork;
pub mod gitops;
pub mod jobs;
pub mod planner;
pub mod swarm;
pub mod tuner;

pub use cowork::{active_agents, is_running, summarize, Cowork};
pub use planner::{heuristic_plan, plan_swarm};
pub use swarm::{SwarmObject, SwarmServices, SwarmState};
