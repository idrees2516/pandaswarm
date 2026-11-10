//! Text utilities shared by RAG and memory: a code-aware tokenizer, a compact
//! BM25 index (custom, semble-flavoured) and a dependency-free hashed embedder
//! ("semble-lite") that turns text into L2-normalised dense vectors without a
//! GPU or an embedding API.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Tokenise for search: lowercase, split on non-alphanumerics, split
/// snake_case / camelCase / kebab-case, drop 1-char noise, keep duplicates.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        // split camelCase boundaries BEFORE lowercasing, or they vanish
        let mut chunk = String::new();
        let mut prev_lower = false;
        for ch in word.chars() {
            if ch.is_uppercase() && prev_lower && !chunk.is_empty() {
                if chunk.len() > 1 {
                    out.push(chunk.to_lowercase());
                }
                chunk.clear();
            }
            prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
            chunk.push(ch);
        }
        if chunk.len() > 1 {
            out.push(chunk.to_lowercase());
        }
    }
    out
}

/// Okapi BM25 (k1 = 1.2, b = 0.75) over string-keyed documents.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Bm25Index {
    docs: HashMap<String, Vec<String>>, // doc_id -> tokens
    df: HashMap<String, u64>,           // term -> doc frequency
    len: HashMap<String, usize>,
    avg_len: f64,
}

impl Bm25Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_doc(&mut self, doc_id: &str, text: &str) {
        self.remove_doc(doc_id);
        let toks = tokenize(text);
        let mut seen = std::collections::HashSet::new();
        for t in &toks {
            if seen.insert(t.clone()) {
                *self.df.entry(t.clone()).or_insert(0) += 1;
            }
        }
        self.len.insert(doc_id.to_string(), toks.len());
        self.docs.insert(doc_id.to_string(), toks);
        let total: usize = self.len.values().sum();
        self.avg_len = if self.docs.is_empty() {
            0.0
        } else {
            total as f64 / self.docs.len() as f64
        };
    }

    pub fn remove_doc(&mut self, doc_id: &str) {
        if let Some(toks) = self.docs.remove(doc_id) {
            let mut seen = std::collections::HashSet::new();
            for t in toks {
                if seen.insert(t.clone()) {
                    if let Some(c) = self.df.get_mut(&t) {
                        *c = c.saturating_sub(1);
                        if *c == 0 {
                            self.df.remove(&t);
                        }
                    }
                }
            }
        }
        self.len.remove(doc_id);
        let total: usize = self.len.values().sum();
        self.avg_len = if self.docs.is_empty() {
            0.0
        } else {
            total as f64 / self.docs.len() as f64
        };
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Score one query against all docs; returns (doc_id, score) sorted desc.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, f64)> {
        let q = tokenize(query);
        let n = self.docs.len() as f64;
        if n == 0.0 || q.is_empty() {
            return vec![];
        }
        let k1 = 1.2_f64;
        let b = 0.75_f64;
        let mut scores: HashMap<String, f64> = HashMap::new();
        for (doc_id, toks) in &self.docs {
            let dl = *self.len.get(doc_id).unwrap_or(&1) as f64;
            let norm = k1 * (1.0 - b + b * dl / self.avg_len.max(1.0));
            let mut score = 0.0;
            for term in &q {
                let tf = toks.iter().filter(|t| *t == term).count() as f64;
                if tf == 0.0 {
                    continue;
                }
                let df = *self.df.get(term).unwrap_or(&0) as f64;
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                score += idf * (tf * (k1 + 1.0)) / (tf + norm);
            }
            if score > 0.0 {
                scores.insert(doc_id.clone(), score);
            }
        }
        let mut ranked: Vec<(String, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(top_k);
        ranked
    }
}

/// Deterministic hashed embedding ("semble-lite"): each token hashes to two
/// dims (one +1, one with sign trick), sums are L2-normalised. Gives useful
/// cosine similarity with zero model downloads.
pub fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    for tok in tokenize(text) {
        for salt in [0u8, 1u8] {
            let mut h = Sha256::new();
            h.update([salt]);
            h.update(tok.as_bytes());
            let digest = h.finalize();
            let idx = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize % dim;
            let sign = if digest[4] & 1 == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

/// Cosine similarity for equal-length vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let dot: f32 = a[..n].iter().zip(&b[..n]).map(|(x, y)| x * y).sum();
    let na = a[..n].iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b[..n].iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Reciprocal-rank fusion of two ranked id lists.
pub fn rrf_fuse(a: &[(String, f64)], b: &[(String, f64)], k: u64, top_k: usize) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    for (rank, (id, _)) in a.iter().enumerate() {
        *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as u64 + 1) as f64;
    }
    for (rank, (id, _)) in b.iter().enumerate() {
        *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (k + rank as u64 + 1) as f64;
    }
    let mut fused: Vec<(String, f64)> = scores.into_iter().collect();
    fused.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    fused.truncate(top_k);
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_splits_code_identifiers() {
        let toks = tokenize("helloWorld foo_bar HTTPServer2 x");
        assert!(toks.contains(&"hello".to_string()));
        assert!(toks.contains(&"world".to_string()));
        assert!(toks.contains(&"foo".to_string()));
        assert!(toks.contains(&"bar".to_string()));
        assert!(toks.contains(&"httpserver2".to_string()));
    }

    #[test]
    fn bm25_ranks_relevant_doc_first() {
        let mut ix = Bm25Index::new();
        ix.add_doc("a", "fibonacci sequence implementation with memoization");
        ix.add_doc("b", "quicksort partition in place sorting algorithm");
        let hits = ix.search("fibonacci memoization", 2);
        assert_eq!(hits[0].0, "a");
    }

    #[test]
    fn hashed_embeddings_are_deterministic_and_normalized() {
        let a = hash_embed("rust durable objects sqlite", 256);
        let b = hash_embed("rust durable objects sqlite", 256);
        let c = hash_embed("completely different topic elephants", 256);
        assert_eq!(a, b);
        assert!((cosine(&a, &c)).abs() < 0.9);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }
}
