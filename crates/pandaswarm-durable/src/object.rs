//! `ObjectAdapter<B>` — the bridge between a typed `Behaviour` and the
//! type-erased world of the runtime registry (the pi-durable "serde erasure"
//! trick): commands arrive as `serde_json::Value`, get deserialized into
//! `B::Command`, and events flow back as `Value` — so logs, snapshots and the
//! wire protocol are all plain JSON while implementations stay strongly typed.
//!
//! Persistence layout per object class under `<data_dir>/objects/`:
//! * `<class>.jsonl`     — append-only event log, one JSON line per event
//! * `<class>.snap.json` — full-state snapshot taken every `SNAPSHOT_EVERY`
//!                        events; the log is truncated after a snapshot
//!
//! Recovery = load snapshot + replay newer log lines through `reduce`.

use crate::behaviour::{Behaviour, Ctx};
use async_trait::async_trait;
use pandaswarm_core::now_ms;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;

const SNAPSHOT_EVERY: u64 = 200;

struct Envelope<B: Behaviour> {
    state: B::State,
    seq: u64,
    kv: std::collections::BTreeMap<String, Value>,
    since_snapshot: u64,
}

/// One instantiated durable object.
pub struct ObjectAdapter<B: Behaviour> {
    class: String,
    dir: PathBuf,
    behaviour: B,
    env: Mutex<Envelope<B>>,
    total_events: AtomicU64,
}

/// The type-erased view the runtime stores.
#[async_trait]
pub trait AnyObject: Send + Sync {
    fn class(&self) -> &str;
    /// Dispatch one command; returns the events it produced (already folded
    /// and persisted).
    async fn send(&self, command: Value) -> anyhow::Result<Vec<Value>>;
    /// Commands to dispatch right now because their alarms came due.
    async fn due_alarms(&self) -> Vec<Value>;
    fn state_json(&self) -> Value;
    fn kv_json(&self) -> Value;
    fn event_count(&self) -> u64;
    /// Force a snapshot (also done automatically every SNAPSHOT_EVERY events).
    fn snapshot(&self) -> anyhow::Result<()>;
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapFile<B: Behaviour> {
    seq: u64,
    state: B::State,
    kv: std::collections::BTreeMap<String, Value>,
}

impl<B: Behaviour> ObjectAdapter<B> {
    /// Create (or recover) the object `class` with the given behaviour
    /// instance (which usually carries Arc'd services).
    pub fn open(class: &str, dir: PathBuf, behaviour: B) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let snap_path = dir.join(format!("{class}.snap.json"));
        let log_path = dir.join(format!("{class}.jsonl"));

        let mut env = Envelope {
            state: B::State::default(),
            seq: 0,
            kv: Default::default(),
            since_snapshot: 0,
        };
        let mut recovered_events = 0u64;

        if snap_path.exists() {
            let txt = std::fs::read_to_string(&snap_path)?;
            let snap: SnapFile<B> = serde_json::from_str(&txt)?;
            env.state = snap.state;
            env.seq = snap.seq;
            env.kv = snap.kv;
        }
        if log_path.exists() {
            for line in std::fs::read_to_string(&log_path)?.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(row) = serde_json::from_str::<LogRow>(line) else {
                    tracing::warn!(class, "skipping corrupt log line");
                    continue;
                };
                if row.seq <= env.seq {
                    continue;
                }
                let ev: B::Event = serde_json::from_value(row.event)?;
                B::reduce(&mut env.state, &ev);
                env.seq = row.seq;
                recovered_events += 1;
            }
        }
        if env.seq == 0 {
            // Fresh object: seed initial events through the reducer.
            for ev in B::initial_events(&env.state) {
                env.seq += 1;
                B::reduce(&mut env.state, &ev);
                let row = LogRow {
                    seq: env.seq,
                    event: serde_json::to_value(&ev)?,
                };
                append_line(&log_path, &serde_json::to_string(&row)?)?;
            }
        }
        let total = AtomicU64::new(env.seq);
        if recovered_events > 0 {
            tracing::info!(class, recovered_events, "object recovered from log");
        }
        Ok(Self {
            class: class.to_string(),
            dir,
            behaviour,
            env: Mutex::new(env),
            total_events: total,
        })
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join(format!("{}.jsonl", self.class))
    }

    fn snap_path(&self) -> PathBuf {
        self.dir.join(format!("{}.snap.json", self.class))
    }

    async fn dispatch(&self, command: B::Command) -> anyhow::Result<Vec<Value>> {
        let mut guard = self.env.lock().await;
        let now = now_ms();
        // Split the envelope into disjoint field borrows (the guard's Deref
        // would otherwise fuse them into one big borrow).
        let Envelope { state, kv, seq, since_snapshot } = &mut *guard;
        let mut ctx = Ctx { kv: &mut *kv, now_ms: now };
        let events = B::handle(&self.behaviour, &*state, &mut ctx, &command).await;
        drop(ctx);
        let mut out = Vec::with_capacity(events.len());
        let log_path = self.log_path();
        let mut lines = String::new();
        for ev in events {
            *seq += 1;
            *since_snapshot += 1;
            B::reduce(&mut *state, &ev);
            let j = serde_json::to_value(&ev)?;
            out.push(j.clone());
            let row = LogRow {
                seq: *seq,
                event: j,
            };
            lines.push_str(&serde_json::to_string(&row)?);
            lines.push('\n');
        }
        if !lines.is_empty() {
            append_line(&log_path, lines.trim_end())?;
        }
        self.total_events.store(*seq, Ordering::Relaxed);
        let should_snapshot = *since_snapshot >= SNAPSHOT_EVERY;
        *since_snapshot = 0;
        if should_snapshot {
            self.write_snapshot_locked(&guard)?;
        }
        Ok(out)
    }

    fn write_snapshot_locked(&self, env: &Envelope<B>) -> anyhow::Result<()> {
        let snap: SnapFile<B> = SnapFile {
            seq: env.seq,
            state: env.state.clone(),
            kv: env.kv.clone(),
        };
        let tmp = self.snap_path().with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&snap)?)?;
        std::fs::rename(&tmp, self.snap_path())?;
        // Truncate the log: everything up to seq is captured by the snapshot.
        std::fs::write(self.log_path(), b"")?;
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LogRow {
    seq: u64,
    event: Value,
}

fn append_line(path: &std::path::Path, line: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")?;
    f.flush()?;
    Ok(())
}

#[async_trait]
impl<B: Behaviour> AnyObject for ObjectAdapter<B> {
    fn class(&self) -> &str {
        &self.class
    }

    async fn send(&self, command: Value) -> anyhow::Result<Vec<Value>> {
        let cmd: B::Command = serde_json::from_value(command)?;
        self.dispatch(cmd).await
    }

    async fn due_alarms(&self) -> Vec<Value> {
        let env = self.env.lock().await;
        let now = now_ms();
        B::pending_alarms(&env.state, now)
            .into_iter()
            .filter_map(|c| serde_json::to_value(c).ok())
            .collect()
    }

    fn state_json(&self) -> Value {
        match self.env.try_lock() {
            Ok(env) => serde_json::to_value(&env.state).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        }
    }

    fn kv_json(&self) -> Value {
        match self.env.try_lock() {
            Ok(env) => serde_json::to_value(&env.kv).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        }
    }

    fn event_count(&self) -> u64 {
        self.total_events.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> anyhow::Result<()> {
        // try_lock: snapshot only when idle — the periodic path handles the rest.
        if let Ok(env) = self.env.try_lock() {
            self.write_snapshot_locked(&env)
        } else {
            Ok(())
        }
    }
}
