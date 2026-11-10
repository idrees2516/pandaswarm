//! Resumable background jobs as pure data (reason-network-flavoured):
//! a job is a row in the swarm's state with a status and an idempotency
//! key, so a restarted process simply re-enqueues `Running` jobs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    #[default]
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// Idempotency key — retries with the same key are no-ops.
    pub idem_key: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub status: JobStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub last_error: Option<String>,
}

impl Job {
    pub fn new(kind: &str, idem_key: &str, payload: serde_json::Value) -> Self {
        let now = pandaswarm_core::now_ms();
        Self {
            id: pandaswarm_core::new_id("job"),
            idem_key: idem_key.into(),
            kind: kind.into(),
            payload,
            status: JobStatus::Queued,
            attempts: 0,
            max_attempts: 3,
            created_ms: now,
            updated_ms: now,
            last_error: None,
        }
    }
}

/// Recovery pass: anything left `Running` by a crash goes back to `Queued`
/// (unless attempts are exhausted, then `Failed`).
pub fn recover_running(jobs: &mut [Job]) -> usize {
    let mut recovered = 0;
    for j in jobs.iter_mut() {
        if j.status == JobStatus::Running {
            j.attempts += 1;
            if j.attempts >= j.max_attempts {
                j.status = JobStatus::Failed;
                j.last_error = Some("exhausted attempts after restart".into());
            } else {
                j.status = JobStatus::Queued;
                recovered += 1;
            }
            j.updated_ms = pandaswarm_core::now_ms();
        }
    }
    recovered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_jobs_requeue_on_recovery() {
        let mut a = Job::new("validate", "k1", serde_json::json!({}));
        a.status = JobStatus::Running;
        let mut b = Job::new("validate", "k2", serde_json::json!({}));
        b.status = JobStatus::Running;
        b.attempts = 2; // one attempt left (max 3)
        let mut jobs = vec![a, b];
        let n = recover_running(&mut jobs);
        assert_eq!(n, 1, "only the job with attempts left is requeued");
        assert_eq!(jobs[0].status, JobStatus::Queued);
        assert_eq!(jobs[1].status, JobStatus::Failed);
    }
}
