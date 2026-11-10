//! Prefixed, sortable-ish identifiers (Cloudflare-DO naming flavour).
//!
//! Every PandaSwarm entity gets an id like `swarm_9f1c2a4b7d3e` — a prefix that
//! documents the kind, plus 12 hex chars of UUIDv4. Keeping ids as plain strings
//! (instead of newtypes) means every layer — durable logs, SQLite rows, the REST
//! API and the Next.js frontend — can pass them around as opaque JSON values.

use uuid::Uuid;

/// Generate a new prefixed id, e.g. `new_id("agent") == "agent_9f1c2a4b7d3e"`.
pub fn new_id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().simple())
}

/// Well-known id prefixes used across the system.
pub mod prefix {
    pub const SESSION: &str = "sess";
    pub const TASK: &str = "task";
    pub const PLAN: &str = "plan";
    pub const AGENT: &str = "agent";
    pub const SWARM: &str = "swarm";
    pub const MILESTONE: &str = "mile";
    pub const ARTIFACT: &str = "arti";
    pub const EVENT: &str = "evt";
    pub const JOB: &str = "job";
    pub const RUN: &str = "run";
    pub const FACT: &str = "fact";
    pub const SANDBOX: &str = "sbx";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = new_id(prefix::AGENT);
        let b = new_id(prefix::AGENT);
        assert!(a.starts_with("agent_"));
        assert_ne!(a, b);
        assert_eq!(a.len(), "agent_".len() + 32);
    }
}
