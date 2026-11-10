//! `DurableRuntime` — the object host: a registry of behaviour factories, a
//! map of live objects, and a 250 ms alarm-wheel ticker that keeps every
//! object's self-scheduled wakeups firing (rivet-agents-style single-writer
//! actors, all inside one Tokio process).

use crate::object::{AnyObject, ObjectAdapter};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Factory that instantiates one durable object (usually capturing services).
pub type ObjectFactory = Arc<dyn Fn() -> anyhow::Result<Box<dyn AnyObject>> + Send + Sync>;

pub struct DurableRuntime {
    dir: PathBuf,
    factories: RwLock<HashMap<String, ObjectFactory>>,
    objects: RwLock<HashMap<String, Arc<dyn AnyObject>>>,
    stop: AtomicBool,
}

impl DurableRuntime {
    pub fn new(dir: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.into(),
            factories: RwLock::new(HashMap::new()),
            objects: RwLock::new(HashMap::new()),
            stop: AtomicBool::new(false),
        })
    }

    /// Register a typed behaviour under `class`. The factory must return a
    /// fully-wired `ObjectAdapter` (see `register_with` for the common case).
    pub async fn register_factory(&self, class: &str, factory: ObjectFactory) {
        self.factories
            .write()
            .await
            .insert(class.to_string(), factory);
    }

    /// Convenience: register `class` backed by a zero-arg constructor.
    pub async fn register<B, F>(&self, class: &str, make: F)
    where
        B: BehaviourConstructible,
        F: Fn() -> anyhow::Result<B> + Send + Sync + 'static,
    {
        let dir = self.objects_dir();
        let class_owned = class.to_string();
        let factory: ObjectFactory = Arc::new(move || {
            let b = make()?;
            Ok(Box::new(ObjectAdapter::<B>::open(
                &class_owned,
                dir.clone(),
                b,
            )?) as Box<dyn AnyObject>)
        });
        self.register_factory(class, factory).await;
    }

    fn objects_dir(&self) -> PathBuf {
        self.dir.join("objects")
    }

    /// Get (spawning or recovering if needed) the object registered as `class`.
    pub async fn get(&self, class: &str) -> anyhow::Result<Arc<dyn AnyObject>> {
        if let Some(obj) = self.objects.read().await.get(class) {
            return Ok(obj.clone());
        }
        let factory = self
            .factories
            .read()
            .await
            .get(class)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no durable object registered for class `{class}`"))?;
        let obj: Arc<dyn AnyObject> = Arc::from(factory()?);
        self.objects
            .write()
            .await
            .entry(class.to_string())
            .or_insert_with(|| obj.clone())
            .clone();
        Ok(obj)
    }

    /// Send one command (as JSON) to an object; returns the events produced.
    pub async fn send(&self, class: &str, command: Value) -> anyhow::Result<Vec<Value>> {
        let obj = self.get(class).await?;
        obj.send(command).await
    }

    /// Fire every due alarm in every live object. Returns how many commands
    /// were dispatched (the alarm-wheel heartbeat).
    pub async fn tick(&self) -> usize {
        let objects: Vec<Arc<dyn AnyObject>> = {
            let map = self.objects.read().await;
            map.values().cloned().collect()
        };
        let mut dispatched = 0;
        for obj in objects {
            for cmd in obj.due_alarms().await {
                dispatched += 1;
                if let Err(e) = obj.send(cmd).await {
                    tracing::warn!(class = obj.class(), error = %e, "alarm dispatch failed");
                }
            }
        }
        dispatched
    }

    /// Classes of all live objects.
    pub async fn classes(&self) -> Vec<String> {
        self.objects.read().await.keys().cloned().collect()
    }

    /// Snapshot every live object (graceful shutdown helper).
    pub async fn snapshot_all(&self) {
        let objects: Vec<Arc<dyn AnyObject>> = {
            let map = self.objects.read().await;
            map.values().cloned().collect()
        };
        for obj in objects {
            let _ = obj.snapshot();
        }
    }

    /// Run the 250 ms alarm ticker until `stop()`; snapshots periodically.
    pub fn start_ticker(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let rt = self.clone();
        tokio::spawn(async move {
            let mut ticks: u64 = 0;
            while !rt.stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let _ = rt.tick().await;
                ticks += 1;
                if ticks % 120 == 0 {
                    rt.snapshot_all().await;
                }
            }
            rt.snapshot_all().await;
        })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Helper so `DurableRuntime::register` can name the behaviour's trait bound
