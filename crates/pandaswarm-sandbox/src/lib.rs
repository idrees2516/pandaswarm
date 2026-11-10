//! # pandaswarm-sandbox
//!
//! Layered execution for agent-written code (nullclaw/sandstorm-inspired):
//!
//! * **local chain** — `bwrap` (ro-root bind + no-net) → `unshare --net
//!   --pid` → plain `timeout` in a temp dir, auto-detected and cached;
//! * **instabox.sh** — full Linux cloud VMs (4 vCPU / 8 GB) driven through
//!   the official CLI with `INSTABOX_API_KEY`;
//! * **E2B-compatible gateways** — CubeSandbox/sandstorm-style REST;
//! * **harness** — turns raw sandbox output into a structured `TestReport`
//!   and proposes harness patches for recurring failure modes (HarnessPro).

pub mod e2b;
pub mod harness;
pub mod instabox;
pub mod local;

use anyhow::Result;
use async_trait::async_trait;
use pandaswarm_core::{ExecResult, SandboxBackend, SandboxProfile};

pub use harness::{parse_test_report, suggest_harness_patches, HarnessSuggestion};

/// A sandbox provider executes a shell command under a `SandboxProfile`,
/// after staging `files` (relative path → content) into the workdir.
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    fn backend(&self) -> SandboxBackend;
    fn describe(&self) -> String;
    async fn exec(
        &self,
        profile: &SandboxProfile,
        files: &[(String, String)],
        cmd: &str,
    ) -> Result<ExecResult>;
    /// Best-effort cleanup (e.g. delete the cloud box).
    async fn cleanup(&self, _profile: &SandboxProfile) -> Result<()> {
        Ok(())
    }
}

/// Detect which local isolation layers are usable right now (sync: called
/// once at startup — cheap process probes).
pub fn detect_local_layers() -> Vec<SandboxBackend> {
    let mut out = Vec::new();
    // Probe with real isolation, not --version: `unshare --pid` needs
    // privileges and bwrap needs setuid/userns.
    let bwrap_ok = std::process::Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if bwrap_ok {
        out.push(SandboxBackend::Bwrap);
    }
    let unshare_ok = std::process::Command::new("unshare")
        .args(["--net", "--pid", "--fork", "--mount-proc", "--", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if unshare_ok {
        out.push(SandboxBackend::Unshare);
    }
    out.push(SandboxBackend::Plain);
    out
}

/// Resolve the configured backend string against what's actually available.
pub fn resolve_backend(want: &str) -> SandboxBackend {
    match want {
        "bwrap" | "unshare" | "plain" | "instabox" | "e2b_compat" => {
            serde_json::from_value::<SandboxBackend>(serde_json::json!(want)).unwrap_or(SandboxBackend::Plain)
        }
        _ => {
            // auto: strongest local layer available
            detect_local_layers().first().cloned().unwrap_or(SandboxBackend::Plain)
        }
    }
}
