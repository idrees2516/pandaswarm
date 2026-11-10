import { NextRequest, NextResponse } from "next/server";
import { reindex, search, ragStatus } from "@/lib/engine/rag";

export async function GET(req: NextRequest) {
  const q = req.nextUrl.searchParams.get("q");
  if (!q) return NextResponse.json({ ...ragStatus(), hits: [] });
  // cold index (e.g. after a dev-server restart): build it before searching
  if (ragStatus().chunks === 0) await reindex();
  const hits = search(q, Number(req.nextUrl.searchParams.get("top_k") ?? 8));
  return NextResponse.json({ query: q, hits, ...ragStatus() });
}

export async function POST() {
  const result = await reindex();
  return NextResponse.json(result);
}