/// without leaking the associated types into the signature.
pub trait BehaviourConstructible: crate::behaviour::Behaviour + Sized + 'static {}

impl<T> BehaviourConstructible for T where T: crate::behaviour::Behaviour + Sized + 'static {}

#[cfg(test)]
mod tests {
    use crate::behaviour::{Behaviour, Ctx};
    use crate::runtime::DurableRuntime;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    // A tiny counter object: commands bump it, alarms double it, everything
    // survives a "restart" because state is rebuilt from the log.
    struct Counter;

    #[derive(Clone, Default, Serialize, Deserialize)]
    struct CounterState {
        value: i64,
        alarm_at: Option<u64>,
    }

    #[derive(Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    enum CounterEvent {
        Bumped { by: i64 },
        AlarmSet { at_ms: u64 },
        Doubled,
    }

    #[derive(Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    enum CounterCommand {
        Bump { by: i64 },
        ScheduleDouble { delay_ms: u64 },
        Double,
    }

    #[async_trait::async_trait]
    impl Behaviour for Counter {
        type State = CounterState;
        type Event = CounterEvent;
        type Command = CounterCommand;

        fn reduce(state: &mut Self::State, event: &Self::Event) {
            match event {
                CounterEvent::Bumped { by } => state.value += by,
                CounterEvent::AlarmSet { at_ms } => state.alarm_at = Some(*at_ms),
                CounterEvent::Doubled => {
                    state.value *= 2;
                    state.alarm_at = None;
                }
            }
        }

        async fn handle(
            &self,
            state: &Self::State,
            ctx: &mut Ctx<'_>,
            command: &Self::Command,
        ) -> Vec<Self::Event> {
            match command {
                CounterCommand::Bump { by } => vec![CounterEvent::Bumped { by: *by }],
                CounterCommand::ScheduleDouble { delay_ms } => {
                    let at = ctx.now_ms + delay_ms;
                    vec![CounterEvent::AlarmSet { at_ms: at }]
                }
                CounterCommand::Double => vec![CounterEvent::Doubled],
            }
        }

        fn pending_alarms(state: &Self::State, now_ms: u64) -> Vec<Self::Command> {
            match state.alarm_at {
                Some(at) if at <= now_ms => vec![CounterCommand::Double],
                _ => vec![],
            }
        }
    }

    #[tokio::test]
    async fn counter_events_persist_and_replay() {
        let dir = std::env::temp_dir().join(format!("psdur-{}", pandaswarm_core::new_id("t")));
        let rt = DurableRuntime::new(dir.clone());
        rt.register::<Counter, _>("counter", || Ok(Counter)).await;

        let events = rt
            .send("counter", json!({"type": "Bump", "by": 5}))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        let events = rt
            .send("counter", json!({"type": "Bump", "by": 3}))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);

        // "Crash": drop the runtime, recover from the same directory.
        drop(rt);
        let rt2 = DurableRuntime::new(dir.clone());
        rt2.register::<Counter, _>("counter", || Ok(Counter)).await;
        let obj = rt2.get("counter").await.unwrap();
        let state = obj.state_json();
        assert_eq!(state["value"], json!(8));

        // Alarm fires through the ticker.
        rt2.send(
            "counter",
            json!({"type": "ScheduleDouble", "delay_ms": 0}),
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let n = rt2.tick().await;
        assert!(n >= 1);
        let obj = rt2.get("counter").await.unwrap();
        let state = obj.state_json();
        assert_eq!(state["value"], json!(16));
    }
}
