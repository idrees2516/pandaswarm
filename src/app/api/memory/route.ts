import { NextRequest, NextResponse } from "next/server";
import { listFacts, recallFacts, rememberFact } from "@/lib/engine/memory";

export async function GET(req: NextRequest) {
  const q = req.nextUrl.searchParams.get("q");
  if (q) {
    const facts = await recallFacts(q, 10, 0);
    return NextResponse.json({ facts });
  }
  const facts = await listFacts(Number(req.nextUrl.searchParams.get("limit") ?? 50));
  return NextResponse.json({ facts });
}

export async function POST(req: NextRequest) {
  const body = (await req.json().catch(() => ({}))) as { content?: string; kind?: string };
  const content = (body.content ?? "").trim();
  if (!content) return NextResponse.json({ error: "content is required" }, { status: 400 });
  const id = await rememberFact(content, body.kind ?? "note");
  return NextResponse.json({ id });
}
