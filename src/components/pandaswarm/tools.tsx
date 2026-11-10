"use client";

import { useCallback, useEffect, useState } from "react";
import { Brain, Database, Loader2, Plus, RefreshCw, Save, Search } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { api } from "./shared";

// ─── Sandbox ────────────────────────────────────────────────────────────────

interface SandboxResult {
  result: { stdout: string; stderr: string; exitCode: number; durationMs: number; timedOut: boolean; backend: string };
  report: { passed: number; failed: number; errors: number; skipped: number; failurePatterns: string[] };
  suggestions: Array<{ pattern: string; suggestion: string; priority: number }>;
}

export function SandboxView() {
  const [fileName, setFileName] = useState("main.py");
  const [code, setCode] = useState(`def fibonacci(n):
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a

print("fib(10) =", fibonacci(10))
assert fibonacci(10) == 55
print("all tests passed")
`);
  const [cmd, setCmd] = useState("python3 main.py");
  const [backend, setBackend] = useState("local");
  const [timeoutSecs, setTimeoutSecs] = useState(60);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<SandboxResult | null>(null);
  const [backends, setBackends] = useState<{ id: string; name: string; available: boolean }[]>([]);
  const [instabox, setInstabox] = useState<{ ok: boolean; detail: string } | null>(null);

  useEffect(() => {
    api<{ backends: { id: string; name: string; available: boolean }[]; instabox: { ok: boolean; detail: string } }>("/api/sandbox")
      .then((d) => { setBackends(d.backends); setInstabox(d.instabox); })
      .catch(() => {});
  }, []);

  const run = async () => {
    if (busy) return;
    setBusy(true);
    setResult(null);
    try {
      const data = await api<SandboxResult>("/api/sandbox", {
        method: "POST",
        body: JSON.stringify({ cmd, timeout_secs: timeoutSecs, backend, files: [{ path: fileName, content: code }] }),
      });
      setResult(data);
    } catch (e) {
      setResult({
        result: { stdout: "", stderr: (e as Error).message, exitCode: -1, durationMs: 0, timedOut: false, backend },
        report: { passed: 0, failed: 1, errors: 0, skipped: 0, failurePatterns: [] },
        suggestions: [],
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid lg:grid-cols-2 gap-4 items-start">
      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardHeader className="py-3"><CardTitle className="text-sm">📝 Code</CardTitle></CardHeader>
        <CardContent className="space-y-3">
          <div className="flex flex-wrap gap-2">
            <Input value={fileName} onChange={(e) => setFileName(e.target.value)} className="bg-zinc-950 border-zinc-800 text-sm font-mono w-40" />
            <select value={backend} onChange={(e) => setBackend(e.target.value)}
              className="h-9 rounded-md border border-zinc-800 bg-zinc-950 px-3 text-sm">
              {(backends.length ? backends : [{ id: "local", name: "local (timeout + tempdir)", available: true }]).map((b) => (
                <option key={b.id} value={b.id} disabled={!b.available}>{b.name}{b.available ? "" : " (unavailable)"}</option>
              ))}
            </select>
          </div>
          <Textarea value={code} onChange={(e) => setCode(e.target.value)}
            className="bg-zinc-950 border-zinc-800 font-mono text-xs min-h-[240px] leading-relaxed" spellCheck={false} />
          <div className="flex flex-wrap gap-2 items-center">
            <Input value={cmd} onChange={(e) => setCmd(e.target.value)} className="bg-zinc-950 border-zinc-800 text-sm font-mono flex-1 min-w-48" />
            <Input type="number" value={timeoutSecs} min={5} max={540} onChange={(e) => setTimeoutSecs(Number(e.target.value))}
              className="bg-zinc-950 border-zinc-800 text-sm w-20" />
            <Button onClick={run} disabled={busy} className="bg-emerald-600 hover:bg-emerald-500 text-white">
              {busy ? <Loader2 className="h-4 w-4 mr-1 animate-spin" /> : "▶"} Run
            </Button>
          </div>
          {instabox && (
            <p className={instabox.ok ? "text-[11px] text-emerald-400" : "text-[11px] text-zinc-500"}>
              instabox: {instabox.ok ? `connected (${instabox.detail})` : instabox.detail}
            </p>
          )}
        </CardContent>
      </Card>

      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardHeader className="py-3">
          <CardTitle className="text-sm">⚙️ Result {result && <span className="ml-2 text-[10px] text-zinc-500 font-normal">{result.result.backend} · {result.result.durationMs}ms</span>}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {!result && <p className="text-xs text-zinc-500">Run code to see stdout/stderr, a structured test report and harness suggestions.</p>}
          {result && (
            <>
              <div className="flex flex-wrap gap-2 text-xs">
                <span className={result.result.exitCode === 0 ? "font-mono px-2 py-1 rounded bg-emerald-950/60 text-emerald-300" : "font-mono px-2 py-1 rounded bg-red-950/60 text-red-300"}>exit {result.result.exitCode}</span>
                <span className="px-2 py-1 rounded bg-zinc-800 text-zinc-300">{result.report.passed} passed</span>
                <span className="px-2 py-1 rounded bg-zinc-800 text-zinc-300">{result.report.failed + result.report.errors} failed</span>
                {result.result.timedOut && <span className="px-2 py-1 rounded bg-amber-950/60 text-amber-300">timed out</span>}
                {result.report.failurePatterns.map((p) => (
                  <span key={p} className="px-2 py-1 rounded bg-red-950/40 text-red-300">{p}</span>
                ))}
              </div>
              <pre className="max-h-56 overflow-auto rounded bg-black/70 p-3 text-[11px] leading-relaxed text-zinc-300 whitespace-pre-wrap">
                {result.result.stdout || "(no stdout)"}
                {result.result.stderr ? `\n--- stderr ---\n${result.result.stderr}` : ""}
              </pre>
              {result.suggestions.map((s, i) => (
                <div key={i} className="rounded-lg border border-amber-900/50 bg-amber-950/20 p-2.5 text-xs">
                  <span className="font-medium text-amber-300">🛠️ {s.pattern}</span>
                  <p className="mt-0.5 text-zinc-400">{s.suggestion}</p>
                </div>
              ))}
            </>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

// ─── RAG ────────────────────────────────────────────────────────────────────

interface RagHit {
  chunkId: string; path: string; lineStart: number; lineEnd: number; score: number;
  bm25Rank: number | null; vectorRank: number | null; snippet: string;
}

export function RagView() {
  const [query, setQuery] = useState("durable object reduce");
  const [hits, setHits] = useState<RagHit[]>([]);
  const [status, setStatus] = useState<{ files: number; chunks: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [reindexing, setReindexing] = useState(false);

  const doSearch = useCallback(async (q: string) => {
    if (!q.trim()) return;
    setBusy(true);
    try {
      const data = await api<{ hits: RagHit[]; files: number; chunks: number }>(`/api/rag?q=${encodeURIComponent(q)}`);
      setHits(data.hits);
      setStatus({ files: data.files, chunks: data.chunks });
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    const t = setTimeout(() => doSearch("durable object reduce"), 10);
    return () => clearTimeout(t);
  }, [doSearch]);

  const reindex = async () => {
    setReindexing(true);
    try {
      const data = await api<{ files: number; chunks: number }>("/api/rag", { method: "POST" });
      setStatus(data);
    } finally {
      setReindexing(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardContent className="p-4 space-y-3">
          <div className="flex gap-2">
            <div className="relative flex-1">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-zinc-500" />
              <Input value={query} onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && doSearch(query)}
                placeholder="Hybrid search: BM25 + hashed vectors + RRF fusion…" className="bg-zinc-950 border-zinc-800 pl-9 text-sm" />
            </div>
            <Button onClick={() => doSearch(query)} disabled={busy} className="bg-emerald-600 hover:bg-emerald-500 text-white">
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : "Search"}
            </Button>
            <Button onClick={reindex} disabled={reindexing} variant="outline" className="border-zinc-700 text-zinc-300">
              {reindexing ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
            </Button>
          </div>
          <p className="text-[11px] text-zinc-500">
            Indexes the PandaSwarm codebase (<code>crates/</code>, <code>docs/</code>, <code>src/lib</code>) plus every artifact agents write — {status ? `${status.files} files · ${status.chunks} chunks` : "…"}
          </p>
        </CardContent>
      </Card>

      <div className="space-y-2">
        {hits.map((h) => (
          <Card key={h.chunkId} className="bg-zinc-900/60 border-zinc-800 hover:border-zinc-700 transition-colors">
            <CardContent className="p-3">
              <div className="flex flex-wrap items-center gap-2 text-xs">
                <span className="font-mono text-zinc-200">{h.path}</span>
                <span className="text-zinc-500">:{h.lineStart}-{h.lineEnd}</span>
                {h.bm25Rank ? <span className="rounded border border-zinc-700 px-1.5 text-[9px] text-zinc-400">bm25 #{h.bm25Rank}</span> : null}
                {h.vectorRank ? <span className="rounded border border-zinc-700 px-1.5 text-[9px] text-zinc-400">vec #{h.vectorRank}</span> : null}
                <span className="ml-auto text-emerald-400/80 font-mono text-[10px]">{h.score.toFixed(4)}</span>
              </div>
              <pre className="mt-2 max-h-28 overflow-auto rounded bg-black/50 p-2 text-[10px] leading-relaxed text-zinc-400"><code>{h.snippet}</code></pre>
            </CardContent>
          </Card>
        ))}
        {hits.length === 0 && !busy && (
          <Card className="bg-zinc-900/40 border-zinc-800 border-dashed">
            <CardContent className="p-8 text-center text-sm text-zinc-500">No hits — try “sandbox”, “fibonacci”, or hit Reindex first.</CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}

// ─── Memory ─────────────────────────────────────────────────────────────────

interface FactItem { id: string; kind: string; content: string; salience: number; hits: number; createdAt: string; lastHitAt: string }

export function MemoryView() {
  const [facts, setFacts] = useState<FactItem[]>([]);
  const [content, setContent] = useState("");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    const data = await api<{ facts: FactItem[] }>("/api/memory?limit=50");
    setFacts(data.facts);
  }, []);

  useEffect(() => {
    const t = setTimeout(load, 10);
    return () => clearTimeout(t);
  }, [load]);

  const remember = async () => {
    if (!content.trim()) return;
    setBusy(true);
    try {
      await api("/api/memory", { method: "POST", body: JSON.stringify({ content, kind: "note" }) });
      setContent("");
      await load();
    } finally {
      setBusy(false);
    }
  };

  const recall = async () => {
    if (!query.trim()) return load();
    setBusy(true);
    try {
      const data = await api<{ facts: FactItem[] }>(`/api/memory?q=${encodeURIComponent(query)}`);
      setFacts(data.facts);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="grid md:grid-cols-2 gap-4">
        <Card className="bg-zinc-900/60 border-zinc-800">
          <CardHeader className="py-3"><CardTitle className="text-sm flex items-center gap-2"><Plus className="h-4 w-4 text-emerald-400" /> Remember a fact</CardTitle></CardHeader>
          <CardContent className="space-y-2">
            <Textarea value={content} onChange={(e) => setContent(e.target.value)} placeholder="e.g. The instabox template for validation runs is ci"
              className="bg-zinc-950 border-zinc-800 text-sm min-h-[64px]" />
            <Button onClick={remember} disabled={busy || !content.trim()} size="sm" className="bg-emerald-600 hover:bg-emerald-500 text-white">
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4 mr-1" />} Remember
            </Button>
            <p className="text-[11px] text-zinc-500">Near-duplicates (cosine ≥ 0.92) merge; salience decays exponentially unless re-hit.</p>
          </CardContent>
        </Card>
        <Card className="bg-zinc-900/60 border-zinc-800">
          <CardHeader className="py-3"><CardTitle className="text-sm flex items-center gap-2"><Brain className="h-4 w-4 text-emerald-400" /> Recall</CardTitle></CardHeader>
          <CardContent className="space-y-2">
            <div className="flex gap-2">
              <Input value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === "Enter" && recall()}
                placeholder="semantic query…" className="bg-zinc-950 border-zinc-800 text-sm" />
              <Button onClick={recall} size="sm" variant="outline" className="border-zinc-700 text-zinc-300">Recall</Button>
            </div>
            <p className="text-[11px] text-zinc-500">Recalled facts get their salience and hit-count bumped — the master agent uses this context.</p>
          </CardContent>
        </Card>
      </div>

      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardHeader className="py-3">
          <CardTitle className="text-sm flex items-center gap-2"><Database className="h-4 w-4 text-emerald-400" /> Facts <span className="ml-auto text-xs text-zinc-500 font-normal">{facts.length}</span></CardTitle>
        </CardHeader>
        <CardContent>
          <div className="space-y-2 max-h-[420px] overflow-y-auto pr-1">
            {facts.length === 0 && <p className="text-xs text-zinc-500 p-2">No facts yet — run a swarm or add one above.</p>}
            {facts.map((f) => (
              <div key={f.id} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-2.5">
                <div className="flex items-center gap-2 text-xs">
                  <span className="rounded border border-zinc-700 px-1.5 text-[9px] text-zinc-400">{f.kind}</span>
                  <span className="text-zinc-500">×{f.hits} hits</span>
                  <span className="ml-auto font-mono text-[10px] text-emerald-400/80">S {f.salience.toFixed(2)}</span>
                </div>
                <p className="mt-1 text-xs text-zinc-300">{f.content}</p>
                <div className="mt-1.5 h-1 rounded-full bg-zinc-800 overflow-hidden">
                  <div className="h-full bg-emerald-500/70" style={{ width: `${Math.min(100, (f.salience / 2) * 100)}%` }} />
                </div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
