//! # pandaswarm-memory
//!
//! obelisk-flavoured agent memory on SQLite:
//!
//! * **episodic** — sessions, messages, tool calls, artifacts;
//! * **semantic** — `facts` with a hashed embedding, cosine-deduplicated on
//!   write and recalled with an exponential salience decay (frequently
//!   re-hit facts stay alive, stale ones fade);
//! * **retrieval** — `search_history` (FTS5 when available, LIKE fallback)
//!   and `recall` for the coordinator's context assembly.

use anyhow::{Context, Result};
use pandaswarm_core::{cosine, hash_embed, new_id, prefix, now_ms};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub struct MemoryStore {
    conn: Mutex<Connection>,
    dim: usize,
}

const DECAY_LAMBDA_PER_DAY: f64 = 0.05;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub mode: String,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactRow {
    pub id: String,
    pub session_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub salience: f64,
    pub hits: u64,
    pub created_ms: u64,
    pub last_hit_ms: u64,
}

impl MemoryStore {
    pub fn open(path: impl AsRef<Path>, vector_dim: usize) -> Result<Self> {
        let conn = if path.as_ref().as_os_str().is_empty() || path.as_ref() == Path::new(":memory:") {
            Connection::open_in_memory()
        } else {
            if let Some(parent) = path.as_ref().parent() {
                std::fs::create_dir_all(parent).ok();
            }
            Connection::open(path)
        }
        .context("open sqlite memory store")?;
        let store = Self {
            conn: Mutex::new(conn),
            dim: vector_dim,
        };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                mode TEXT NOT NULL DEFAULT 'task',
                created_ms INTEGER NOT NULL,
                updated_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, created_ms);
            CREATE TABLE IF NOT EXISTS tool_calls (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL DEFAULT '',
                tool TEXT NOT NULL,
                input TEXT NOT NULL DEFAULT '{}',
                output TEXT NOT NULL DEFAULT '{}',
                created_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS artifacts (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL DEFAULT '',
                path TEXT NOT NULL,
                language TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL DEFAULT '',
                hash TEXT NOT NULL,
                created_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_artifacts_session ON artifacts(session_id);
            CREATE TABLE IF NOT EXISTS facts (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                kind TEXT NOT NULL DEFAULT 'note',
                content TEXT NOT NULL,
                salience REAL NOT NULL DEFAULT 1.0,
                hits INTEGER NOT NULL DEFAULT 0,
                embedding BLOB NOT NULL,
                created_ms INTEGER NOT NULL,
                last_hit_ms INTEGER NOT NULL
            );
            "#,
        )?;
        // FTS5 when the bundled SQLite has it; history search falls back to LIKE.
        let _ = conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(content, content='messages', content_rowid='rowid');",
        );
        Ok(())
    }

    // ---- episodic ----

    pub fn create_session(&self, id: &str, title: &str, mode: &str) -> Result<SessionRow> {
        let now = now_ms();
        let row = SessionRow {
            id: id.to_string(),
            title: title.into(),
            mode: mode.into(),
            created_ms: now,
            updated_ms: now,
        };
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions (id, title, mode, created_ms, updated_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![row.id, row.title, row.mode, row.created_ms as i64, row.updated_ms as i64],
        )?;
        Ok(row)
    }

    pub fn touch_session(&self, id: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE sessions SET updated_ms = ?1 WHERE id = ?2",
            params![now_ms() as i64, id],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self, limit: u64) -> Result<Vec<SessionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, mode, created_ms, updated_ms FROM sessions ORDER BY updated_ms DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(SessionRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    mode: r.get(2)?,
                    created_ms: r.get::<_, i64>(3)? as u64,
                    updated_ms: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_message(&self, session_id: &str, role: &str, content: &str) -> Result<MessageRow> {
        let row = MessageRow {
            id: new_id("msg"),
            session_id: session_id.into(),
            role: role.into(),
            content: content.into(),
            created_ms: now_ms(),
        };
        self.conn.lock().unwrap().execute(
            "INSERT INTO messages (id, session_id, role, content, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![row.id, row.session_id, row.role, row.content, row.created_ms as i64],
        )?;
        self.touch_session(session_id)?;
        Ok(row)
    }

    pub fn list_messages(&self, session_id: &str) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, content, created_ms FROM messages WHERE session_id = ?1 ORDER BY created_ms ASC",
        )?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    created_ms: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_tool_call(
        &self,
        session_id: &str,
        agent_id: &str,
        tool: &str,
        input: &serde_json::Value,
        output: &serde_json::Value,
    ) -> Result<String> {
        let id = new_id("tcall");
        self.conn.lock().unwrap().execute(
            "INSERT INTO tool_calls (id, session_id, agent_id, tool, input, output, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                session_id,
                agent_id,
                tool,
                serde_json::to_string(input)?,
                serde_json::to_string(output)?,
                now_ms() as i64
            ],
        )?;
        Ok(id)
    }

    pub fn upsert_artifact(
        &self,
        session_id: &str,
        agent_id: &str,
        path: &str,
        language: &str,
        content: &str,
    ) -> Result<String> {
        let hash = format!("{:x}", Sha256Digest::of(content));
        let conn = self.conn.lock().unwrap();
        // Same path + same content => no-op; same path + new content => new version.
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM artifacts WHERE session_id = ?1 AND path = ?2 AND hash = ?3 ORDER BY created_ms DESC LIMIT 1",
                params![session_id, path, hash],
                |r| r.get(0),
            )
            .ok();
        if let Some(id) = existing {
            return Ok(id);
        }
        let id = new_id(prefix::ARTIFACT);
        conn.execute(
            "INSERT INTO artifacts (id, session_id, agent_id, path, language, content, hash, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, session_id, agent_id, path, language, content, hash, now_ms() as i64],
        )?;
        drop(conn);
        Ok(id)
    }

    pub fn list_artifacts(&self, session_id: &str) -> Result<Vec<(String, String, String, u64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, path, language, created_ms FROM artifacts WHERE session_id = ?1 ORDER BY created_ms ASC",
        )?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? as u64))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn artifact_content(&self, artifact_id: &str) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT path, content FROM artifacts WHERE id = ?1",
                params![artifact_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok())
    }

    // ---- semantic facts ----

    /// Remember a fact: cosine-deduplicated against existing facts (≥ 0.92
    /// merges by bumping salience + hits), otherwise inserted fresh.
    pub fn remember_fact(
        &self,
        session_id: Option<&str>,
        kind: &str,
        content: &str,
    ) -> Result<String> {
        let embed = hash_embed(content, self.dim);
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, embedding, salience FROM facts")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (id, blob, salience) in rows {
            if blob.len() == embed.len() * 4 {
                let existing = bytes_to_f32(&blob);
                if cosine(&embed, &existing) >= 0.92 {
                    conn.execute(
                        "UPDATE facts SET salience = ?1, hits = hits + 1, last_hit_ms = ?2 WHERE id = ?3",
                        params![(salience + 0.25).min(2.0), now as i64, id],
                    )?;
                    return Ok(id);
                }
            }
        }
        let id = new_id(prefix::FACT);
        conn.execute(
            "INSERT INTO facts (id, session_id, kind, content, salience, hits, embedding, created_ms, last_hit_ms) VALUES (?1, ?2, ?3, ?4, 1.0, 0, ?5, ?6, ?6)",
            params![id, session_id, kind, content, f32_to_bytes(&embed), now as i64],
        )?;
        Ok(id)
    }

    /// Recall facts by query embedding, applying salience decay and bumping
    /// hit counters for anything returned.
    pub fn recall(&self, query: &str, k: usize, min_salience: f64) -> Result<Vec<FactRow>> {
        let qe = hash_embed(query, self.dim);
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, session_id, kind, content, salience, hits, embedding, created_ms, last_hit_ms FROM facts")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, i64>(5)? as u64,
                    r.get::<_, Vec<u8>>(6)?,
                    r.get::<_, i64>(7)? as u64,
                    r.get::<_, i64>(8)? as u64,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut scored: Vec<(f64, FactRow)> = rows
            .into_iter()
            .filter_map(|(id, sess, kind, content, salience, hits, blob, created, last_hit)| {
                if blob.len() != qe.len() * 4 {
                    return None;
                }
                let days = (now.saturating_sub(last_hit)) as f64 / 86_400_000.0;
                let decayed = salience * (-DECAY_LAMBDA_PER_DAY * days).exp();
                if decayed < min_salience {
                    return None;
                }
                let sim = cosine(&qe, &bytes_to_f32(&blob)) as f64;
                let score = sim * (0.5 + 0.5 * decayed);
                Some((
                    score,
                    FactRow {
                        id,
                        session_id: sess,
                        kind,
                        content,
                        salience: decayed,
                        hits,
                        created_ms: created,
                        last_hit_ms: last_hit,
                    },
                ))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        let mut out = Vec::new();
        for (_, mut fact) in scored {
            fact.hits += 1;
            conn.execute(
                "UPDATE facts SET hits = ?1, last_hit_ms = ?2, salience = ?3 WHERE id = ?4",
                params![fact.hits as i64, now as i64, fact.salience, fact.id],
            )?;
            out.push(fact);
        }
        Ok(out)
    }

    pub fn list_facts(&self, limit: u64) -> Result<Vec<FactRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, kind, content, salience, hits, created_ms, last_hit_ms FROM facts ORDER BY salience DESC, created_ms DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(FactRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    kind: r.get(2)?,
                    content: r.get(3)?,
                    salience: r.get(4)?,
                    hits: r.get::<_, i64>(5)? as u64,
                    created_ms: r.get::<_, i64>(6)? as u64,
                    last_hit_ms: r.get::<_, i64>(7)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- history search ----

    /// Search message history. Tries FTS5 match syntax first, falls back to
    /// LIKE for plain substrings.
    pub fn search_history(&self, session_id: Option<&str>, query: &str, limit: u64) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock().unwrap();
        let like = format!("%{}%", query.replace('%', ""));
        let limit_i = limit.to_string();
        let sql = match session_id {
            Some(_) => "SELECT id, session_id, role, content, created_ms FROM messages WHERE session_id = ?1 AND (content LIKE ?2) ORDER BY created_ms DESC LIMIT ?3",
            None => "SELECT id, session_id, role, content, created_ms FROM messages WHERE content LIKE ?1 ORDER BY created_ms DESC LIMIT ?2",
        };
        let mut prm: Vec<String> = Vec::new();
        if let Some(sid) = session_id {
            prm.push(sid.to_string());
        }
        prm.push(like);
        prm.push(limit_i);
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(prm.iter()), |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    created_ms: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn f32_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

struct Sha256Digest(Vec<u8>);
impl Sha256Digest {
    fn of(content: &str) -> Self {
        use sha2::Digest;
        Self(sha2::Sha256::digest(content.as_bytes()).to_vec())
    }
}
impl std::fmt::LowerHex for Sha256Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_dedupe_and_decay() {
        let store = MemoryStore::open(":memory:", 256).unwrap();
        let a = store.remember_fact(None, "note", "The build uses bwrap sandboxing").unwrap();
        let b = store.remember_fact(None, "note", "The build uses bwrap sandboxing!").unwrap();
        assert_eq!(a, b, "near-duplicate facts merge");
        store.remember_fact(None, "note", "Rust backend talks axum REST").unwrap();
        let hits = store.recall("sandbox build isolation", 5, 0.0).unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|f| f.content.contains("bwrap")));
    }
}
