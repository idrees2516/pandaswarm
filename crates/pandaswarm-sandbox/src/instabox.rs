//! instabox.sh sandbox backend — full Linux cloud VMs (default 4 vCPU /
//! 8 GB) driven through the official `instabox` CLI with an `ibk_…` API key.
//!
//! Two modes:
//! * **one_shot** (default) — `instabox run --template <t> --push <dir>
//!   --timeout <s> -- bash -lc '<cmd>'`: creates a box, uploads the staged
//!   files, runs the command, deletes the box. Perfect for validation runs.
//! * **persistent** — `create` a named box (`pandaswarm-<task>`), push a
//!   tar bundle, `exec --json` commands against it, `rm --yes` on cleanup.
//!
//! The staging dir deliberately lives under `/tmp` so `push` never hits the
//! "only tracked files inside a git repo" rule.

use crate::SandboxProvider;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use pandaswarm_core::{now_ms, ExecResult, SandboxBackend, SandboxProfile};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tokio::process::Command;

pub struct InstaboxSandbox {
    api_key: String,
    template: String,
    mode: String,
    bin: PathBuf,
    /// persistent mode: box id by staging root
    boxes: Mutex<std::collections::HashMap<String, String>>,
}

impl InstaboxSandbox {
    pub fn new(cfg: &pandaswarm_core::config::InstaboxConfig) -> Self {
        let api_key = if cfg.api_key.is_empty() {
            std::env::var("INSTABOX_API_KEY").unwrap_or_default()
        } else {
            cfg.api_key.clone()
        };
        Self {
            api_key,
            template: if cfg.template.is_empty() { "ci".into() } else { cfg.template.clone() },
            mode: if cfg.mode.is_empty() { "one_shot".into() } else { cfg.mode.clone() },
            bin: resolve_bin(&cfg.bin),
            boxes: Mutex::new(Default::default()),
        }
    }

    fn base_cmd(&self) -> Command {
        let mut c = Command::new(&self.bin);
        c.env("INSTABOX_API_KEY", &self.api_key);
        c
    }

    async fn box_id(&self, name: &str) -> Result<String> {
        if let Some(id) = self.boxes.lock().unwrap().get(name) {
            return Ok(id.clone());
        }
        let out = self
            .base_cmd()
            .args(["create", "--name", name, "--template", &self.template, "--json"])
            .output()
            .await
            .context("instabox create")?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            anyhow::bail!(
                "instabox create failed ({}): {}{}",
                out.status,
                stdout.trim(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let v: Value = serde_json::from_str(stdout.trim())
            .with_context(|| format!("parse create output: {stdout}"))?;
        let id = v["machineId"]
            .as_str()
            .or_else(|| v["id"].as_str())
            .map(String::from)
            .ok_or_else(|| anyhow!("no machineId in create output"))?;
        self.boxes.lock().unwrap().insert(name.to_string(), id.clone());
        Ok(id)
    }
}

/// Find the instabox CLI: explicit config → `INSTABOX_BIN` → PATH →
/// `~/.local/bin/instabox`.
fn resolve_bin(configured: &str) -> PathBuf {
    let candidates: Vec<PathBuf> = if !configured.is_empty() {
        vec![PathBuf::from(configured)]
    } else if let Ok(v) = std::env::var("INSTABOX_BIN") {
        vec![PathBuf::from(v)]
    } else {
        vec![
            PathBuf::from("instabox"),
            dirs_home().join(".local/bin/instabox"),
            PathBuf::from("/usr/local/bin/instabox"),
        ]
    };
    for c in candidates {
        if c.exists() || which(&c) {
            return c;
        }
    }
    PathBuf::from("instabox")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/root"))
}

fn which(bin: &PathBuf) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn stage(files: &[(String, String)], tag: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("ps-stage-{tag}-{}", now_ms()));
    std::fs::create_dir_all(&dir)?;
    for (rel, content) in files {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, content)?;
    }
    Ok(dir)
}

