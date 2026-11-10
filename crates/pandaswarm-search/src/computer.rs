//! Computer-use agent over the Chrome DevTools Protocol — works with
//! lightpanda, headless chromium or any CDP-compatible endpoint.
//!
//! Flow: screenshot → (vision LLM decides) → click / type / scroll / wait →
//! screenshot … until `done`. This module is the *hands*; the orchestrator
//! supplies the *eyes* by routing screenshots through the configured LLM.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ComputerAction {
    Navigate { url: String },
    Click { x: i64, y: i64 },
    Type { text: String },
    Press { key: String },
    Scroll { direction: String, amount: i64 },
    Wait { ms: u64 },
    Screenshot,
    Done { summary: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub done: bool,
    pub description: String,
    /// Base64 PNG of the page after the action.
    pub screenshot_b64: Option<String>,
}

pub struct ComputerUse {
    http: reqwest::Client,
    endpoint: String, // e.g. http://127.0.0.1:9222
    ws_url: String,
    ws: Option<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>,
    next_id: i64,
}

impl ComputerUse {
    /// Connect to a CDP endpoint, preferring an existing page target.
    pub async fn connect(endpoint: &str) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
        let base = endpoint.trim_end_matches('/').to_string();
        // /json/list → pick the first page-type target, else /json/new
        let list: Value = http
            .get(format!("{base}/json/list"))
            .send()
            .await
            .with_context(|| format!("GET {base}/json/list"))?
            .json()
            .await
            .map_err(|e| anyhow!("endpoint is not CDP-compatible: {e}"))?;
        let ws_url = list
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|t| t["type"] == "page")
                    .and_then(|t| t["webSocketDebuggerUrl"].as_str().map(String::from))
            })
            .or_else(|| {
                list.as_array()
                    .and_then(|arr| arr.first())
                    .and_then(|t| t["webSocketDebuggerUrl"].as_str().map(String::from))
            })
            .ok_or_else(|| anyhow!("no page target with webSocketDebuggerUrl"))?;

        let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .with_context(|| format!("connect ws {ws_url}"))?;
        Ok(Self {
            http,
            endpoint: base,
            ws_url,
            ws: Some(ws),
            next_id: 1,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn cdp(&mut self, method: &str, params: Value) -> Result<Value> {
        let ws = self
            .ws
            .as_mut()
            .ok_or_else(|| anyhow!("websocket closed"))?;
        let id = self.next_id;
        self.next_id += 1;
        ws.send(WsMessage::Text(
            json!({"id": id, "method": method, "params": params}).to_string().into(),
        ))
        .await?;
        // Read frames until the matching response id arrives.
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(20), ws.next())
                .await
                .map_err(|_| anyhow!("CDP timeout waiting for response to {method}"))?
                .ok_or_else(|| anyhow!("CDP socket closed"))??;
            if let WsMessage::Text(txt) = msg {
                let v: Value = serde_json::from_str(&txt)?;
                if v["id"] == json!(id) {
                    if let Some(err) = v.get("error") {
                        anyhow::bail!("CDP error on {method}: {err}");
                    }
                    return Ok(v["result"].clone());
                }
                // events in between are ignored for simplicity
            }
        }
    }

    async fn screenshot(&mut self) -> Result<String> {
        self.cdp("Page.enable", json!({})).await.ok();
        let res = self
            .cdp("Page.captureScreenshot", json!({"format": "png"}))
            .await?;
        res["data"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow!("no screenshot data"))
    }

    /// Execute one action; returns the post-action screenshot when relevant.
    pub async fn act(&mut self, action: &ComputerAction) -> Result<ActionResult> {
        match action {
            ComputerAction::Navigate { url } => {
                self.cdp("Page.enable", json!({})).await.ok();
                self.cdp("Page.navigate", json!({"url": url})).await?;
                tokio::time::sleep(Duration::from_millis(700)).await;
                Ok(ActionResult {
                    done: false,
                    description: format!("navigated to {url}"),
                    screenshot_b64: Some(self.screenshot().await?),
                })
            }
            ComputerAction::Click { x, y } => {
                for typ in ["mousePressed", "mouseReleased"] {
                    self.cdp(
                        "Input.dispatchMouseEvent",
                        json!({"type": typ, "x": x, "y": y, "button": "left", "clickCount": 1}),
                    )
                    .await?;
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
                Ok(ActionResult {
                    done: false,
                    description: format!("clicked at ({x}, {y})"),
                    screenshot_b64: Some(self.screenshot().await?),
                })
            }
            ComputerAction::Type { text } => {
                self.cdp("Input.insertText", json!({"text": text})).await?;
                tokio::time::sleep(Duration::from_millis(250)).await;
                Ok(ActionResult {
                    done: false,
                    description: format!("typed {text:?}"),
                    screenshot_b64: Some(self.screenshot().await?),
                })
            }
            ComputerAction::Press { key } => {
                let (key_code, code, text) = key_info(key);
                self.cdp(
                    "Input.dispatchKeyEvent",
                    json!({"type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": key_code, "text": text}),
                )
                .await?;
                self.cdp(
                    "Input.dispatchKeyEvent",
                    json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": key_code}),
                )
                .await?;
                Ok(ActionResult { done: false, description: format!("pressed {key}"), screenshot_b64: None })
            }
            ComputerAction::Scroll { direction, amount } => {
                let dy = if direction == "up" { -amount } else { *amount };
                self.cdp(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": 400, "y": 400, "deltaX": 0, "deltaY": dy}),
                )
                .await?;
                tokio::time::sleep(Duration::from_millis(300)).await;
                Ok(ActionResult {
                    done: false,
                    description: format!("scrolled {direction} by {amount}"),
                    screenshot_b64: Some(self.screenshot().await?),
                })
            }
            ComputerAction::Wait { ms } => {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
                Ok(ActionResult {
                    done: false,
                    description: format!("waited {ms}ms"),
                    screenshot_b64: Some(self.screenshot().await?),
                })
            }
            ComputerAction::Screenshot => Ok(ActionResult {
                done: false,
                description: "screenshot".into(),
                screenshot_b64: Some(self.screenshot().await?),
            }),
            ComputerAction::Done { summary } => Ok(ActionResult {
                done: true,
                description: summary.clone(),
                screenshot_b64: None,
            }),
        }
    }

    /// Parse the current page's visible text (for LLM grounding).
    pub async fn page_text(&mut self) -> Result<String> {
        let res = self
            .cdp("Runtime.evaluate", json!({"expression": "document.body.innerText", "returnByValue": true}))
            .await?;
        Ok(res["result"]["value"].as_str().unwrap_or_default().to_string())
    }

    pub fn screenshot_bytes(b64: &str) -> Result<Vec<u8>> {
        Ok(base64::engine::general_purpose::STANDARD.decode(b64)?)
    }
}

fn key_info(key: &str) -> (u32, &str, &str) {
    match key {
        "Enter" => (13, "Enter", "\r"),
        "Tab" => (9, "Tab", "\t"),
        "Escape" => (27, "Escape", ""),
        "Backspace" => (8, "Backspace", "\u{8}"),
        other => (other.chars().next().map(|c| c as u32).unwrap_or(0), other, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_bytes_roundtrip() {
        let b = ComputerUse::screenshot_bytes(&base64::engine::general_purpose::STANDARD.encode(b"pngdata")).unwrap();
        assert_eq!(b, b"pngdata");
    }
}
