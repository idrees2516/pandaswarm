//! OpenAI-compatible chat provider (`/chat/completions`, `/embeddings`) —
//! covers OpenAI, OpenRouter, DeepSeek, GLM, Ollama, vLLM and friends.

use crate::{ChatRequest, ChatResponse, LlmProvider, ToolCallReq, Usage};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    default_model: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
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
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let model = if req.model.is_empty() {
            self.default_model.clone()
        } else {
            req.model.clone()
        };

        let mut messages = Vec::new();
        for m in &req.messages {
            let mut v = json!({"role": m.role, "content": m.content});
            if !m.tool_calls.is_empty() {
                v["tool_calls"] = json!(m
                    .tool_calls
                    .iter()
                    .map(|tc| json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {"name": tc.name, "arguments": tc.arguments}
                    }))
                    .collect::<Vec<_>>());
            }
            if let Some(id) = &m.tool_call_id {
                v["tool_call_id"] = json!(id);
            }
            messages.push(v);
        }

        let mut body = json!({
            "model": model,
            "messages": messages,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
        });
        if !req.tools.is_empty() {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": {"name": t.name, "description": t.description, "parameters": t.parameters}
                }))
                .collect::<Vec<_>>());
        }

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let bytes = resp.bytes().await.context("read body")?;
        if !status.is_success() {
            anyhow::bail!(
                "openai-compatible provider returned {}: {}",
                status,
                String::from_utf8_lossy(&bytes).chars().take(500).collect::<String>()
            );
        }
        let v: Value = serde_json::from_slice(&bytes).context("decode JSON")?;
        let choice = v
            .pointer("/choices/0")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no choices in response"))?;
        let message = &choice["message"];

        let mut tool_calls = Vec::new();
        if let Some(arr) = message["tool_calls"].as_array() {
            for tc in arr {
                let name = tc["function"]["name"].as_str().unwrap_or_default().to_string();
                let arguments = tc["function"]["arguments"].as_str().unwrap_or("{}").to_string();
                let id = tc["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| format!("call_{}", tool_calls.len()));
                if !name.is_empty() {
                    tool_calls.push(ToolCallReq { id, name, arguments });
                }
            }
        }

        Ok(ChatResponse {
            content: message["content"].as_str().unwrap_or_default().to_string(),
            tool_calls,
            stop_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
            usage: Usage {
                prompt_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                completion_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            },
            provider: "openai".into(),
            model,
        })
    }

    async fn embed(&self, texts: &[String], dim: usize) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = json!({
            "model": if self.default_model.is_empty() { "text-embedding-3-small" } else { &self.default_model },
            "input": texts,
        });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            // Graceful fallback to the local hashed embedder.
            return Ok(texts.iter().map(|t| pandaswarm_core::hash_embed(t, dim)).collect());
        }
        let v: Value = resp.json().await?;
        let mut out = Vec::new();
        if let Some(arr) = v["data"].as_array() {
            for item in arr {
                if let Some(vec) = item["embedding"].as_array() {
                    out.push(vec.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect());
                }
            }
        }
        if out.len() != texts.len() {
            return Ok(texts.iter().map(|t| pandaswarm_core::hash_embed(t, dim)).collect());
        }
        Ok(out)
    }

    async fn health(&self) -> Result<()> {
        let url = format!("{}/models", self.base_url);
        let resp = self.client.get(&url).bearer_auth(&self.api_key).send().await?;
        anyhow::ensure!(resp.status().is_success(), "provider health check failed: {}", resp.status());
        Ok(())
    }
}
