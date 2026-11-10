//! # pandaswarm-durable
//!
//! Self-hosted **Durable Objects** for Rust, inspired by Cloudflare DOs,
//! pi-rs's `pi-durable` and rivet-agents:
//!
//! * an object is a `Behaviour` — `Event` / `State` / `Command` — whose state
//!   only ever changes through the deterministic `reduce`;
//! * `handle` performs async side effects (LLM, sandbox, network) and returns
//!   events which the host folds, appends to an append-only JSONL log and
//!   periodically snapshots;
//! * durable KV (`Ctx::kv_put/kv_get`) and alarms (`Alarms` book kept inside
//!   state, fired by a 250 ms runtime ticker) round out the DO contract;
//! * objects are addressed by class name, discovered through a typed registry
//!   with serde-erased boundaries (`AnyObject` + `ObjectAdapter<B>`).
//!
//! Agents therefore become *pure data between turns*: a sleeping sub-agent is
//! just a log on disk, and the whole swarm can be replayed after a crash.

pub mod alarms;
pub mod behaviour;
pub mod object;
pub mod runtime;

pub use alarms::{AlarmEntry, Alarms};
pub use behaviour::{Behaviour, Ctx};
pub use object::{AnyObject, ObjectAdapter};
pub use runtime::{DurableRuntime, ObjectFactory};
