//! # pandaswarm-llm
//!
//! One chat/embedding interface over several providers:
//!
//! * **OpenAI-compatible** — any `/chat/completions` endpoint: OpenAI,
//!   OpenRouter, DeepSeek, GLM, Ollama, vLLM…
//! * **Anthropic** — `/v1/messages` with tool-use blocks.
//! * **Mock** — fully deterministic, zero-network provider that powers
//!   `pandaswarm demo`, CI and the offline test-suite: it recognises the
//!   structured prompts the orchestrator emits and answers with real plans
//!   and real (working) code.

pub mod anthropic;
pub mod mock;
pub mod openai;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// A tool exposed to the model (OpenAI function-calling shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value, // JSON schema
}

impl ToolSpec {
    pub fn new(name: &str, description: &str, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// For `role == "tool"`: which tool call this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool call requested by the assistant (OpenAI shape).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallReq>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into(), tool_call_id: None, tool_calls: vec![] }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into(), tool_call_id: None, tool_calls: vec![] }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into(), tool_call_id: None, tool_calls: vec![] }
    }
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallReq {
    pub id: String,
    pub name: String,
    /// JSON-encoded arguments.
    pub arguments: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallReq>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
    pub provider: String,
    pub model: String,
}

impl ChatResponse {
    pub fn wants_tool(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub tools: Vec<ToolSpec>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature: 0.2,
            max_tokens: 4096,
            tools: vec![],
        }
    }
}

/// The provider contract.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse>;
    /// Dense embeddings; falls back to `pandaswarm_core::hash_embed` when the
    /// provider has no embedding endpoint.
    async fn embed(&self, texts: &[String], dim: usize) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| pandaswarm_core::hash_embed(t, dim)).collect())
    }
    /// Cheap probe used by `pandaswarm doctor`.
    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

/// Build a provider from config (provider name + key + base URL + model).
pub fn build_provider(cfg: &pandaswarm_core::PandaSwarmConfig) -> Result<Arc<dyn LlmProvider>> {
    let llm = &cfg.llm;
    match llm.provider.as_str() {
        "mock" | "" => Ok(Arc::new(mock::MockProvider::new(&llm.model))),
        "openai" => Ok(Arc::new(openai::OpenAiProvider::new(
            base_or(&llm.base_url, "https://api.openai.com/v1"),
            non_empty(&llm.api_key).ok_or_else(|| anyhow!("OPENAI/PANDASWARM_LLM_API_KEY not set"))?,
            &llm.model,
        ))),
        "openrouter" => Ok(Arc::new(openai::OpenAiProvider::new(
            base_or(&llm.base_url, "https://openrouter.ai/api/v1"),
            non_empty(&llm.api_key).ok_or_else(|| anyhow!("API key not set for openrouter"))?,
            &llm.model,
        ))),
        "deepseek" => Ok(Arc::new(openai::OpenAiProvider::new(
            base_or(&llm.base_url, "https://api.deepseek.com/v1"),
            non_empty(&llm.api_key).ok_or_else(|| anyhow!("API key not set for deepseek"))?,
            &llm.model,
        ))),
        "glm" => Ok(Arc::new(openai::OpenAiProvider::new(
            base_or(&llm.base_url, "https://open.bigmodel.cn/api/paas/v4"),
            non_empty(&llm.api_key).ok_or_else(|| anyhow!("API key not set for glm"))?,
            &llm.model,
        ))),
        "ollama" => Ok(Arc::new(openai::OpenAiProvider::new(
            base_or(&llm.base_url, "http://127.0.0.1:11434/v1"),
            non_empty(&llm.api_key).unwrap_or_else(|| "ollama".into()),
            &llm.model,
        ))),
        "anthropic" => Ok(Arc::new(anthropic::AnthropicProvider::new(
            base_or(&llm.base_url, "https://api.anthropic.com"),
            non_empty(&llm.api_key).ok_or_else(|| anyhow!("API key not set for anthropic"))?,
            &llm.model,
        ))),
        other => Err(anyhow!("unknown LLM provider `{other}`")),
    }
}

fn non_empty(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Configured base URL or the provider default.
fn base_or(cfg_val: &str, default: &str) -> String {
    if cfg_val.trim().is_empty() {
        default.to_string()
    } else {
        cfg_val.trim().trim_end_matches('/').to_string()
    }
}

/// Standard tool set exposed to coding agents.
pub fn coding_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec::new(
            "write_file",
            "Write a file into your workspace",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        ),
        ToolSpec::new(
            "read_file",
            "Read a file from your workspace",
            json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        ),
        ToolSpec::new(
            "sandbox_run",
            "Run a shell command inside the sandbox to validate code",
            json!({
                "type": "object",
                "properties": {
                    "cmd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "default": 60}
                },
                "required": ["cmd"]
            }),
        ),
        ToolSpec::new(
            "rag_search",
            "Hybrid-search indexed code and past artifacts (BM25 + vector + RRF)",
            json!({"type": "object", "properties": {"query": {"type": "string"}, "top_k": {"type": "integer"}}, "required": ["query"]}),
        ),
        ToolSpec::new(
            "web_search",
            "Search the web (requires a configured search provider)",
            json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
        ),
    ]
}

/// Extract the first JSON object/array embedded in model text (used by the
/// planner to pull structured plans out of chatty responses).
pub fn extract_json(text: &str) -> Option<Value> {
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let Some(start) = text.find(open) {
            if let Some(end) = text[start..].rfind(close) {
                if let Ok(v) = serde_json::from_str::<Value>(&text[start..=start + end]) {
                    return Some(v);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_embedded_json() {
        let text = "Here is the plan:\n{\"milestones\": [1,2]}\nhope that helps";
        let v = extract_json(text).unwrap();
        assert_eq!(v["milestones"].as_array().unwrap().len(), 2);
    }
}
