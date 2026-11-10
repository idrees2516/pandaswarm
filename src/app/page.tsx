"use client";

import { useCallback, useEffect, useState } from "react";
import { Brain, Boxes, Cpu, Gauge, Search, Settings2, Terminal } from "lucide-react";
import { api, type Stats, type TabId } from "@/components/pandaswarm/shared";
import { OverviewView } from "@/components/pandaswarm/overview";
import { SessionsView } from "@/components/pandaswarm/sessions";
import { MemoryView, RagView, SandboxView } from "@/components/pandaswarm/tools";
import { SettingsView } from "@/components/pandaswarm/settings";
import { cn } from "@/lib/utils";

const TABS: Array<{ id: TabId; label: string; icon: React.ComponentType<{ className?: string }> }> = [
  { id: "overview", label: "Overview", icon: Gauge },
  { id: "sessions", label: "Swarm Sessions", icon: Boxes },
  { id: "sandbox", label: "Sandbox", icon: Terminal },
  { id: "rag", label: "RAG Search", icon: Search },
  { id: "memory", label: "Memory", icon: Brain },
  { id: "settings", label: "Settings", icon: Settings2 },
];

export default function Home() {
  const [tab, setTab] = useState<TabId>("overview");
  const [stats, setStats] = useState<Stats | null>(null);

  const refreshStats = useCallback(async () => {
    try {
      const data = await api<{ stats: Stats }>("/api/settings");
      setStats(data.stats);
    } catch { /* ignore */ }
  }, []);

  useEffect(() => {
    const initial = setTimeout(refreshStats, 10);
    const t = setInterval(refreshStats, 5000);
    return () => {
      clearTimeout(initial);
      clearInterval(t);
    };
  }, [refreshStats]);

  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-100 flex flex-col">
      <header className="border-b border-zinc-800/80 sticky top-0 z-20 bg-zinc-950/90 backdrop-blur">
        <div className="mx-auto max-w-7xl px-4 sm:px-6 h-16 flex items-center gap-4">
          <div className="flex items-center gap-2.5">
            <div className="h-9 w-9 rounded-xl bg-emerald-500/15 border border-emerald-500/30 flex items-center justify-center text-xl">🐼</div>
            <div>
              <h1 className="font-bold text-lg leading-tight tracking-tight">PandaSwarm</h1>
              <p className="text-[11px] text-zinc-500 leading-tight hidden sm:block">agentic coding-agent orchestration</p>
            </div>
          </div>
          <span className="ml-2 hidden md:inline-flex items-center gap-1 rounded-full border border-emerald-800/60 bg-emerald-950/40 px-2 py-0.5 text-[11px] text-emerald-300">
            <Cpu className="h-3 w-3" /> Rust core · embedded engine
          </span>
          <nav className="ml-auto flex items-center gap-1 overflow-x-auto">
            {TABS.map((t) => (
              <button
                key={t.id}
                onClick={() => setTab(t.id)}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium transition-colors whitespace-nowrap border",
                  tab === t.id
                    ? "bg-emerald-500/15 text-emerald-300 border-emerald-500/30"
                    : "text-zinc-400 hover:text-zinc-200 hover:bg-zinc-900 border-transparent"
                )}
              >
                <t.icon className="h-4 w-4" />
                <span className="hidden sm:inline">{t.label}</span>
              </button>
            ))}
          </nav>
        </div>
      </header>

      <main className="flex-1 mx-auto w-full max-w-7xl px-4 sm:px-6 py-6">
        {tab === "overview" && <OverviewView stats={stats} onOpenSession={() => setTab("sessions")} onSandbox={() => setTab("sandbox")} />}
        {tab === "sessions" && <SessionsView />}
        {tab === "sandbox" && <SandboxView />}
        {tab === "rag" && <RagView />}
        {tab === "memory" && <MemoryView />}
        {tab === "settings" && <SettingsView />}
      </main>

      <footer className="border-t border-zinc-800/80 mt-auto">
        <div className="mx-auto max-w-7xl px-4 sm:px-6 py-4 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-zinc-500">
          <span>PandaSwarm v0.1 — three-tier cowork · durable objects · layered sandboxes · hybrid RAG</span>
          <span className="ml-auto">Rust workspace in <code className="text-zinc-400">crates/</code> · extends pandacode</span>
        </div>
      </footer>
    </div>
  );
}
