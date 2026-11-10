// Text kernel: code-aware tokenizer, BM25, hashed embeddings ("semble-lite"),
// cosine similarity and RRF fusion — a faithful port of crates/pandaswarm-core/src/text.rs.

export function tokenize(text: string): string[] {
  const out: string[] = [];
  for (const word of text.split(/[^\p{L}\p{N}]+/u)) {
    if (!word) continue;
    // split camelCase BEFORE lowercasing
    let chunk = "";
    let prevLower = false;
    for (const ch of word) {
      if (ch === ch.toUpperCase() && ch !== ch.toLowerCase() && prevLower && chunk) {
        if (chunk.length > 1) out.push(chunk.toLowerCase());
        chunk = "";
      }
      prevLower = ch === ch.toLowerCase() || /[0-9]/.test(ch);
      chunk += ch;
    }
    if (chunk.length > 1) out.push(chunk.toLowerCase());
  }
  return out;
}

/** FNV-1a 32-bit — fast, dependency-free string hashing. */
function fnv1a(s: string, salt = 0): number {
  let h = 0x811c9dc5 ^ salt;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** Deterministic hashed embedding, L2-normalised. */
export function hashEmbed(text: string, dim = 256): Float32Array {
  const v = new Float32Array(dim);
  for (const tok of tokenize(text)) {
    for (let salt = 0; salt < 2; salt++) {
      const h = fnv1a(tok, salt);
      const idx = h % dim;
      v[idx] += h & 0x100 ? -1 : 1;
    }
  }
  let norm = 0;
  for (const x of v) norm += x * x;
  norm = Math.sqrt(norm);
  if (norm > 0) for (let i = 0; i < dim; i++) v[i] /= norm;
  return v;
}

export function cosine(a: Float32Array | number[] | null, b: Float32Array | number[] | null): number {
  if (!a || !b) return 0;
  const n = Math.min(a.length, b.length);
  let dot = 0,
    na = 0,
    nb = 0;
  for (let i = 0; i < n; i++) {
    dot += a[i] * b[i];
    na += a[i] * a[i];
    nb += b[i] * b[i];
  }
  if (na === 0 || nb === 0) return 0;
  return dot / (Math.sqrt(na) * Math.sqrt(nb));
}

/** Bytes ↔ Float32Array (for storing embeddings in SQLite BLOBs). */
export function f32ToBytes(v: Float32Array): Buffer {
  return Buffer.from(v.buffer, v.byteOffset, v.byteLength);
}

export function bytesToF32(b: Buffer): Float32Array {
  return new Float32Array(b.buffer, b.byteOffset, Math.floor(b.byteLength / 4));
}

export class Bm25Index {
  private docs = new Map<string, string[]>();
  private df = new Map<string, number>();
  private lens = new Map<string, number>();
  private avgLen = 0;

  addDoc(id: string, text: string): void {
    this.removeDoc(id);
    const toks = tokenize(text);
    const seen = new Set<string>();
    for (const t of toks) {
      if (!seen.has(t)) {
        seen.add(t);
        this.df.set(t, (this.df.get(t) ?? 0) + 1);
      }
    }
    this.lens.set(id, toks.length);
    this.docs.set(id, toks);
    this.recalc();
  }

  removeDoc(id: string): void {
    const toks = this.docs.get(id);
    if (!toks) return;
    const seen = new Set<string>();
    for (const t of toks) {
      if (!seen.has(t)) {
        seen.add(t);
        const c = (this.df.get(t) ?? 1) - 1;
        if (c <= 0) this.df.delete(t);
        else this.df.set(t, c);
      }
    }
    this.docs.delete(id);
    this.lens.delete(id);
    this.recalc();
  }

  private recalc(): void {
    let total = 0;
    for (const l of this.lens.values()) total += l;
    this.avgLen = this.docs.size ? total / this.docs.size : 0;
  }

  get size(): number {
    return this.docs.size;
  }

  search(query: string, topK = 10): Array<{ id: string; score: number }> {
    const q = tokenize(query);
    const n = this.docs.size;
    if (!n || !q.length) return [];
    const k1 = 1.2,
      b = 0.75;
    const scores = new Map<string, number>();
    for (const [id, toks] of this.docs) {
      const dl = this.lens.get(id) ?? 1;
      const norm = k1 * (1 - b + (b * dl) / Math.max(this.avgLen, 1));
      let score = 0;
      for (const term of q) {
        let tf = 0;
        for (const t of toks) if (t === term) tf++;
        if (!tf) continue;
        const df = this.df.get(term) ?? 0;
        const idf = Math.log((n - df + 0.5) / (df + 0.5) + 1);
        score += (idf * tf * (k1 + 1)) / (tf + norm);
      }
      if (score > 0) scores.set(id, score);
    }
    return [...scores.entries()]
      .map(([id, score]) => ({ id, score }))
      .sort((x, y) => y.score - x.score)
      .slice(0, topK);
  }
}

export function rrfFuse(
  a: Array<{ id: string; score: number }>,
  b: Array<{ id: string; score: number }>,
  k: number,
  topK: number
): Array<{ id: string; score: number }> {
  const scores = new Map<string, number>();
  a.forEach((hit, rank) => scores.set(hit.id, (scores.get(hit.id) ?? 0) + 1 / (k + rank + 1)));
  b.forEach((hit, rank) => scores.set(hit.id, (scores.get(hit.id) ?? 0) + 1 / (k + rank + 1)));
  return [...scores.entries()]
    .map(([id, score]) => ({ id, score }))
    .sort((x, y) => y.score - x.score)
    .slice(0, topK);
}

/** Extract the first JSON object embedded in model text. */
export function extractJson(text: string): Record<string, unknown> | null {
  for (const [open, close] of [["{", "}"], ["[", "[]"]] as const) {
    const start = text.indexOf(open);
    if (start === -1) continue;
    const end = text.lastIndexOf(close[0] ?? close);
    if (end <= start) continue;
    try {
      return JSON.parse(text.slice(start, end + 1));
    } catch {
      /* try next */
    }
  }
  return null;
}
