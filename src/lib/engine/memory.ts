// Semantic memory: facts with hashed embeddings, cosine-deduplicated on
// write, recalled with exponential salience decay. Mirrors pandaswarm-memory.

import { db } from "@/lib/db";
import { hashEmbed, cosine, f32ToBytes, bytesToF32 } from "./text";

const DECAY_PER_DAY = 0.05;

export interface FactView {
  id: string;
  kind: string;
  content: string;
  salience: number;
  hits: number;
  createdAt: string;
  lastHitAt: string;
}

/** Remember a fact (merges near-duplicates at cosine ≥ 0.92). */
export async function rememberFact(
  content: string,
  kind = "note",
  sessionId?: string | null
): Promise<string> {
  const embed = hashEmbed(content);
  const existing = await db.fact.findMany();
  for (const f of existing) {
    const ev = bytesToF32(Buffer.from(f.embedding));
    if (ev.length === embed.length && cosine(embed, ev) >= 0.92) {
      await db.fact.update({
        where: { id: f.id },
        data: {
          salience: Math.min(2, f.salience + 0.25),
          hits: { increment: 1 },
          lastHitAt: new Date(),
        },
      });
      return f.id;
    }
  }
  const created = await db.fact.create({
    data: {
      sessionId: sessionId ?? null,
      kind,
      content,
      salience: 1,
      embedding: f32ToBytes(embed),
    },
  });
  return created.id;
}

/** Recall facts by query, decaying salience and bumping hits. */
export async function recallFacts(query: string, k = 10, minSalience = 0): Promise<FactView[]> {
  const qe = hashEmbed(query);
  const all = await db.fact.findMany();
  const now = Date.now();
  const scored = all
    .map((f) => {
      const days = (now - f.lastHitAt.getTime()) / 86_400_000;
      const decayed = f.salience * Math.exp(-DECAY_PER_DAY * days);
      const ev = bytesToF32(Buffer.from(f.embedding));
      return { f, decayed, sim: cosine(qe, ev) as number };
    })
    .filter(({ decayed }) => decayed >= minSalience)
    .map(({ f, decayed, sim }) => ({ f, decayed, score: sim * (0.5 + 0.5 * decayed) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, k);

  await Promise.all(
    scored.map(({ f, decayed }) =>
      db.fact.update({
        where: { id: f.id },
        data: { hits: { increment: 1 }, lastHitAt: new Date(), salience: decayed },
      })
    )
  );

  return scored.map(({ f, decayed }) => ({
    id: f.id,
    kind: f.kind,
    content: f.content,
    salience: decayed,
    hits: f.hits + 1,
    createdAt: f.createdAt.toISOString(),
    lastHitAt: new Date().toISOString(),
  }));
}

/** Top facts by salience (for the memory browser). */
export async function listFacts(limit = 50): Promise<FactView[]> {
  const facts = await db.fact.findMany({
    orderBy: [{ salience: "desc" }, { createdAt: "desc" }],
    take: limit,
  });
  const now = Date.now();
  return facts.map((f) => ({
    id: f.id,
    kind: f.kind,
    content: f.content,
    salience: f.salience * Math.exp((-DECAY_PER_DAY * (now - f.lastHitAt.getTime())) / 86_400_000),
    hits: f.hits,
    createdAt: f.createdAt.toISOString(),
    lastHitAt: f.lastHitAt.toISOString(),
  }));
}
