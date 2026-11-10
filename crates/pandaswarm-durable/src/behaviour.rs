//! The `Behaviour` contract: a deterministic reducer plus an async command
//! handler — the pi-durable / Cloudflare-DO triad of `Event`, `State`,
//! `Command`.
//!
//! * `reduce(state, event)` is **pure and synchronous**: replaying the same
//!   events always yields the same state. It is the only way state changes.
//! * `handle(state, ctx, command)` may perform I/O (LLM calls, sandbox runs…)
//!   but **never mutates state directly** — it returns events, which the host
//!   folds through `reduce` and appends to the durable log.
//! * `pending_alarms(state, now_ms)` lets an object wake itself up later:
//!   alarms live *inside* state (deterministic), and are cancelled by events
//!   once handled — giving at-least-once delivery across restarts.

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// Mutable context handed to `handle` — durable KV plus a clock.
pub struct Ctx<'a> {
    pub kv: &'a mut std::collections::BTreeMap<String, Value>,
    pub now_ms: u64,
}

impl<'a> Ctx<'a> {
    pub fn kv_get(&self, key: &str) -> Option<&Value> {
        self.kv.get(key)
    }
    pub fn kv_put(&mut self, key: impl Into<String>, value: Value) {
        self.kv.insert(key.into(), value);
    }
    pub fn kv_remove(&mut self, key: &str) -> Option<Value> {
        self.kv.remove(key)
    }
}

/// Everything a durable object can be, from the framework's point of view.
#[async_trait]
pub trait Behaviour: Send + Sync + 'static {
    type State: Serialize + DeserializeOwned + Clone + Send + Sync + 'static + Default;
    type Event: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    type Command: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;

    /// Events seeded into a brand-new object (folded through `reduce` too).
    fn initial_events(_state: &Self::State) -> Vec<Self::Event> {
        Vec::new()
    }

    /// The deterministic fold. This is the *only* place state may change.
    fn reduce(state: &mut Self::State, event: &Self::Event);

    /// React to a command: may do async I/O, returns events to fold+persist.
    /// `&self` lets the behaviour instance carry shared services (LLM client,
    /// sandbox pool, RAG index…) — state itself stays out here.
    async fn handle(
        &self,
        state: &Self::State,
        ctx: &mut Ctx<'_>,
        command: &Self::Command,
    ) -> Vec<Self::Event>;

    /// Commands whose alarm time has arrived. Non-consuming on purpose: the
    /// object cancels an alarm by folding an event; a crash before that
    /// simply re-delivers it (at-least-once).
    fn pending_alarms(_state: &Self::State, _now_ms: u64) -> Vec<Self::Command> {
        Vec::new()
    }
}
