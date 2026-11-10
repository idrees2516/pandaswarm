import { NextResponse } from "next/server";

export async function GET() {
  return NextResponse.json({
    ok: true,
    service: "pandaswarm",
    engine: "embedded-typescript",
    rustBackend: process.env.PANDASWARM_RUST_URL ?? null,
    now: Date.now(),
  });
}
