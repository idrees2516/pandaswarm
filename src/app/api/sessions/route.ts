import { NextRequest, NextResponse } from "next/server";
import { db } from "@/lib/db";
import { createTaskSession, chatTurn } from "@/lib/engine/runner";

export async function GET() {
  const sessions = await db.session.findMany({
    orderBy: { updatedAt: "desc" },
    take: 50,
    include: {
      _count: { select: { agents: true, events: true, artifacts: true, runs: true } },
    },
  });
  return NextResponse.json({
    sessions: sessions.map((s) => ({
      id: s.id,
      title: s.title,
      mode: s.mode,
      status: s.status,
      summary: s.summary,
      createdAt: s.createdAt,
      updatedAt: s.updatedAt,
      counts: s._count,
    })),
  });
}

export async function POST(req: NextRequest) {
  const body = (await req.json().catch(() => ({}))) as {
    prompt?: string;
    title?: string;
    mode?: string;
  };
  const prompt = (body.prompt ?? "").trim();
  if (!prompt) {
    return NextResponse.json({ error: "prompt is required" }, { status: 400 });
  }

  if (body.mode === "chat") {
    // plain chat session (tier-1 master agent)
    let session = await db.session.findFirst({ where: { mode: "chat", title: "Chat" } });
    if (!session) {
      session = await db.session.create({ data: { id: `sess_chat_${Date.now().toString(36)}`, title: "Chat", mode: "chat", status: "completed" } });
    }
    const reply = await chatTurn(session.id, prompt);
    return NextResponse.json({ session_id: session.id, reply });
  }

  const sessionId = await createTaskSession(prompt, body.title);
  return NextResponse.json({ session_id: sessionId, status: "running" });
}
