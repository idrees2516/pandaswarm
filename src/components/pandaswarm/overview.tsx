"use client";

import { Bot, Brain, FileCode2, Package, Search, Sparkles, ListTree, Users, Zap, FlaskConical, Play, Terminal } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import type { Stats } from "./shared";

export function OverviewView({ stats, onOpenSession, onSandbox }: { stats: Stats | null; onOpenSession: () => void; onSandbox: () => void }) {
  const cards = [
    { label: "Sessions", value: stats?.sessions ?? 0, icon: Package, hint: "swarms + chats" },
    { label: "Agents spawned", value: stats?.agents ?? 0, icon: Bot, hint: "tier-3 workers" },
    { label: "Artifacts", value: stats?.artifacts ?? 0, icon: FileCode2, hint: "files written" },
    { label: "Sandbox runs", value: stats?.runs ?? 0, icon: Terminal, hint: "validated executions" },
    { label: "Memory facts", value: stats?.facts ?? 0, icon: Brain, hint: "semantic, decaying" },
    { label: "RAG chunks", value: stats?.rag.chunks ?? 0, icon: Search, hint: `${stats?.rag.files ?? 0} files indexed` },
  ];
  return (
    <div className="space-y-6">
      <section className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
        {cards.map((c) => (
          <Card key={c.label} className="bg-zinc-900/60 border-zinc-800">
            <CardContent className="p-4">
              <div className="flex items-center justify-between">
                <span className="text-xs text-zinc-500">{c.label}</span>
                <c.icon className="h-4 w-4 text-emerald-500/80" />
              </div>
              <div className="mt-2 text-2xl font-bold tabular-nums">{c.value}</div>
              <div className="text-[11px] text-zinc-600">{c.hint}</div>
            </CardContent>
          </Card>
        ))}
      </section>

      <section className="grid lg:grid-cols-3 gap-4">
        {[
          {
            icon: Sparkles, title: "Tier 1 — Master agent", body: "Talks to you, owns the plan, receives results. Chat turns blend semantic memory and hybrid RAG context.",
            tag: "plan · receive",
          },
          {
            icon: ListTree, title: "Tier 2 — Coordinator", body: "A self-hosted Durable Object: append-only event log, snapshots, KV and alarm-driven ticks. Kill the process — the swarm resumes.",
            tag: "durable objects (Rust)",
          },
          {
            icon: Users, title: "Tier 3 — Sub-agents", body: "Coders, testers, reviewers, integrators execute in layered workspaces; code is validated in sandboxes before it counts.",
            tag: "swarm · sandbox",
          },
        ].map((t) => (
          <Card key={t.title} className="bg-zinc-900/60 border-zinc-800">
            <CardHeader className="pb-2">
              <CardTitle className="text-sm flex items-center gap-2">
                <t.icon className="h-4 w-4 text-emerald-400" /> {t.title}
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-2">
              <p className="text-xs text-zinc-400 leading-relaxed">{t.body}</p>
              <span className="inline-flex rounded-full border border-zinc-700 px-2 py-0.5 text-[10px] text-zinc-400">{t.tag}</span>
            </CardContent>
          </Card>
        ))}
      </section>

      <section className="grid md:grid-cols-2 gap-4">
        <Card className="bg-gradient-to-br from-emerald-950/40 to-zinc-900/60 border-emerald-900/50">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm flex items-center gap-2"><Zap className="h-4 w-4 text-emerald-400" /> Launch a swarm</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <p className="text-xs text-zinc-400">Describe a coding task. The planner decomposes it into milestones and spawns a swarm that writes, tests and reviews real code.</p>
            <Button onClick={onOpenSession} className="bg-emerald-600 hover:bg-emerald-500 text-white">
              <Play className="h-4 w-4 mr-1" /> New task
            </Button>
          </CardContent>
        </Card>
        <Card className="bg-zinc-900/60 border-zinc-800">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm flex items-center gap-2"><FlaskConical className="h-4 w-4 text-emerald-400" /> Try the sandbox</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <p className="text-xs text-zinc-400">Run code locally (timeout + tempdir) or on instabox.sh cloud VMs — with structured test reports and harness suggestions.</p>
            <Button onClick={onSandbox} variant="outline" className="border-zinc-700 text-zinc-200 hover:bg-zinc-800">
              <Terminal className="h-4 w-4 mr-1" /> Open sandbox
            </Button>
          </CardContent>
        </Card>
      </section>
    </div>
  );
}
