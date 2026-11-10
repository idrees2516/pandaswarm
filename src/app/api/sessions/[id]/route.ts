import { NextRequest, NextResponse } from "next/server";
import { db } from "@/lib/db";

export async function GET(
  req: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const after = Number(req.nextUrl.searchParams.get("after") ?? 0);

  const session = await db.session.findUnique({
    where: { id },
    include: {
      messages: { orderBy: { createdAt: "asc" } },
      agents: { orderBy: { createdAt: "asc" }, include: { artifacts: true } },
      events: { orderBy: { seq: "asc" } },
      artifacts: { orderBy: { createdAt: "asc" } },
      runs: { orderBy: { createdAt: "desc" }, take: 20 },
    },
  });
  if (!session) return NextResponse.json({ error: "not found" }, { status: 404 });

  return NextResponse.json({
    session: {
      id: session.id,
      title: session.title,
      mode: session.mode,
      status: session.status,
      summary: session.summary,
      createdAt: session.createdAt,
      updatedAt: session.updatedAt,
    },
    messages: session.messages.map((m) => ({ role: m.role, content: m.content, createdAt: m.createdAt })),
    agents: session.agents.map((a) => ({
      id: a.id,
      name: a.name,
      role: a.role,
      goal: a.goal,
      status: a.status,
      turns: a.turns,
      maxTurns: a.maxTurns,
      workspace: a.workspace,
      dependsOn: JSON.parse(a.dependsOn || "[]"),
      error: a.error,
      artifacts: a.artifacts.map((x) => ({ id: x.id, path: x.path, language: x.language, bytes: x.bytes })),
    })),
    events: session.events
      .filter((e) => e.seq > after)
      .map((e) => ({ seq: e.seq, kind: e.kind, payload: safeJson(e.payload), createdAt: e.createdAt })),
    artifacts: session.artifacts.map((a) => ({
      id: a.id,
      agentId: a.agentId,
      path: a.path,
      language: a.language,
      bytes: a.bytes,
      content: a.content,
      createdAt: a.createdAt,
    })),
    runs: session.runs.map((r) => ({
      id: r.id,
      agentId: r.agentId,
      backend: r.backend,
      cmd: r.cmd,
      exitCode: r.exitCode,
      durationMs: r.durationMs,
      timedOut: r.timedOut,
      stdout: r.stdout.slice(0, 4000),
      stderr: r.stderr.slice(0, 2000),
      report: safeJson(r.reportJson),
      createdAt: r.createdAt,
    })),
  });
}

function safeJson(s: string): Record<string, unknown> {
  try {
    return JSON.parse(s);
  } catch {
    return {};
  }
}
