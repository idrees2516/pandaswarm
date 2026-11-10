// Shared client-side helpers + types for the PandaSwarm UI.

import { cn } from "@/lib/utils";
import { Loader2 } from "lucide-react";

export type TabId = "overview" | "sessions" | "sandbox" | "rag" | "memory" | "settings";

export interface SessionListItem {
  id: string; title: string; mode: string; status: string; summary: string;
  createdAt: string; updatedAt: string;
  counts: { agents: number; events: number; artifacts: number; runs: number };
}

export interface AgentView {
  id: string; name: string; role: string; goal: string; status: string;
  turns: number; maxTurns: number; workspace: string; dependsOn: string[];
  error: string | null; artifacts: Array<{ id: string; path: string; language: string; bytes: number }>;
}

export interface EventView { seq: number; kind: string; payload: Record<string, unknown>; createdAt: string }

export interface ArtifactView { id: string; agentId: string | null; path: string; language: string; bytes: number; content: string; createdAt: string }

export interface RunView {
  id: string; agentId: string | null; backend: string; cmd: string; exitCode: number;
  durationMs: number; timedOut: boolean; stdout: string; stderr: string;
  report: { passed?: number; failed?: number; errors?: number; skipped?: number; failurePatterns?: string[]; rawOutput?: string };
  createdAt: string;
}

export interface SessionDetail {
  session: { id: string; title: string; mode: string; status: string; summary: string; createdAt: string; updatedAt: string };
  messages: Array<{ role: string; content: string; createdAt: string }>;
  agents: AgentView[];
  events: EventView[];
  artifacts: ArtifactView[];
  runs: RunView[];
}

export interface Stats {
  sessions: number; agents: number; artifacts: number; runs: number; facts: number;
  rag: { files: number; chunks: number; lastIndexed: number };
}

export const STATUS_COLORS: Record<string, string> = {
  pending: "text-zinc-400 bg-zinc-800 border-zinc-700",
  running: "text-amber-300 bg-amber-950/60 border-amber-800",
  waiting_tool: "text-sky-300 bg-sky-950/60 border-sky-800",
  waiting_sandbox: "text-violet-300 bg-violet-950/60 border-violet-800",
  done: "text-emerald-300 bg-emerald-950/60 border-emerald-800",
  failed: "text-red-300 bg-red-950/60 border-red-800",
  cancelled: "text-zinc-500 bg-zinc-900 border-zinc-800",
  completed: "text-emerald-300 bg-emerald-950/60 border-emerald-800",
  planning: "text-amber-300 bg-amber-950/60 border-amber-800",
  queued: "text-zinc-400 bg-zinc-800 border-zinc-700",
};

export const EVENT_ICONS: Record<string, string> = {
  "task.created": "🚀", "plan.generated": "🗺️", "agent.spawned": "🐝", "agent.status": "🔄",
  "tool.call": "🔧", "sandbox.run": "📦", "sandbox.test_report": "🧪", "artifact.written": "📄",
  "milestone.completed": "🏁", "harness.suggestion": "🛠️", "task.completed": "✅", "log": "💬",
};

export function StatusBadge({ status }: { status: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium", STATUS_COLORS[status] ?? STATUS_COLORS.pending)}>
      {["running", "planning"].includes(status) && <Loader2 className="h-3 w-3 animate-spin" />}
      {status}
    </span>
  );
}

export function timeAgo(iso: string): string {
  const s = Math.floor((Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export async function api<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, { ...init, headers: { "content-type": "application/json", ...(init?.headers ?? {}) } });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error((data as { error?: string }).error ?? res.statusText);
  return data as T;
}

export function summarizeEvent(e: EventView): string {
  const p = e.payload;
  switch (e.kind) {
    case "agent.spawned": return `${(p.agent as { name?: string })?.name ?? "agent"} — ${(p.agent as { goal?: string })?.goal ?? ""}`;
    case "agent.status": return `${p.agent ?? p.agent_id} → ${p.status}${p.detail ? ` (${p.detail})` : ""}`;
    case "artifact.written": return `${p.agent ?? p.agent_id} wrote ${p.path} (${p.bytes}B)`;
    case "sandbox.run": return `${p.agent ?? p.agent_id}: ${p.cmd}`;
    case "sandbox.test_report": {
      const r = p.report as { passed?: number; failed?: number; errors?: number } | undefined;
      return `${r?.passed ?? 0} passed / ${(r?.failed ?? 0) + (r?.errors ?? 0)} failed`;
    }
    case "plan.generated": return String(p.note ?? "swarm scheduled");
    case "tool.call": return `${p.agent ?? p.agent_id} → ${p.tool}`;
    case "log": return String(p.message ?? "");
    case "milestone.completed": return String(p.title ?? "");
    case "task.completed": return `${p.status}: ${p.summary ?? ""}`;
    default: return JSON.stringify(p).slice(0, 120);
  }
}
