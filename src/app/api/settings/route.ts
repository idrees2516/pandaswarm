import { NextRequest, NextResponse } from "next/server";
import { db } from "@/lib/db";
import { instaboxStatus } from "@/lib/engine/sandbox";
import { ragStatus } from "@/lib/engine/rag";

export async function GET() {
  const settings = await db.setting.findMany();
  const map = Object.fromEntries(settings.map((s) => [s.key, s.value]));
  const instabox = await instaboxStatus();
  const [sessions, agents, artifacts, runs, facts] = await Promise.all([
    db.session.count(),
    db.agent.count(),
    db.artifact.count(),
    db.sandboxRun.count(),
    db.fact.count(),
  ]);
  return NextResponse.json({
    llm: {
      provider: map["llm.provider"] ?? "zai",
      model: map["llm.model"] ?? "glm-4.6",
      baseUrl: map["llm.baseurl"] ?? "",
      hasApiKey: Boolean(map["llm.apikey"]),
    },
    sandbox: {
      backend: map["sandbox.backend"] ?? "local",
      instaboxTemplate: map["sandbox.instaboxtemplate"] ?? "ci",
      instaboxMode: map["sandbox.instaboxmode"] ?? "one_shot",
      hasInstaboxKey: Boolean(map["sandbox.instaboxkey"] ?? process.env.INSTABOX_API_KEY),
      instabox,
    },
    computerUse: { cdpEndpoint: map["computeruse.cdp"] ?? "" },
    stats: { sessions, agents, artifacts, runs, facts, rag: ragStatus() },
  });
}

export async function POST(req: NextRequest) {
  const body = (await req.json().catch(() => ({}))) as Record<string, string>;
  const updates = Object.entries(body).filter(([k, v]) => typeof v === "string");
  for (const [key, value] of updates) {
    await db.setting.upsert({
      where: { key },
      update: { value },
      create: { key, value },
    });
  }
  return NextResponse.json({ saved: updates.length });
}