#[async_trait]
impl SandboxProvider for InstaboxSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Instabox
    }

    fn describe(&self) -> String {
        format!(
            "instabox.sh cloud VM (template `{}`, mode `{}`) — 4 vCPU / 8 GB Linux boxes",
            self.template, self.mode
        )
    }

    async fn exec(
        &self,
        profile: &SandboxProfile,
        files: &[(String, String)],
        cmd: &str,
    ) -> Result<ExecResult> {
        if self.api_key.is_empty() {
            return Err(anyhow!("INSTABOX_API_KEY (ibk_…) not set — get one from instabox.sh"));
        }
        let timeout = profile.timeout_secs.clamp(1, 540); // run's cap is 10m
        let started = std::time::Instant::now();

        if self.mode == "persistent" {
            let name = format!("pandaswarm-{}", tag_from(profile));
            let id = self.box_id(&name).await?;

            // Push a tar bundle, then unpack + run in the box.
            let stage_dir = stage(files, &name)?;
            let bundle = std::env::temp_dir().join(format!("ps-bundle-{}.tar.gz", now_ms()));
            let cwd = stage_dir.display().to_string();
            let ok = Command::new("tar")
                .args(["-czf", &bundle.display().to_string(), "."])
                .current_dir(&stage_dir)
                .output()
                .await?
                .status
                .success();
            if !ok {
                anyhow::bail!("failed to pack sandbox bundle");
            }
            let push = self
                .base_cmd()
                .args(["push", &id, &bundle.display().to_string(), "bundle.tar.gz"])
                .output()
                .await
                .context("instabox push")?;
            if !push.status.success() {
                anyhow::bail!("instabox push failed: {}", String::from_utf8_lossy(&push.stderr));
            }
            let out = self
                .base_cmd()
                .args(["exec", &id, "--timeout", &format!("{}s", timeout), "--json", "--"])
                .arg("bash")
                .arg("-lc")
                .arg(format!(
                    "rm -rf ~/pswork && mkdir -p ~/pswork && tar -xzf ~/bundle.tar.gz -C ~/pswork && cd ~/pswork && {cmd}"
                ))
                .output()
                .await
                .context("instabox exec")?;
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let _ = std::fs::remove_file(&bundle);
            let _ = std::fs::remove_dir_all(&stage_dir);
            // exec --json prints {stdout, stderr, exitCode, signal, timedOut, truncated, durationMs}
            let v: Value = serde_json::from_str(stdout.trim()).with_context(|| format!("parse exec json: {stdout}"))?;
            Ok(ExecResult {
                stdout: v["stdout"].as_str().unwrap_or_default().into(),
                stderr: v["stderr"].as_str().unwrap_or_default().into(),
                exit_code: v["exitCode"].as_i64().unwrap_or(-1) as i32,
                timed_out: v["timedOut"].as_bool().unwrap_or(false),
                duration_ms: v["durationMs"].as_u64().unwrap_or_else(|| started.elapsed().as_millis() as u64),
                backend: "instabox".into(),
            })
        } else {
            // one_shot: create + push + run + delete in a single CLI call.
            let stage_dir = stage(files, "oneshot")?;
            let mut c = self.base_cmd();
            c.args(["run", "--template", &self.template])
                .arg("--push").arg(stage_dir.display().to_string())
                .arg("--timeout").arg(format!("{}s", timeout))
                .arg("--")
                .arg("bash").arg("-lc").arg(cmd);
            c.stdin(std::process::Stdio::null()).kill_on_drop(true);
            let mut spawned = c.spawn().context("instabox run")?;
            let output = tokio::time::timeout(Duration::from_secs((timeout + 60).into()), spawned.wait_with_output())
                .await
                .map_err(|_| anyhow!("instabox run timed out"))?
                .context("instabox run wait")?;
            let _ = std::fs::remove_dir_all(&stage_dir);
            Ok(ExecResult {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code().unwrap_or(-1),
                timed_out: false,
                duration_ms: started.elapsed().as_millis() as u64,
                backend: "instabox".into(),
            })
        }
    }

    async fn cleanup(&self, _profile: &SandboxProfile) -> Result<()> {
        let ids: Vec<String> = self.boxes.lock().unwrap().values().cloned().collect();
        for id in ids {
            let _ = self
                .base_cmd()
                .args(["rm", "--yes", &id])
                .stdin(std::process::Stdio::null())
                .output()
                .await;
        }
        self.boxes.lock().unwrap().clear();
        Ok(())
    }
}

fn tag_from(profile: &SandboxProfile) -> String {
    PathBuf::from(&profile.fs_rw)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "task".into())
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .take(24)
        .collect::<String>()
        .to_lowercase()
}
