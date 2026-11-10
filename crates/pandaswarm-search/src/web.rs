//! Web search (Tavily / Brave / SearXNG / DuckDuckGo-lite) and
//! fetch-to-markdown (firecrawl-flavoured, zero extra services).

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub provider: String,
}

#[derive(Debug, Clone)]
pub struct WebSearchConfig {
    pub provider: String, // tavily | brave | searxng | ddg | none
    pub tavily_key: String,
    pub brave_key: String,
    pub searxng_url: String,
}

impl WebSearchConfig {
    pub fn from_core(cfg: &pandaswarm_core::PandaSwarmConfig) -> Self {
        Self {
            provider: cfg.search.web_provider.clone(),
            tavily_key: cfg.search.tavily_key.clone(),
            brave_key: cfg.search.brave_key.clone(),
            searxng_url: cfg.search.searxng_url.clone(),
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (compatible; PandaSwarm/0.1; +https://github.com/idrees2516/pandaswarm)")
        .build()
        .expect("reqwest client")
}

/// Search the web with the first configured provider; `ddg` needs no key.
pub async fn web_search(cfg: &WebSearchConfig, query: &str, limit: usize) -> Result<Vec<WebResult>> {
    match cfg.provider.as_str() {
        "tavily" if !cfg.tavily_key.is_empty() => tavily(&cfg.tavily_key, query, limit).await,
        "brave" if !cfg.brave_key.is_empty() => brave(&cfg.brave_key, query, limit).await,
        "searxng" if !cfg.searxng_url.is_empty() => searxng(&cfg.searxng_url, query, limit).await,
        "ddg" => ddg_lite(query, limit).await,
        other => Err(anyhow!(
            "no web-search provider configured (`{other}`); set web_provider to tavily/brave/searxng/ddg and provide the key"
        )),
    }
}

async fn tavily(key: &str, query: &str, limit: usize) -> Result<Vec<WebResult>> {
    let body = serde_json::json!({
        "api_key": key,
        "query": query,
        "max_results": limit,
        "search_depth": "basic"
    });
    let v: serde_json::Value = client()
        .post("https://api.tavily.com/search")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(v["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|r| WebResult {
                    title: r["title"].as_str().unwrap_or_default().into(),
                    url: r["url"].as_str().unwrap_or_default().into(),
                    snippet: r["content"].as_str().unwrap_or_default().into(),
                    provider: "tavily".into(),
                })
                .collect()
        })
        .unwrap_or_default())
}

async fn brave(key: &str, query: &str, limit: usize) -> Result<Vec<WebResult>> {
    let v: serde_json::Value = client()
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .query(&[("q", query), ("count", &limit.to_string())])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(v["web"]["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|r| WebResult {
                    title: r["title"].as_str().unwrap_or_default().into(),
                    url: r["url"].as_str().unwrap_or_default().into(),
                    snippet: r["description"].as_str().unwrap_or_default().into(),
                    provider: "brave".into(),
                })
                .collect()
        })
        .unwrap_or_default())
}

