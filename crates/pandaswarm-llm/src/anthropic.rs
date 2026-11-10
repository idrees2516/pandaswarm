//! Anthropic Messages API provider (content blocks + tool_use).

use crate::{ChatRequest, ChatResponse, LlmProvider, ToolCallReq, Usage};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    default_model: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(base_url: String, api_key: String, default_model: &str) -> Self {
        Self {
            base_url,
            api_key,
            default_model: default_model.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(180))
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let model = if req.model.is_empty() || req.model.starts_with("pandaswarm-mock") {
            if self.default_model.is_empty() {
                "claude-sonnet-4-5".to_string()
            } else {
                self.default_model.clone()
            }
        } else {
            req.model.clone()
        };

        let mut system = String::new();
        let mut messages = Vec::new();
        for m in &req.messages {
            match m.role {
                crate::Role::System => {
                    if !system.is_empty() {
                        system.push('\n');
                    }
                    system.push_str(&m.content);
                }
                crate::Role::User => messages.push(json!({"role": "user", "content": m.content})),
                crate::Role::Assistant => {
                    if m.tool_calls.is_empty() {
                        messages.push(json!({"role": "assistant", "content": m.content}));
                    } else {
                        let blocks: Vec<Value> = m
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.name,
                                    "input": serde_json::from_str::<Value>(&tc.arguments).unwrap_or(json!({}))
                                })
                            })
                            .collect();
                        messages.push(json!({"role": "assistant", "content": blocks}));
                    }
                }
                crate::Role::Tool => {
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                            "content": m.content
                        }]
                    }));
                }
            }
        }

        let mut body = json!({
            "model": model,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "system": system,
            "messages": messages,
        });
        if !req.tools.is_empty() {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters
                }))
                .collect::<Vec<_>>());
        }

        let url = format!("{}/v1/messages", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            anyhow::bail!(
                "anthropic returned {}: {}",
                status,
                String::from_utf8_lossy(&bytes).chars().take(500).collect::<String>()
            );
        }
        let v: Value = serde_json::from_slice(&bytes)?;
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        if let Some(blocks) = v["content"].as_array() {
            for b in blocks {
                match b["type"].as_str().unwrap_or_default() {
                    "text" => content.push_str(b["text"].as_str().unwrap_or_default()),
                    "tool_use" => tool_calls.push(ToolCallReq {
                        id: b["id"].as_str().unwrap_or_default().to_string(),
                        name: b["name"].as_str().unwrap_or_default().to_string(),
                        arguments: serde_json::to_string(&b["input"]).unwrap_or_else(|_| "{}".into()),
                    }),
                    _ => {}
                }
            }
        }
        Ok(ChatResponse {
            content,
            tool_calls,
            stop_reason: v["stop_reason"].as_str().map(|s| s.to_string()),
            usage: Usage {
                prompt_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
                completion_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
            },
            provider: "anthropic".into(),
            model,
        })
    }
}
