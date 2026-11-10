// Hybrid retrieval over the repository + session artifacts: BM25 + hashed
// vectors fused with RRF, incremental mtime re-index, file:line provenance.
// Mirrors crates/pandaswarm-rag.

import { readdir, readFile, stat } from "fs/promises";
import path from "path";
import { Bm25Index, hashEmbed, cosine, rrfFuse } from "./text";
import { db } from "@/lib/db";

const SKIP_DIRS = new Set([
  ".git", "node_modules", "target", ".next", ".pandaswarm", "dist", "build",
  "__pycache__", ".venv", "venv", ".cache", "coverage", "db", "download",
  "mini-services", "examples", ".zscripts", "skills",
]);
const TEXT_EXT = new Set([
  "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "md", "json", "toml",
  "yaml", "yml", "txt", "sh", "sql", "css", "prisma",
]);

// Index roots: the PandaSwarm codebase itself + session workspaces.
const ROOTS = ["crates", "docs", "src/lib", "prisma"];

interface Chunk {
  id: string;
  path: string;
  lineStart: number;
  lineEnd: number;
  text: string;
  vector: Float32Array;
}

export interface SearchHit {
  chunkId: string;
  path: string;
  lineStart: number;
  lineEnd: number;
  score: number;
  bm25Rank: number | null;
  vectorRank: number | null;
  snippet: string;
}

// Module-scoped cache (rebuilt on demand; the dev server keeps it warm).
let chunks: Chunk[] = [];
let bm25 = new Bm25Index();
let lastIndexed = 0;
let indexing = false;

async function* walk(dir: string): AsyncGenerator<string> {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const e of entries) {
    if (e.name.startsWith(".") || SKIP_DIRS.has(e.name)) continue;
    const full = path.join(dir, e.name);
    if (e.isDirectory()) yield* walk(full);
    else if (TEXT_EXT.has(path.extname(e.name).slice(1).toLowerCase())) yield full;
  }
}

/** (Re)index changed files. Returns counts. */
export async function reindex(force = false): Promise<{ files: number; chunks: number; reindexed: number }> {
  if (indexing) return { files: 0, chunks: chunks.length, reindexed: 0 };
  indexing = true;
  try {
    const known = new Map(
      (await db.ragDoc.findMany()).map((d) => [d.path, { mtime: d.mtime, bytes: d.bytes }])
    );
    const nextChunks: Chunk[] = [];
    const nextBm25 = new Bm25Index();
    const files: Array<{ path: string; mtime: number; bytes: number; chunks: number }> = [];
    let reindexed = 0;

    for (const root of ROOTS) {
      for await (const file of walk(path.join(process.cwd(), root))) {
        const rel = path.relative(process.cwd(), file);
        try {
          const st = await stat(file);
          const prev = known.get(rel);
          const unchanged = !force && prev && prev.mtime === st.mtimeMs && prev.bytes === st.size;
          if (!unchanged) reindexed++;
          const content = unchanged && chunkCache.has(rel) ? null : await readFile(file, "utf8");
          const cached = unchanged ? chunkCache.get(rel) : null;
          if (cached) {
            for (const c of cached) {
              nextChunks.push(c);
              nextBm25.addDoc(c.id, c.text);
            }
            files.push({ path: rel, mtime: st.mtimeMs, bytes: st.size, chunks: cached.length });
            continue;
          }
          const text = content ?? "";
          const lines = text.split("\n");
          const fileChunks: Chunk[] = [];
          const CHUNK_LINES = 40,
            OVERLAP = 8;
          let start = 0;
          while (start < lines.length) {
            const end = Math.min(start + CHUNK_LINES, lines.length);
            const chunkText = lines.slice(start, end).join("\n");
            if (chunkText.trim()) {
              const id = `${rel}:${start}-${end}`;
              fileChunks.push({
                id,
                path: rel,
                lineStart: start + 1,
                lineEnd: end,
                text: chunkText,
                vector: hashEmbed(chunkText),
              });
              nextBm25.addDoc(id, chunkText);
            }
            if (end === lines.length) break;
            start = end - OVERLAP;
          }
          chunkCache.set(rel, fileChunks);
          nextChunks.push(...fileChunks);
          files.push({ path: rel, mtime: st.mtimeMs, bytes: st.size, chunks: fileChunks.length });
        } catch {
          /* unreadable file — skip */
        }
      }
    }

    chunks = nextChunks;
    bm25 = nextBm25;
    lastIndexed = Date.now();

    await db.ragDoc.deleteMany({});
    if (files.length) {
      await db.ragDoc.createMany({
        data: files.map((f) => ({
          path: f.path,
          mtime: f.mtime,
          bytes: f.bytes,
          chunkCount: f.chunks,
        })),
      });
    }
    return { files: files.length, chunks: chunks.length, reindexed };
  } finally {
    indexing = false;
  }
}

const chunkCache = new Map<string, Chunk[]>();

/** Add an artifact (in-memory document) to the live index. */
export function addDocument(docPath: string, content: string): void {
  const lines = content.split("\n");
  const fileChunks: Chunk[] = [];
  const CHUNK_LINES = 40,
    OVERLAP = 8;
  let start = 0;
  while (start < lines.length) {
    const end = Math.min(start + CHUNK_LINES, lines.length);
    const text = lines.slice(start, end).join("\n");
    if (text.trim()) {
      const id = `${docPath}:${start}-${end}`;
      fileChunks.push({
        id,
        path: docPath,
        lineStart: start + 1,
        lineEnd: end,
        text,
        vector: hashEmbed(text),
      });
      bm25.addDoc(id, text);
    }
    if (end === lines.length) break;
    start = end - OVERLAP;
  }
  chunkCache.set(docPath, fileChunks);
  chunks = chunks.filter((c) => c.path !== docPath).concat(fileChunks);
}

/** Hybrid search: BM25 + vector cosine fused with RRF. */
export function search(query: string, topK = 8): SearchHit[] {
  if (!chunks.length && Date.now() - lastIndexed > 5_000) {
    // lazily trigger a reindex (fire and forget)
    void reindex();
  }
  const bm25Hits = bm25.search(query, 50);
  const qv = hashEmbed(query);
  const vecHits = chunks
    .map((c) => ({ id: c.id, score: cosine(qv, c.vector) as number }))
    .filter((h) => h.score > 0.02)
    .sort((a, b) => b.score - a.score)
    .slice(0, 50);

  const bm25Rank = new Map(bm25Hits.map((h, i) => [h.id, i + 1]));
  const vecRank = new Map(vecHits.map((h, i) => [h.id, i + 1]));
  const fused = rrfFuse(bm25Hits, vecHits, 60, topK);

  return fused
    .map(({ id, score }) => {
      const chunk = chunks.find((c) => c.id === id);
      if (!chunk) return null;
      return {
        chunkId: chunk.id,
        path: chunk.path,
        lineStart: chunk.lineStart,
        lineEnd: chunk.lineEnd,
        score,
        bm25Rank: bm25Rank.get(id) ?? null,
        vectorRank: vecRank.get(id) ?? null,
        snippet: chunk.text.split("\n").slice(0, 6).join("\n"),
      } satisfies SearchHit;
    })
    .filter((h): h is SearchHit => h !== null);
}

export function ragStatus(): { files: number; chunks: number; lastIndexed: number } {
  return { files: new Set(chunks.map((c) => c.path)).size, chunks: chunks.length, lastIndexed };
}