async fn searxng(base: &str, query: &str, limit: usize) -> Result<Vec<WebResult>> {
    let url = format!("{}/search", base.trim_end_matches('/'));
    let v: serde_json::Value = client()
        .get(&url)
        .query(&[("q", query), ("format", "json")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(v["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(limit)
                .map(|r| WebResult {
                    title: r["title"].as_str().unwrap_or_default().into(),
                    url: r["url"].as_str().unwrap_or_default().into(),
                    snippet: r["content"].as_str().unwrap_or_default().into(),
                    provider: "searxng".into(),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// DuckDuckGo Lite HTML — keyless best-effort search.
async fn ddg_lite(query: &str, limit: usize) -> Result<Vec<WebResult>> {
    let html = client()
        .get("https://lite.duckduckgo.com/lite/")
        .query(&[("q", query)])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(parse_ddg_lite(&html, limit))
}

fn parse_ddg_lite(html: &str, limit: usize) -> Vec<WebResult> {
    let mut out = Vec::new();
    for chunk in html.split("<a rel=\"nofollow\" href=\"").skip(1) {
        let Some((url, rest)) = chunk.split_once('"') else { continue };
        if !url.starts_with("http") {
            continue;
        }
        let title = rest
            .split_once('>')
            .map(|(_, t)| t.split('<').next().unwrap_or_default())
            .unwrap_or_default()
            .to_string();
        if title.is_empty() {
            continue;
        }
        out.push(WebResult {
            title: decode_entities(&title),
            url: url.to_string(),
            snippet: String::new(),
            provider: "ddg".into(),
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

/// Fetch a URL and convert the HTML to readable markdown.
pub async fn fetch_markdown(url: &str) -> Result<String> {
    let resp = client().get(url).send().await?;
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = resp.text().await?;
    if content_type.contains("text/html") {
        Ok(html_to_markdown(&body))
    } else {
        Ok(body)
    }
}

/// Pragmatic HTML → markdown: drop script/style/nav noise, map headers,
/// lists, links and paragraphs. Not a full parser — enough for agent context.
pub fn html_to_markdown(html: &str) -> String {
    let mut s = html.to_string();
    // strip noise blocks
    for tag in ["script", "style", "noscript", "svg", "nav", "footer", "header"] {
        s = strip_block(&s, tag);
    }
    s = s.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n");
    s = regex_replace_all(&s, r"(?i)<h1[^>]*>", "\n\n# ");
    s = regex_replace_all(&s, r"(?i)<h2[^>]*>", "\n\n## ");
    s = regex_replace_all(&s, r"(?i)<h3[^>]*>", "\n\n### ");
    s = regex_replace_all(&s, r"(?i)<h4[^>]*>", "\n\n#### ");
    s = regex_replace_all(&s, r"(?i)<li[^>]*>", "\n- ");
    s = regex_replace_all(&s, r"(?i)<p[^>]*>", "\n\n");
    s = regex_replace_all(&s, r"(?i)</p>", "\n\n");
    s = regex_replace_all(&s, r"(?i)</tr>", "\n");
    s = regex_replace_all(&s, r"(?i)</td>|</th>", " | ");
    // links: <a href="x">text</a> -> [text](x)
    s = replace_links(&s);
    // bold/italic
    s = regex_replace_all(&s, r"(?i)<(b|strong)[^>]*>", "**");
    s = regex_replace_all(&s, r"(?i)</(b|strong)>", "**");
    s = regex_replace_all(&s, r"(?i)<(i|em)[^>]*>", "_");
    s = regex_replace_all(&s, r"(?i)</(i|em)>", "_");
    s = regex_replace_all(&s, r"(?i)<code[^>]*>", "`");
    s = regex_replace_all(&s, r"(?i)</code>", "`");
    // drop all remaining tags
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = decode_entities(&out);
    // collapse blank lines
    let mut collapsed = String::new();
    let mut blanks = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks <= 1 {
                collapsed.push('\n');
            }
        } else {
            blanks = 0;
            collapsed.push_str(line.trim_end());
            collapsed.push('\n');
        }
    }
    collapsed.trim().to_string()
}

fn strip_block(s: &str, tag: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        match lower[i..].find(&format!("<{tag}")) {
            Some(start) => {
                out.push_str(&s[i..i + start]);
                let after = i + start;
                match lower[after..].find(&format!("</{tag}>")) {
                    Some(end) => {
                        i = after + end + tag.len() + 3;
                    }
                    None => {
                        i = s.len();
                    }
                }
            }
            None => {
                out.push_str(&s[i..]);
                break;
            }
        }
    }
    out
}

fn replace_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<a ") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            out.push_str(after);
            return out;
        };
        let tag = &after[..tag_end];
        let href = tag
            .split("href=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap_or("");
        let body = &after[tag_end + 1..];
        let Some(close) = body.to_lowercase().find("</a>") else {
            out.push_str(after);
            return out;
        };
        let text = body[..close].trim();
        if href.is_empty() || text.is_empty() {
            out.push_str(text);
        } else {
            out.push_str(&format!("[{text}]({href})"));
        }
        rest = &body[close + 4..];
    }
    out.push_str(rest);
    out
}

/// Tiny regex helper: since we avoid pulling `regex` into this crate, we do
/// simple literal-prefix matching for the handful of patterns above. (The
/// patterns are all literal tags with optional attributes.)
fn regex_replace_all(s: &str, pattern: &str, replacement: &str) -> String {
    // pattern shape: (?i)<tag[^>]*> or (?i)</tag>  — parse the tag name out.
    let tag_start = match pattern.find('<') {
        Some(p) => p,
        None => return s.to_string(),
    };
    let after_lt = &pattern[tag_start + 1..];
    let closing = after_lt.starts_with('/');
    let name_src = after_lt.strip_prefix('/').unwrap_or(after_lt);
    let tag: String = name_src
        .chars()
        .skip_while(|c| !c.is_alphanumeric()) // handles "(b|strong)" alternations
        .take_while(|c| c.is_alphanumeric())
        .collect();
    if tag.is_empty() {
        return s.to_string();
    }
    let needle = format!("{}{}", if closing { "</" } else { "<" }, tag.to_lowercase());
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        match lower[i..].find(&needle) {
            Some(start) => {
                out.push_str(&s[i..i + start]);
                let after = i + start;
                match s[after..].find('>') {
                    Some(end) => {
                        out.push_str(replacement);
                        i = after + end + 1;
                    }
                    None => {
                        out.push_str(&s[after..]);
                        break;
                    }
                }
            }
            None => {
                out.push_str(&s[i..]);
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_markdown_basics() {
        let html = r#"<html><body><script>evil()</script><h1>Title</h1><p>Hello <b>world</b></p><ul><li>one</li><li>two</li></ul><a href="https://x.dev">link</a></body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("**world**"));
        assert!(md.contains("- one"));
        assert!(md.contains("[link](https://x.dev)"));
        assert!(!md.contains("evil"));
    }
}
