import { NextRequest, NextResponse } from "next/server";
import { chatTurn } from "@/lib/engine/runner";

export async function POST(
  req: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const body = (await req.json().catch(() => ({}))) as { content?: string };
  const content = (body.content ?? "").trim();
  if (!content) return NextResponse.json({ error: "content is required" }, { status: 400 });
  try {
    const reply = await chatTurn(id, content);
    return NextResponse.json({ reply });
  } catch (e) {
    return NextResponse.json({ error: (e as Error).message }, { status: 500 });
  }
}
