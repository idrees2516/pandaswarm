//! PandaSwarm configuration: one struct, loaded from `pandaswarm.toml` and/or
//! environment variables, consumed by every other crate.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PandaSwarmConfig {
    pub workspace_root: PathBuf,
    pub data_dir: PathBuf,
    pub llm: LlmConfig,
    pub sandbox: SandboxConfig,
    pub rag: RagConfig,
    pub search: SearchConfig,
    pub server: ServerConfig,
}

impl Default for PandaSwarmConfig {
    fn default() -> Self {
        Self {
            workspace_root: PathBuf::from(".pandaswarm/workspace"),
            data_dir: PathBuf::from(".pandaswarm/data"),
            llm: LlmConfig::default(),
            sandbox: SandboxConfig::default(),
            rag: RagConfig::default(),
            search: SearchConfig::default(),
            server: ServerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// mock | openai | anthropic | openrouter | deepseek | glm | ollama
    pub provider: String,
    pub model: String,
    pub api_key: String,
    /// Override base URL (OpenAI-compatible endpoints).
    pub base_url: String,
    /// Embedding model for providers that expose /embeddings (optional).
    pub embed_model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "mock".into(),
            model: "pandaswarm-mock-1".into(),
            api_key: String::new(),
            base_url: String::new(),
            embed_model: String::new(),
            temperature: 0.2,
            max_tokens: 4096,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// auto | bwrap | unshare | plain | instabox | e2b_compat
    pub backend: String,
    pub default_timeout_secs: u64,
    /// instabox.sh settings (see crates/pandaswarm-sandbox/src/instabox.rs).
    pub instabox: InstaboxConfig,
    /// E2B-compatible gateway settings.
    pub e2b: E2bCompatConfig,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            backend: "auto".into(),
            default_timeout_secs: 120,
            instabox: InstaboxConfig::default(),
            e2b: E2bCompatConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InstaboxConfig {
    /// `ibk_...` API key — also read from INSTABOX_API_KEY.
    pub api_key: String,
    /// Template to create boxes from (free plan: agent-browser|android|omarchy|ci).
    pub template: String,
    /// one-shot (`instabox run`, box deleted after) or persistent (create/exec/rm).
    pub mode: String,
    /// Explicit path to the `instabox` binary; empty = auto-detect.
    pub bin: String,
}

impl Default for InstaboxConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            template: "ci".into(),
            mode: "one_shot".into(),
            bin: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct E2bCompatConfig {
    pub api_key: String,
    pub base_url: String,
}

impl Default for E2bCompatConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RagConfig {
    /// Dirs (relative to CWD) indexed for retrieval.
    pub roots: Vec<String>,
    pub chunk_lines: usize,
    pub chunk_overlap: usize,
    pub vector_dim: usize,
    /// RRF k constant.
    pub rrf_k: u64,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            roots: vec![".".into()],
            chunk_lines: 40,
            chunk_overlap: 8,
            vector_dim: 256,
            rrf_k: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// tavily | brave | searxng | ddg | none
    pub web_provider: String,
    pub tavily_key: String,
    pub brave_key: String,
    pub searxng_url: String,
    /// CDP endpoint for the computer-use agent, e.g. http://127.0.0.1:9222.
    pub cdp_endpoint: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            web_provider: "none".into(),
            tavily_key: String::new(),
            brave_key: String::new(),
            searxng_url: String::new(),
            cdp_endpoint: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    /// Serve the built Next.js frontend from this dir if it exists.
    pub static_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8081".into(),
            static_dir: PathBuf::from("frontend/out"),
        }
    }
}

impl PandaSwarmConfig {
    /// Load `pandaswarm.toml` if present, apply env overrides, defaults elsewhere.
    pub fn load(path: Option<&std::path::Path>) -> anyhow::Result<Self> {
        let mut cfg = Self::default();
        if let Some(p) = path {
            if p.exists() {
                let txt = std::fs::read_to_string(p)?;
                cfg = toml_merge(&txt, cfg)?;
            }
        } else if std::path::Path::new("pandaswarm.toml").exists() {
            let txt = std::fs::read_to_string("pandaswarm.toml")?;
            cfg = toml_merge(&txt, cfg)?;
        }
        cfg.apply_env();
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("PANDASWARM_LLM_PROVIDER") {
            self.llm.provider = v;
        }
        if let Ok(v) = std::env::var("PANDASWARM_LLM_MODEL") {
            self.llm.model = v;
        }
        if let Ok(v) = std::env::var("PANDASWARM_LLM_BASE_URL") {
            self.llm.base_url = v;
        }
        if let Ok(v) = std::env::var("PANDASWARM_LLM_API_KEY") {
            self.llm.api_key = v;
        }
        if let Ok(v) = std::env::var("PANDASWARM_SANDBOX_BACKEND") {
            self.sandbox.backend = v;
        }
        if let Ok(v) = std::env::var("PANDASWARM_BIND") {
            self.server.bind = v;
        }
        if let Ok(v) = std::env::var("INSTABOX_API_KEY") {
            self.sandbox.instabox.api_key = v;
        }
        if let Ok(v) = std::env::var("INSTABOX_BIN") {
            self.sandbox.instabox.bin = v;
        }
        if let Ok(v) = std::env::var("TAVILY_API_KEY") {
            self.search.tavily_key = v;
            if self.search.web_provider == "none" {
                self.search.web_provider = "tavily".into();
            }
        }
        if let Ok(v) = std::env::var("BRAVE_API_KEY") {
            self.search.brave_key = v;
            if self.search.web_provider == "none" {
                self.search.web_provider = "brave".into();
            }
        }
    }
}

/// Minimal TOML merge: parse with a tiny hand-rolled reader and splice values
/// into the JSON representation, then deserialize. Avoids pulling in `toml`
/// (and its sizable dep tree) for one config file.
fn toml_merge(txt: &str, base: PandaSwarmConfig) -> anyhow::Result<PandaSwarmConfig> {
    let mut value = serde_json::to_value(&base)?;
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim().trim_matches('"').trim_matches('\'').to_string();
        if let Some(obj) = value.as_object_mut() {
            apply_flat(obj, key, &val);
        }
    }
    Ok(serde_json::from_value(value)?)
}

/// Apply `a.b.c = v` dotted keys against the config's JSON object.
fn apply_flat(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str, val: &str) {
    let mut parts = key.split('.').peekable();
    let mut cur = obj;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cur.insert(part.to_string(), serde_json::Value::String(val.to_string()));
        } else {
            let entry = cur
                .entry(part.to_string())
                .or_insert_with(|| serde_json::Value::Object(Default::default()));
            if !entry.is_object() {
                *entry = serde_json::Value::Object(Default::default());
            }
            cur = entry.as_object_mut().unwrap();
        }
    }
}

/// Wall-clock milliseconds since epoch (single source of truth for timestamps).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
