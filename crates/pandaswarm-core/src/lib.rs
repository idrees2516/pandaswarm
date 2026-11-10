//! # pandaswarm-core
//!
//! Shared vocabulary of the PandaSwarm platform: prefixed ids, swarm events,
//! plans / agent specs / artifacts / sandbox profiles, configuration, and the
//! text kernel (BM25 + hashed embeddings + RRF) reused by memory and RAG.

pub mod config;
pub mod ids;
pub mod text;
pub mod types;

pub use config::{now_ms, PandaSwarmConfig};
pub use ids::{new_id, prefix};
pub use text::{cosine, hash_embed, rrf_fuse, tokenize, Bm25Index};
pub use types::{
    AgentRole, AgentSpec, AgentStatus, Artifact, ExecResult, Milestone, Plan, SandboxBackend,
    SandboxProfile, SwarmEvent, TaskRecord, TaskStatus, TestReport,
};
