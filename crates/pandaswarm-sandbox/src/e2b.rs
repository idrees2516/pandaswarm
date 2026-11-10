//! E2B-compatible REST gateway backend (CubeSandbox / sandstorm / nvx style).
//!
//! These gateways differ enough in the wild that we ship a *configurable*
//! adapter rather than pretending to one spec: you point it at the three
//! endpoints (create / exec / delete) of your gateway and PandaSwarm speaks
//! plain JSON to them. Marked experimental on purpose.

use crate::SandboxProvider;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use pandaswarm_core::{now_ms, ExecResult, SandboxBackend, SandboxProfile};
use std::time::Duration;

pub struct E2bCompatSandbox {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

/// Endpoint shape your gateway must implement:
/// * POST `{base}/sandboxes`            → `{"id": "..."}`
/// * POST `{base}/sandboxes/{id}/cmds`  body `{"cmd": "...", "cwd": "."}` →
///   `{"stdout": "...", "stderr": "...", "exitCode": 0}`
/// * DELETE `{base}/sandboxes/{id}`
impl E2bCompatSandbox {
    pub fn new(api_key: &str, base_url: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("reqwest client"),
        }
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.api_key.is_empty() {
            rb
        } else {
            rb.bearer_auth(&self.api_key)
        }
    }
}

#[async_trait]
impl SandboxProvider for E2bCompatSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::E2bCompat
    }

    fn describe(&self) -> String {
        format!("E2B-compatible REST gateway at {}", self.base_url)
    }

    async fn exec(
        &self,
        profile: &SandboxProfile,
        files: &[(String, String)],
        cmd: &str,
    ) -> Result<ExecResult> {
        if self.base_url.is_empty() {
            return Err(anyhow!(
                "E2B-compatible gateway not configured (sandbox.e2b.base_url / api_key)"
            ));
        }
        let started = std::time::Instant::now();
        // 1. create sandbox
        let create: serde_json::Value = self
            .auth(self.client.post(format!("{}/sandboxes", self.base_url)).json(&serde_json::json!({
                "template": "default",
                "timeoutMs": profile.timeout_secs * 1000
            })))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let id = create["id"]
            .as_str()
            .or_else(|| create["sandboxID"].as_str())
            .ok_or_else(|| anyhow!("gateway returned no sandbox id"))?
            .to_string();

        // 2. stage files (one shell-safe heredoc per file)
        let mut script = String::new();
        for (rel, content) in files {
            let delim = format!("EOF{}", now_ms());
            script.push_str(&format!(
                "mkdir -p $(dirname {rel:?}) && cat > {rel:?} <<'{delim}'\n{content}\n{delim}\n"
            ));
        }
        script.push_str(cmd);
        let exec: serde_json::Value = self
            .auth(
                self.client
                    .post(format!("{}/sandboxes/{id}/cmds", self.base_url))
                    .json(&serde_json::json!({"cmd": script, "cwd": "."})),
            )
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // 3. delete sandbox (best effort)
        let _ = self
            .auth(self.client.delete(format!("{}/sandboxes/{id}", self.base_url)))
            .send()
            .await;

        Ok(ExecResult {
            stdout: exec["stdout"].as_str().unwrap_or_default().into(),
            stderr: exec["stderr"].as_str().unwrap_or_default().into(),
            exit_code: exec["exitCode"].as_i64().unwrap_or(-1) as i32,
            timed_out: false,
            duration_ms: started.elapsed().as_millis() as u64,
            backend: "e2b_compat".into(),
        })
    }
}
