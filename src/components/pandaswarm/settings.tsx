"use client";

import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, Cpu, Loader2, Monitor, Save, Server } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { api } from "./shared";

interface SettingsData {
  llm: { provider: string; model: string; baseUrl: string; hasApiKey: boolean };
  sandbox: { backend: string; instaboxTemplate: string; instaboxMode: string; hasInstaboxKey: boolean; instabox: { ok: boolean; detail: string } };
  computerUse: { cdpEndpoint: string };
}

export function SettingsView() {
  const [data, setData] = useState<SettingsData | null>(null);
  const [form, setForm] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  const load = useCallback(async () => {
    const d = await api<SettingsData>("/api/settings");
    setData(d);
    setForm({
      "llm.provider": d.llm.provider,
      "llm.model": d.llm.model,
      "llm.baseurl": d.llm.baseUrl,
      "sandbox.backend": d.sandbox.backend,
      "sandbox.instaboxtemplate": d.sandbox.instaboxTemplate,
      "sandbox.instaboxmode": d.sandbox.instaboxMode,
      "computeruse.cdp": d.computerUse.cdpEndpoint,
    });
  }, []);

  useEffect(() => {
    const t = setTimeout(load, 10);
    return () => clearTimeout(t);
  }, [load]);

  const save = async () => {
    setSaving(true);
    try {
      await api("/api/settings", { method: "POST", body: JSON.stringify(form) });
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
      await load();
    } finally {
      setSaving(false);
    }
  };

  if (!data) return <Card className="bg-zinc-900/60 border-zinc-800"><CardContent className="p-6 flex justify-center"><Loader2 className="h-5 w-5 animate-spin text-emerald-400" /></CardContent></Card>;

  const set = (k: string, v: string) => setForm((f) => ({ ...f, [k]: v }));

  return (
    <div className="space-y-4 max-w-3xl">
      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardHeader className="py-3"><CardTitle className="text-sm flex items-center gap-2"><Cpu className="h-4 w-4 text-emerald-400" /> LLM provider</CardTitle></CardHeader>
        <CardContent className="space-y-3">
          <div className="grid sm:grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <label className="text-xs text-zinc-400">Provider (embedded engine)</label>
              <select value={form["llm.provider"]} onChange={(e) => set("llm.provider", e.target.value)}
                className="h-9 w-full rounded-md border border-zinc-800 bg-zinc-950 px-3 text-sm">
                <option value="zai">zai (GLM — this preview)</option>
                <option value="mock">mock (deterministic, offline)</option>
                <option value="openai">OpenAI-compatible endpoint</option>
              </select>
            </div>
            <div className="space-y-1.5">
              <label className="text-xs text-zinc-400">Model</label>
              <Input value={form["llm.model"] ?? ""} onChange={(e) => set("llm.model", e.target.value)} className="bg-zinc-950 border-zinc-800 text-sm" />
            </div>
            <div className="space-y-1.5 sm:col-span-2">
              <label className="text-xs text-zinc-400">Base URL (OpenAI-compatible: DeepSeek, GLM, OpenRouter, Ollama…)</label>
              <Input value={form["llm.baseurl"] ?? ""} onChange={(e) => set("llm.baseurl", e.target.value)} placeholder="https://api.deepseek.com/v1"
                className="bg-zinc-950 border-zinc-800 text-sm font-mono" />
            </div>
          </div>
          <p className="text-[11px] text-zinc-500">The Rust server (<code>pandaswarm serve</code>) reads the same knobs from <code>pandaswarm.toml</code> / env vars.</p>
        </CardContent>
      </Card>

      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardHeader className="py-3"><CardTitle className="text-sm flex items-center gap-2">📦 Sandbox</CardTitle></CardHeader>
        <CardContent className="space-y-3">
          <div className="grid sm:grid-cols-3 gap-3">
            <div className="space-y-1.5">
              <label className="text-xs text-zinc-400">Backend</label>
              <select value={form["sandbox.backend"]} onChange={(e) => set("sandbox.backend", e.target.value)}
                className="h-9 w-full rounded-md border border-zinc-800 bg-zinc-950 px-3 text-sm">
                <option value="local">local (timeout + tempdir)</option>
                <option value="instabox">instabox.sh cloud VM</option>
              </select>
            </div>
            <div className="space-y-1.5">
              <label className="text-xs text-zinc-400">instabox template</label>
              <Input value={form["sandbox.instaboxtemplate"] ?? ""} onChange={(e) => set("sandbox.instaboxtemplate", e.target.value)} placeholder="ci"
                className="bg-zinc-950 border-zinc-800 text-sm font-mono" />
            </div>
            <div className="space-y-1.5">
              <label className="text-xs text-zinc-400">instabox mode</label>
              <select value={form["sandbox.instaboxmode"]} onChange={(e) => set("sandbox.instaboxmode", e.target.value)}
                className="h-9 w-full rounded-md border border-zinc-800 bg-zinc-950 px-3 text-sm">
                <option value="one_shot">one-shot (run + delete)</option>
                <option value="persistent">persistent box</option>
              </select>
            </div>
          </div>
          <div className={cn("rounded-lg border p-3 text-xs", data.sandbox.instabox.ok ? "border-emerald-900/50 bg-emerald-950/20 text-emerald-300" : "border-zinc-800 bg-zinc-950/60 text-zinc-400")}>
            <div className="flex items-center gap-2">
              <Server className="h-3.5 w-3.5" /> instabox.sh
              <span className={cn("ml-auto rounded-full border px-2 py-0.5 text-[10px]", data.sandbox.instabox.ok ? "border-emerald-800" : "border-zinc-700")}>
                {data.sandbox.instabox.ok ? "connected" : "not connected"}
              </span>
            </div>
            <p className="mt-1 text-zinc-500">{data.sandbox.instabox.detail || "Set INSTABOX_API_KEY (ibk_…) or add it below."}</p>
          </div>
          <div className="space-y-1.5">
            <label className="text-xs text-zinc-400">INSTABOX_API_KEY</label>
            <Input type="password" value={form["sandbox.instaboxkey"] ?? ""} onChange={(e) => set("sandbox.instaboxkey", e.target.value)} placeholder="ibk_…"
              className="bg-zinc-950 border-zinc-800 text-sm font-mono" />
            <p className="text-[11px] text-zinc-500">Stored server-side in the settings table; also readable from the INSTABOX_API_KEY env var.</p>
          </div>
        </CardContent>
      </Card>

      <Card className="bg-zinc-900/60 border-zinc-800">
        <CardHeader className="py-3"><CardTitle className="text-sm flex items-center gap-2"><Monitor className="h-4 w-4 text-emerald-400" /> Computer-use agent</CardTitle></CardHeader>
        <CardContent className="space-y-2">
          <label className="text-xs text-zinc-400">CDP endpoint (lightpanda / headless chromium)</label>
          <Input value={form["computeruse.cdp"] ?? ""} onChange={(e) => set("computeruse.cdp", e.target.value)} placeholder="http://127.0.0.1:9222"
            className="bg-zinc-950 border-zinc-800 text-sm font-mono" />
          <p className="text-[11px] text-zinc-500">
            The full client (screenshot → vision-LLM → click/type/scroll/wait) ships in <code>crates/pandaswarm-search/src/computer.rs</code>; the preview verifies reachability.
          </p>
        </CardContent>
      </Card>

      <div className="flex items-center gap-3">
        <Button onClick={save} disabled={saving} className="bg-emerald-600 hover:bg-emerald-500 text-white">
          {saving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4 mr-1" />} Save settings
        </Button>
        {saved && <span className="text-xs text-emerald-400 flex items-center gap-1"><CheckCircle2 className="h-3.5 w-3.5" /> saved</span>}
        <span className="text-xs text-zinc-500">web search: zai built-in (Tavily/Brave via Rust server)</span>
      </div>
    </div>
  );
}
