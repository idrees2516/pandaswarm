//! # pandaswarm-rag
//!
//! Hybrid retrieval over a workspace, semble-style but dependency-light:
//!
//! * **incremental indexing** — files are re-read only when their mtime or
//!   size changed (git-ignore-ish skips: `.git`, `node_modules`, `target`…);
//! * **chunking** — line windows with overlap, each chunk remembering
//!   `path:line_start-line_end` provenance;
//! * **hybrid search** — BM25 + hashed-vector cosine, fused with RRF;
//! * **frame packs** — selected chunks gzipped into a base64 "frame" so a
//!   whole retrieval set ships inside one tool-result string (memvid-style
//!   smart frames, minus the video).

use anyhow::Result;
use base64::Engine;
use flate2::{write::GzEncoder, read::GzDecoder};
use pandaswarm_core::{hash_embed, rrf_fuse, tokenize, Bm25Index};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub text: String,
    #[serde(skip)]
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub chunk_id: String,
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub score: f64,
    pub bm25_rank: Option<usize>,
    pub vector_rank: Option<usize>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMeta {
    pub path: String,
    pub mtime_ms: u64,
    pub bytes: u64,
    pub chunks: usize,
    pub indexed_ms: u64,
}

pub struct RagIndex {
    root: PathBuf,
    chunk_lines: usize,
    overlap: usize,
    dim: usize,
    rrf_k: u64,
    chunks: Vec<Chunk>,
    bm25: Bm25Index,
    meta: std::collections::HashMap<String, IndexMeta>,
    file_count: usize,
}

const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", ".next", ".pandaswarm", "dist", "build", "__pycache__",
    ".venv", "venv", ".cache", "coverage",
];
const TEXT_EXT: &[&str] = &[
    "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "md", "json", "toml", "yaml", "yml",
    "txt", "sh", "sql", "html", "css", "go", "java", "c", "h", "cpp", "rb", "php",
];

impl RagIndex {
    pub fn new(root: impl Into<PathBuf>, chunk_lines: usize, overlap: usize, dim: usize, rrf_k: u64) -> Self {
        Self {
            root: root.into(),
            chunk_lines,
            overlap,
            dim,
            rrf_k,
            chunks: Vec::new(),
            bm25: Bm25Index::new(),
            meta: Default::default(),
            file_count: 0,
        }
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn file_count(&self) -> usize {
        self.file_count
    }

    /// (Re)index the workspace incrementally. Returns (files_scanned, files_reindexed).
    pub fn reindex(&mut self) -> Result<(usize, usize)> {
        let mut scanned = 0usize;
        let mut reindexed = 0usize;
        if !self.root.exists() {
            return Ok((0, 0));
        }
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in walkdir::WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !SKIP_DIRS.contains(&name.as_ref())
            })
        {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let ext = entry
                .path()
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !TEXT_EXT.contains(&ext.as_str()) {
                continue;
            }
            paths.push(entry.path().to_path_buf());
        }
        paths.sort();
        self.file_count = paths.len();

        let mut new_chunks = Vec::new();
        let mut new_meta = std::collections::HashMap::new();
        let mut new_bm25 = Bm25Index::new();

        for path in paths {
            scanned += 1;
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let bytes = meta.len();

            let cached = self.meta.get(&rel);
            let unchanged = cached
                .map(|m| m.mtime_ms == mtime_ms && m.bytes == bytes)
                .unwrap_or(false);
            if unchanged {
                // keep the existing chunks for this file
                new_chunks.extend(self.chunks.iter().filter(|c| c.path == rel).cloned());
                new_meta.insert(
                    rel.clone(),
                    IndexMeta {
                        path: rel.clone(),
                        mtime_ms,
                        bytes,
                        chunks: cached.map(|m| m.chunks).unwrap_or(0),
                        indexed_ms: cached.map(|m| m.indexed_ms).unwrap_or(0),
                    },
                );
                continue;
            }
            reindexed += 1;
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let lines: Vec<&str> = text.lines().collect();
            let mut chunk_count = 0usize;
            let mut start = 0usize;
            while start < lines.len() {
                let end = (start + self.chunk_lines).min(lines.len());
                let chunk_text = lines[start..end].join("\n");
                if !chunk_text.trim().is_empty() {
                    let id = format!("{rel}:{start}-{end}");
                    let vector = hash_embed(&chunk_text, self.dim);
                    new_bm25.add_doc(&id, &chunk_text);
                    chunk_count += 1;
                    new_chunks.push(Chunk {
                        id,
                        path: rel.clone(),
                        line_start: start + 1,
                        line_end: end,
                        text: chunk_text,
                        vector,
                    });
                }
                if end == lines.len() {
                    break;
                }
                start = end.saturating_sub(self.overlap);
            }
            new_meta.insert(
                rel.clone(),
                IndexMeta {
                    path: rel,
                    mtime_ms,
                    bytes,
                    chunks: chunk_count,
                    indexed_ms: pandaswarm_core::now_ms(),
                },
            );
        }

