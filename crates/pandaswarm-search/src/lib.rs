//! # pandaswarm-search
//!
//! The agent tool belt:
//! * `ripgrep` — `rg --json` wrapper (with a pure-Rust walk fallback);
//! * `web` — Tavily / Brave / SearXNG / DuckDuckGo-lite + fetch-to-markdown;
//! * `computer` — a CDP (Chrome DevTools Protocol) computer-use client for
//!   lightpanda / headless-chromium endpoints;
//! * `mcp` — a Model Context Protocol stdio client (initialize → tools/list
//!   → tools/call over newline-delimited JSON-RPC).

pub mod computer;
pub mod mcp;
pub mod ripgrep;
pub mod web;

pub use ripgrep::{rg_search, RgMatch};
pub use web::{fetch_markdown, web_search, WebResult};
