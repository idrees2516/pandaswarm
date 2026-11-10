//! MCP (Model Context Protocol) stdio client: spawn a server, initialize,
//! list tools, call tools. One JSON-RPC object per line over stdin/stdout.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct McpClient {
    child: Mutex<Option<Child>>,
    stdin: Mutex<tokio::process::ChildStdin>,
    reader: Mutex<BufReader<tokio::process::ChildStdout>>,
    next_id: i64,
}

impl McpClient {
    /// Start an MCP server (e.g. `npx -y @modelcontextprotocol/server-filesystem /tmp`).
    pub async fn start(cmd: &str, args: &[String]) -> Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn MCP server `{cmd}`"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let mut client = Self {
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(stdin),
            reader: Mutex::new(BufReader::new(stdout)),
            next_id: 1,
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(format!("{}\n", req).as_bytes())
                .await
                .context("write to MCP stdin")?;
            stdin.flush().await?;
        }
        let mut reader = self.reader.lock().await;
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(std::time::Duration::from_secs(60), reader.read_line(&mut line))
                .await
                .map_err(|_| anyhow!("MCP timeout on {method}"))?
                .context("MCP stdout closed")?;
            if n == 0 {
                anyhow::bail!("MCP server closed stdout");
            }
            let v: Value = serde_json::from_str(line.trim()).context("parse MCP line")?;
            if v["id"] == json!(id) {
                if let Some(err) = v.get("error") {
                    anyhow::bail!("MCP error on {method}: {err}");
                }
                return Ok(v["result"].clone());
            }
            // notifications (e.g. logging) are skipped
        }
    }

    async fn initialize(&mut self) -> Result<()> {
        let _ = self
            .rpc(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "pandaswarm", "version": "0.1.0"}
                }),
            )
            .await?;
        // initialized notification (no response expected)
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(format!("{}\n", note).as_bytes())
            .await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        let res = self.rpc("tools/list", json!({})).await?;
        Ok(res["tools"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|t| McpTool {
                        name: t["name"].as_str().unwrap_or_default().into(),
                        description: t["description"].as_str().unwrap_or_default().into(),
                        input_schema: t.get("inputSchema").cloned().unwrap_or(json!({})),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let res = self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        Ok(res["content"].clone())
    }

    pub async fn shutdown(&mut self) {
        if let Some(mut child) = self.child.try_lock().ok().and_then(|mut g| g.take()) {
            let _ = child.kill().await;
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Best-effort kill; tokio children die with the process anyway.
        if let Ok(mut guard) = self.child.try_lock() {
            if let Some(c) = guard.as_mut() {
                c.start_kill().ok();
            }
        }
    }
}