        self.chunks = new_chunks;
        self.bm25 = new_bm25;
        self.meta = new_meta;
        Ok((scanned, reindexed))
    }

    /// Add an artifact (in-memory document) to the index.
    pub fn add_document(&mut self, path: &str, content: &str) {
        let lines: Vec<&str> = content.lines().collect();
        let mut start = 0usize;
        // replace existing chunks for this path
        self.chunks.retain(|c| c.path != path);
        while start < lines.len() {
            let end = (start + self.chunk_lines).min(lines.len());
            let text = lines[start..end].join("\n");
            if !text.trim().is_empty() {
                let id = format!("{path}:{start}-{end}");
                let vector = hash_embed(&text, self.dim);
                self.bm25.add_doc(&id, &text);
                self.chunks.push(Chunk {
                    id,
                    path: path.to_string(),
                    line_start: start + 1,
                    line_end: end,
                    text,
                    vector,
                });
            }
            if end == lines.len() {
                break;
            }
            start = end.saturating_sub(self.overlap);
        }
    }

    /// Hybrid search: BM25 + vector cosine fused with RRF.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchHit> {
        let bm25_hits = self.bm25.search(query, 50);
        let qv = hash_embed(query, self.dim);

        let mut vec_hits: Vec<(String, f64)> = self
            .chunks
            .iter()
            .map(|c| (c.id.clone(), pandaswarm_core::cosine(&qv, &c.vector) as f64))
            .filter(|(_, s)| *s > 0.02)
            .collect();
        vec_hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        vec_hits.truncate(50);

        let bm25_rank: std::collections::HashMap<&str, usize> =
            bm25_hits.iter().enumerate().map(|(i, (id, _))| (id.as_str(), i + 1)).collect();
        let vec_rank: std::collections::HashMap<&str, usize> =
            vec_hits.iter().enumerate().map(|(i, (id, _))| (id.as_str(), i + 1)).collect();

        let fused = rrf_fuse(&bm25_hits, &vec_hits, self.rrf_k, top_k);
        fused
            .into_iter()
            .filter_map(|(id, score)| {
                let chunk = self.chunks.iter().find(|c| c.id == id)?;
                Some(SearchHit {
                    chunk_id: chunk.id.clone(),
                    path: chunk.path.clone(),
                    line_start: chunk.line_start,
                    line_end: chunk.line_end,
                    score,
                    bm25_rank: bm25_rank.get(id.as_str()).copied(),
                    vector_rank: vec_rank.get(id.as_str()).copied(),
                    snippet: first_lines(&chunk.text, 6),
                })
            })
            .collect()
    }
}

fn first_lines(text: &str, n: usize) -> String {
    text.lines().take(n).collect::<Vec<_>>().join("\n")
}

/// Pack retrieval results into a gzip+base64 frame for context shipping.
pub fn frame_pack(hits: &[SearchHit]) -> Result<String> {
    let mut plain = String::new();
    for h in hits {
        plain.push_str(&format!(
            "### {} (lines {}-{}, score {:.4})\n{}\n\n",
            h.path, h.line_start, h.line_end, h.score, h.snippet
        ));
    }
    let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(plain.as_bytes())?;
    let packed = enc.finish()?;
    Ok(base64::engine::general_purpose::STANDARD.encode(packed))
}

/// Unpack a frame produced by `frame_pack`.
pub fn frame_unpack(frame: &str) -> Result<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(frame.trim())?;
    let mut dec = GzDecoder::new(&raw[..]);
    let mut out = String::new();
    dec.read_to_string(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("psrag-{}", pandaswarm_core::new_id("t")));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/fib.py"),
            "\"\"\"Fibonacci sequence, iterative implementation.\"\"\"\n\n\ndef fibonacci(n):\n    a, b = 0, 1\n    for _ in range(n):\n        a, b = b, a + b\n    return a\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/notes.md"), "# Notes\nElephants never forget.\n").unwrap();
        dir
    }

    #[test]
    fn hybrid_search_finds_relevant_chunk_with_provenance() {
        let dir = temp_workspace();
        let mut ix = RagIndex::new(&dir, 40, 8, 256, 60);
        let (scanned, reindexed) = ix.reindex().unwrap();
        assert_eq!(scanned, 2);
        assert_eq!(reindexed, 2);
        let hits = ix.search("fibonacci implementation", 5);
        assert!(!hits.is_empty());
        assert!(hits[0].path.ends_with("fib.py"));
        assert_eq!(hits[0].line_start, 1);

        // Incremental: nothing changed → nothing reindexed.
        let (_, reindexed) = ix.reindex().unwrap();
        assert_eq!(reindexed, 0);
    }

    #[test]
    fn frame_pack_roundtrip() {
        let hits = vec![SearchHit {
            chunk_id: "a:1-2".into(),
            path: "a.py".into(),
            line_start: 1,
            line_end: 2,
            score: 0.5,
            bm25_rank: Some(1),
            vector_rank: Some(2),
            snippet: "print('hi')".into(),
        }];
        let frame = frame_pack(&hits).unwrap();
        let unpacked = frame_unpack(&frame).unwrap();
        assert!(unpacked.contains("a.py"));
        assert!(unpacked.contains("print"));
    }

    #[test]
    fn tokenize_helper_visible() {
        assert!(tokenize("Hello World").len() == 2);
    }
}
