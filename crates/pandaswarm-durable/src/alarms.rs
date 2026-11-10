//! Alarm book-keeping for behaviours that want self-scheduled wakeups
//! (Cloudflare-DO `setAlarm` semantics, but many named alarms at once).

use serde::{Deserialize, Serialize};

/// A set of named alarms carried *inside* a behaviour's state so that it
/// survives snapshots and replays.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Alarms<C> {
    next_id: u64,
    entries: Vec<AlarmEntry<C>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlarmEntry<C> {
    pub id: u64,
    pub at_ms: u64,
    pub command: C,
}

impl<C: Clone> Alarms<C> {
    /// Schedule `command` to fire after `delay_ms`. Returns the alarm id.
    pub fn schedule(&mut self, delay_ms: u64, command: C, now_ms: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(AlarmEntry {
            id,
            at_ms: now_ms.saturating_add(delay_ms),
            command,
        });
        id
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    /// All alarms whose time has come (without removing them).
    pub fn due(&self, now_ms: u64) -> Vec<C> {
        self.entries
            .iter()
            .filter(|e| e.at_ms <= now_ms)
            .map(|e| e.command.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_and_due() {
        let mut a = Alarms::<u8>::default();
        a.schedule(100, 7, 1_000);
        assert!(a.due(1_050).is_empty());
        assert_eq!(a.due(1_100), vec![7]);
        a.cancel(1);
        assert!(a.due(1_200).is_empty());
    }
}
