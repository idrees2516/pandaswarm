import { NextRequest, NextResponse } from "next/server";
import { cancelSession } from "@/lib/engine/runner";

export async function POST(
  req: NextRequest,
  { params }: { params: Promise<{ id: string }> }
) {
  const { id } = await params;
  const body = (await req.json().catch(() => ({}))) as { reason?: string };
  await cancelSession(id, body.reason ?? "user request");
  return NextResponse.json({ cancelled: true });
}
