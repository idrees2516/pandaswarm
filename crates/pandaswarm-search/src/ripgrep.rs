//! ripgrep integration: shells out to `rg --json` when available (exact
//! pandacode-grade speed) and falls back to a pure-Rust recursive scan.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RgMatch {
    pub path: String,
    pub line_no: u64,
    pub line_text: String,
}

/// Search `dir` for `pattern` (regex). Returns up to `limit` matches.
pub async fn rg_search(pattern: &str, dir: &Path, limit: usize) -> Result<Vec<RgMatch>> {
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    if !dir.exists() {
        return Ok(vec![]);
    }
    if have_rg().await {
        if let Ok(hits) = rg_json(pattern, dir, limit).await {
            return Ok(hits);
        }
    }
    walk_search(pattern, dir, limit).await
}

async fn have_rg() -> bool {
    Command::new("rg")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn rg_json(pattern: &str, dir: &Path, limit: usize) -> Result<Vec<RgMatch>> {
    let out = Command::new("rg")
        .arg("--json")
        .arg("--max-count")
        .arg(limit.to_string())
        .arg("--no-messages")
        .arg("-S")
        .arg(pattern)
        .arg(dir)
        .output()
        .await?;
    let mut hits = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["type"] != "match" {
            continue;
        }
        let data = &v["data"];
        let path = data["path"]["text"].as_str().map(|p| p.to_string()).unwrap_or_default();
        let line_no = data["line_number"].as_u64().unwrap_or(0);
        let line_text = data["lines"]["text"].as_str().map(|s| s.trim_end().to_string()).unwrap_or_default();
        if !path.is_empty() && line_no > 0 {
            hits.push(RgMatch { path, line_no, line_text });
            if hits.len() >= limit {
                break;
            }
        }
    }
    Ok(hits)
}

async fn walk_search(pattern: &str, dir: &Path, limit: usize) -> Result<Vec<RgMatch>> {
    let regex = build_lazy_regex(pattern);
    let mut hits = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !matches!(n.as_ref(), ".git" | "node_modules" | "target" | ".next")
        })
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(text) = tokio::fs::read_to_string(entry.path()).await else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if regex(line) {
                hits.push(RgMatch {
                    path: entry.path().to_string_lossy().to_string(),
                    line_no: i as u64 + 1,
                    line_text: line.trim_end().to_string(),
                });
                if hits.len() >= limit {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

/// Case-insensitive substring or word-contains matcher (stand-in when the
/// pattern is not a real regex).
fn build_lazy_regex(pattern: &str) -> impl Fn(&str) -> bool + '_ {
    let needle = pattern.to_lowercase();
    move |line: &str| line.to_lowercase().contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finds_matches_with_or_without_rg() {
        let dir = std::env::temp_dir().join(format!("psrg-{}", pandaswarm_core::new_id("t")));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "hello pandaswarm\nnothing here\nPANDASWARM rocks\n").unwrap();
        let hits = rg_search("pandaswarm", &dir, 10).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line_no, 1);
    }
}
