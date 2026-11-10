"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity, Bot, CircleDot, FileCode2, FlaskConical, Layers, Loader2, MessageSquare,
  Play, Sparkles, Square, Trash2,
} from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  api, EVENT_ICONS, StatusBadge, summarizeEvent, timeAgo,
  type SessionDetail, type SessionListItem,
} from "./shared";

export function SessionsView() {
  const [sessions, setSessions] = useState<SessionListItem[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("implement fibonacci and palindrome utilities with tests");
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    try {
      const data = await api<{ sessions: SessionListItem[] }>("/api/sessions");
      setSessions(data.sessions);
    } catch { /* ignore */ }
  }, []);

  useEffect(() => {
    const initial = setTimeout(load, 10);
    const t = setInterval(load, 3000);
    return () => { clearTimeout(initial); clearInterval(t); };
  }, [load]);

  const createTask = async () => {
    if (!prompt.trim() || creating) return;
    setCreating(true);
    try {
      const data = await api<{ session_id: string }>("/api/sessions", {
        method: "POST",
        body: JSON.stringify({ prompt }),
      });
      setSelected(data.session_id);
      await load();
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardContent className="p-4 space-y-3">
          <div className="flex items-center gap-2 text-sm font-medium"><Sparkles className="h-4 w-4 text-emerald-400" /> New swarm task</div>
          <Textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="Describe the coding task… e.g. implement fibonacci and palindrome utilities with tests"
            className="bg-zinc-950 border-zinc-800 min-h-[70px] text-sm"
          />
          <div className="flex items-center gap-2">
            <Button onClick={createTask} disabled={creating || !prompt.trim()} className="bg-emerald-600 hover:bg-emerald-500 text-white">
              {creating ? <Loader2 className="h-4 w-4 mr-1 animate-spin" /> : <Play className="h-4 w-4 mr-1" />}
              Spawn swarm
            </Button>
            <span className="text-xs text-zinc-500">planner → coders → tester → reviewer → integrator</span>
          </div>
        </CardContent>
      </Card>

      <div className="grid lg:grid-cols-[320px_1fr] gap-4 items-start">
        <Card className="bg-zinc-900/60 border-zinc-800">
          <CardHeader className="py-3">
            <CardTitle className="text-sm flex items-center gap-2"><Layers className="h-4 w-4 text-emerald-400" /> Sessions</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <ScrollArea className="max-h-[480px]">
              <div className="p-2 space-y-1">
                {sessions.length === 0 && <p className="text-xs text-zinc-500 p-3">No sessions yet — spawn a swarm above.</p>}
                {sessions.map((s) => (
                  <button
                    key={s.id}
                    onClick={() => setSelected(s.id)}
                    className={cn(
                      "w-full text-left rounded-lg border p-2.5 transition-colors",
                      selected === s.id ? "border-emerald-600/60 bg-emerald-950/30" : "border-transparent hover:bg-zinc-800/60"
                    )}
                  >
                    <div className="flex items-center gap-2">
                      <span className="text-xs font-medium truncate flex-1">{s.title}</span>
                      <StatusBadge status={s.status} />
                    </div>
                    <div className="mt-1 flex items-center gap-2 text-[10px] text-zinc-500">
                      <span>{s.counts.agents} agents</span><CircleDot className="h-2 w-2" /><span>{s.counts.artifacts} artifacts</span><CircleDot className="h-2 w-2" /><span>{timeAgo(s.updatedAt)}</span>
                    </div>
                  </button>
                ))}
              </div>
            </ScrollArea>
          </CardContent>
        </Card>

        {selected ? (
          <SessionDetailPanel sessionId={selected} onClose={() => setSelected(null)} />
        ) : (
          <Card className="bg-zinc-900/40 border-zinc-800 border-dashed">
            <CardContent className="p-10 text-center text-sm text-zinc-500">
              Select a session to watch its swarm work.
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}

function SessionDetailPanel({ sessionId, onClose }: { sessionId: string; onClose: () => void }) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [viewArtifact, setViewArtifact] = useState<SessionDetail["artifacts"][number] | null>(null);
  const [chatInput, setChatInput] = useState("");
  const [chatBusy, setChatBusy] = useState(false);
  const eventsRef = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    try {
      const data = await api<SessionDetail>(`/api/sessions/${sessionId}`);
      setDetail(data);
    } catch { /* ignore */ }
  }, [sessionId]);

  useEffect(() => {
    const initial = setTimeout(load, 10);
    const t = setInterval(load, 2000);
    return () => { clearTimeout(initial); clearInterval(t); };
  }, [load]);

  useEffect(() => {
    eventsRef.current?.scrollTo({ top: eventsRef.current.scrollHeight });
  }, [detail?.events.length]);

  const sendChat = async () => {
    if (!chatInput.trim() || chatBusy) return;
    setChatBusy(true);
    try {
      await api(`/api/sessions/${sessionId}/chat`, { method: "POST", body: JSON.stringify({ content: chatInput }) });
      setChatInput("");
      await load();
    } finally {
      setChatBusy(false);
    }
  };

  if (!detail) return <Card className="bg-zinc-900/60 border-zinc-800"><CardContent className="p-6"><Loader2 className="h-5 w-5 animate-spin text-emerald-400" /></CardContent></Card>;

  const running = ["running", "planning", "queued"].includes(detail.session.status);
  const suggestions = detail.events.filter((e) => e.kind === "harness.suggestion").map((e) => e.payload as { pattern: string; suggestion: string; priority: number });

  return (
    <div className="space-y-4">
      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardContent className="p-4">
          <div className="flex flex-wrap items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-sm font-semibold truncate">{detail.session.title}</div>
              <div className="text-[11px] text-zinc-500 font-mono truncate">{detail.session.id}</div>
            </div>
            <StatusBadge status={detail.session.status} />
            {running ? (
              <Button size="sm" variant="outline" className="border-red-900/60 text-red-300 hover:bg-red-950/40 h-8"
                onClick={() => api(`/api/sessions/${sessionId}/cancel`, { method: "POST", body: JSON.stringify({ reason: "user request" }) })}>
                <Square className="h-3 w-3 mr-1" /> Cancel
              </Button>
            ) : null}
            <Button size="sm" variant="ghost" className="h-8 text-zinc-400" onClick={onClose}><Trash2 className="h-3 w-3" /></Button>
          </div>
          {detail.session.summary && <p className="mt-2 text-xs text-zinc-400">{detail.session.summary}</p>}
        </CardContent>
      </Card>

      <div className="grid xl:grid-cols-2 gap-4">
        <Card className="bg-zinc-900/60 border-zinc-800">
          <CardHeader className="py-3">
            <CardTitle className="text-sm flex items-center gap-2"><Bot className="h-4 w-4 text-emerald-400" /> Agent swarm</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2 max-h-[340px] overflow-y-auto">
            {detail.agents.map((a) => (
              <div key={a.id} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-2.5">
                <div className="flex items-center gap-2">
                  <span className="text-xs font-semibold">{a.name}</span>
                  <Badge variant="outline" className="text-[9px] px-1.5 py-0 border-zinc-700 text-zinc-400">{a.role}</Badge>
                  <StatusBadge status={a.status} />
                  <span className="ml-auto text-[10px] text-zinc-500">{a.turns}/{a.maxTurns} turns</span>
                </div>
                <p className="mt-1 text-[11px] text-zinc-500 truncate">{a.goal}</p>
                {a.artifacts.length > 0 && (
                  <div className="mt-1.5 flex flex-wrap gap-1">
                    {a.artifacts.map((x) => (
                      <button key={x.id} onClick={() => setViewArtifact(detail.artifacts.find((y) => y.id === x.id) ?? null)}
                        className="inline-flex items-center gap-1 rounded border border-emerald-900/60 bg-emerald-950/30 px-1.5 py-0.5 text-[10px] text-emerald-300 hover:bg-emerald-900/40">
                        <FileCode2 className="h-2.5 w-2.5" /> {x.path}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </CardContent>
        </Card>

        <Card className="bg-zinc-900/60 border-zinc-800">
          <CardHeader className="py-3">
            <CardTitle className="text-sm flex items-center gap-2"><Activity className="h-4 w-4 text-emerald-400" /> Event stream
              {running && <span className="ml-auto flex items-center gap-1 text-[10px] text-amber-300"><span className="h-1.5 w-1.5 rounded-full bg-amber-400 animate-pulse" /> live</span>}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div ref={eventsRef} className="max-h-[340px] overflow-y-auto space-y-1 pr-1">
              {detail.events.map((e) => (
                <div key={e.seq} className="flex gap-2 text-[11px] leading-relaxed border-b border-zinc-800/50 pb-1">
                  <span className="text-zinc-600 tabular-nums w-8 shrink-0 text-right">#{e.seq}</span>
                  <span className="shrink-0">{EVENT_ICONS[e.kind] ?? "•"}</span>
                  <span className="text-zinc-300 min-w-0">
                    <span className="text-emerald-400/90 font-mono">{e.kind}</span>{" "}
                    <span className="text-zinc-500 break-all">{summarizeEvent(e)}</span>
                  </span>
                </div>
              ))}
              {detail.events.length === 0 && <p className="text-xs text-zinc-500">Waiting for events…</p>}
            </div>
          </CardContent>
        </Card>
      </div>

      {(detail.runs.length > 0 || suggestions.length > 0) && (
        <Card className="bg-zinc-900/60 border-zinc-800">
          <CardHeader className="py-3">
            <CardTitle className="text-sm flex items-center gap-2"><FlaskConical className="h-4 w-4 text-emerald-400" /> Validation harness</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {detail.runs.map((r) => {
              const rep = r.report ?? {};
              const green = (rep.failed ?? 0) === 0 && (rep.errors ?? 0) === 0 && r.exitCode === 0;
              return (
                <div key={r.id} className="rounded-lg border border-zinc-800 bg-zinc-950/60 p-3">
                  <div className="flex flex-wrap items-center gap-2 text-xs">
                    <span className={cn("font-mono px-1.5 py-0.5 rounded", green ? "bg-emerald-950/60 text-emerald-300" : "bg-red-950/60 text-red-300")}>
                      exit {r.exitCode}
                    </span>
                    <Badge variant="outline" className="text-[10px] border-zinc-700 text-zinc-400">{r.backend}</Badge>
                    <span className="text-zinc-500">{rep.passed ?? 0}✅ {rep.failed ?? 0}❌ {rep.errors ?? 0}💥 · {r.durationMs}ms</span>
                    <code className="ml-auto text-[10px] text-zinc-500 truncate max-w-[300px]">{r.cmd}</code>
                  </div>
                  {(rep.rawOutput ?? r.stdout).trim() ? (
                    <pre className="mt-2 max-h-32 overflow-auto rounded bg-black/60 p-2 text-[10px] leading-relaxed text-zinc-400">
                      {(rep.rawOutput || r.stdout).trim().slice(0, 1500)}
                    </pre>
                  ) : null}
                </div>
              );
            })}
            {suggestions.map((s, i) => (
              <div key={i} className="rounded-lg border border-amber-900/50 bg-amber-950/20 p-3 text-xs">
                <div className="flex items-center gap-2 font-medium text-amber-300">
                  🛠️ harness suggestion · {s.pattern}
                  <Badge variant="outline" className="ml-auto border-amber-800 text-amber-300 text-[10px]">P{s.priority}</Badge>
                </div>
                <p className="mt-1 text-zinc-400">{s.suggestion}</p>
              </div>
            ))}
          </CardContent>
        </Card>
      )}

      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardHeader className="py-3">
          <CardTitle className="text-sm flex items-center gap-2"><MessageSquare className="h-4 w-4 text-emerald-400" /> Master agent</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2">
          <ScrollArea className="max-h-52">
            <div className="space-y-1.5">
              {detail.messages.map((m, i) => (
                <div key={i} className={cn("text-xs rounded-lg px-3 py-2 max-w-[85%]", m.role === "user" ? "ml-auto bg-emerald-950/40 border border-emerald-900/40" : "bg-zinc-800/60")}>
                  <div className="text-[10px] text-zinc-500 mb-0.5">{m.role}</div>
                  <p className="whitespace-pre-wrap">{m.content}</p>
                </div>
              ))}
            </div>
          </ScrollArea>
          <div className="flex gap-2">
            <Input value={chatInput} onChange={(e) => setChatInput(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") void sendChat(); }}
              placeholder="Ask the master agent about this session…" className="bg-zinc-950 border-zinc-800 text-sm" />
            <Button size="sm" disabled={chatBusy || !chatInput.trim()} onClick={sendChat}
              className="bg-emerald-600 hover:bg-emerald-500 text-white">
              {chatBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : "Send"}
            </Button>
          </div>
        </CardContent>
      </Card>

      {viewArtifact && (
        <div
          className="fixed inset-0 z-40 bg-black/70 backdrop-blur-sm flex items-center justify-center p-4"
          onClick={() => setViewArtifact(null)}
          onKeyDown={(e) => { if (e.key === "Escape") setViewArtifact(null); }}
        >
          <div className="max-w-4xl w-full max-h-[80vh] rounded-xl border border-zinc-800 bg-zinc-950 overflow-hidden flex flex-col" onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center gap-2 border-b border-zinc-800 px-4 py-3">
              <FileCode2 className="h-4 w-4 text-emerald-400" />
              <span className="text-sm font-mono">{viewArtifact.path}</span>
              <Badge variant="outline" className="text-[10px] border-zinc-700 text-zinc-400">{viewArtifact.language}</Badge>
              <span className="ml-auto text-xs text-zinc-500">{viewArtifact.bytes} bytes</span>
              <Button size="sm" variant="ghost" className="h-7" onClick={() => setViewArtifact(null)}>✕</Button>
            </div>
            <pre className="flex-1 overflow-auto p-4 text-xs leading-relaxed text-zinc-300"><code>{viewArtifact.content}</code></pre>
          </div>
        </div>
      )}
    </div>
  );
}
