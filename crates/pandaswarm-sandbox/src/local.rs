//! Local sandbox: the bwrap → unshare → plain chain.
//!
//! * **bwrap** — read-only bind of `/`, the workspace bound writable at
//!   `/workspace`, fresh `/dev` `/proc` `/tmp`, no network
//!   (`--unshare-net` unless the profile allows it), `--die-with-parent`;
//! * **unshare** — `unshare --net --pid --fork --mount-proc` (needs
//!   userns or root; falls through on EPERM);
//! * **plain** — `timeout <secs> bash -lc` inside the staging dir. No
//!   isolation: dev boxes only, the UI says so loudly.

use crate::SandboxProvider;
use anyhow::Result;
use async_trait::async_trait;
use pandaswarm_core::{now_ms, ExecResult, SandboxBackend, SandboxProfile};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

pub struct LocalSandbox {
    backend: SandboxBackend,
}

impl LocalSandbox {
    pub fn new(backend: SandboxBackend) -> Self {
        Self { backend }
    }

    /// Strongest advertised layer (used when config says `auto` — the crate
    /// root's `resolve_backend` is preferred, this is the sync fallback).
    pub fn advertised() -> Self {
        Self { backend: SandboxBackend::Plain }
    }
}

fn stage_dir(_profile: &SandboxProfile, tag: &str) -> Result<PathBuf> {
    // Always stage outside any workspace: keeps run dirs out of agent
    // workspaces (and out of `push`'s git-repo file selection).
    let dir = std::env::temp_dir()
        .join("pandaswarm-sandbox")
        .join(format!("{}-{}", tag, now_ms()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn stage_files(dir: &Path, files: &[(String, String)]) -> Result<()> {
    for (rel, content) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
    }
    Ok(())
}

fn apply_env(cmd: &mut Command, profile: &SandboxProfile) {
    cmd.env_clear();
    let keep = ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "PYTHONUNBUFFERED", "NODE_OPTIONS"];
    for k in keep {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    for k in &profile.env_allowlist {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    // Some minimal default PATH in case the parent env is stripped.
    if std::env::var("PATH").is_err() {
        cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");
    }
}

async fn run_with_timeout(mut cmd: Command, timeout_secs: u64) -> Result<ExecResult> {
    use tokio::io::AsyncReadExt;
    let started = std::time::Instant::now();
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut spawned = cmd.spawn()?;
    let mut stdout_pipe = spawned.stdout.take();
    let mut stderr_pipe = spawned.stderr.take();

    let fut = async {
        let mut so = Vec::new();
        let mut se = Vec::new();
        if let Some(p) = stdout_pipe.as_mut() {
            p.read_to_end(&mut so).await?;
        }
        if let Some(p) = stderr_pipe.as_mut() {
            p.read_to_end(&mut se).await?;
        }
        let status = spawned.wait().await?;
        Ok::<_, std::io::Error>((so, se, status))
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    match tokio::time::timeout(Duration::from_secs(timeout_secs.max(1)), fut).await {
        Ok(Ok((so, se, status))) => Ok(ExecResult {
            stdout: String::from_utf8_lossy(&so).to_string(),
            stderr: String::from_utf8_lossy(&se).to_string(),
            exit_code: status.code().unwrap_or(-1),
            timed_out: false,
            duration_ms,
            backend: "local".into(),
        }),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Ok(ExecResult {
            stdout: String::new(),
            stderr: format!("command timed out after {timeout_secs}s"),
            exit_code: 124,
            timed_out: true,
            duration_ms,
            backend: "local".into(),
        }),
    }
}

#[async_trait]
impl SandboxProvider for LocalSandbox {
    fn backend(&self) -> SandboxBackend {
        self.backend
    }

    fn describe(&self) -> String {
        match self.backend {
            SandboxBackend::Bwrap => "bubblewrap jail: read-only root, writable /workspace, no network".into(),
            SandboxBackend::Unshare => "unshare --net --pid --fork --mount-proc".into(),
            _ => "plain: timeout + tempdir (no kernel isolation)".into(),
        }
    }

    async fn exec(
        &self,
        profile: &SandboxProfile,
        files: &[(String, String)],
        cmd: &str,
    ) -> Result<ExecResult> {
        let dir = stage_dir(profile, "run")?;
        stage_files(&dir, files)?;
        let timeout = profile.timeout_secs;
        let net = profile.net_enabled;
        let dir_display = dir.display().to_string();

        let mut command = match self.backend {
            SandboxBackend::Bwrap => {
                let mut c = Command::new("bwrap");
                c.arg("--ro-bind").arg("/").arg("/")
                    .arg("--bind").arg(&dir_display).arg("/workspace")
                    .arg("--dev").arg("/dev")
                    .arg("--proc").arg("/proc")
                    .arg("--tmpfs").arg("/tmp")
                    .arg("--chdir").arg("/workspace")
                    .arg("--die-with-parent")
                    .arg("--new-session")
                    .arg("--unshare-ipc").arg("--unshare-pid");
                if net {
                    // keep network (still namespaced)
                } else {
                    c.arg("--unshare-net");
                }
                c.arg("/usr/bin/timeout").arg(timeout.to_string()).arg("bash").arg("-lc").arg(cmd);
                c
            }
            SandboxBackend::Unshare => {
                let mut c = Command::new("unshare");
                c.arg("--fork").arg("--mount-proc");
                if !net {
                    c.arg("--net");
                }
                c.arg("--pid");
                c.arg("bash").arg("-lc");
                c.arg(format!("cd {dir_display:?} && /usr/bin/timeout {timeout} bash -lc {cmd:?}"));
                c
            }
            _ => {
                let mut c = Command::new("bash");
                c.arg("-lc");
                c.arg(format!("cd {dir_display:?} && /usr/bin/timeout {timeout} bash -lc {cmd:?}"));
                c
            }
        };
        apply_env(&mut command, profile);
        command.current_dir(&dir);
        let mut result = run_with_timeout(command, timeout + 5).await?;
        result.backend = format!("local:{}", self.backend.as_str());
        Ok(result)
    }
}
