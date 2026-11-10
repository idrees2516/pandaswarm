import { NextResponse } from "next/server";
import { db } from "@/lib/db";

/**
 * Computer-use agent status. The full CDP client (screenshot → vision-LLM →
 * click/type/scroll/wait) lives in crates/pandaswarm-search/src/computer.rs;
 * the preview exposes configuration + reachability of the endpoint.
 */
export async function GET() {
  const setting = await db.setting.findUnique({ where: { key: "computeruse.cdp" } });
  const endpoint = setting?.value ?? "";
  if (!endpoint) {
    return NextResponse.json({
      configured: false,
      hint: "set a CDP endpoint (e.g. http://127.0.0.1:9222 — lightpanda or headless chromium) in Settings",
    });
  }
  try {
    const res = await fetch(`${endpoint.replace(/\/$/, "")}/json/version`, {
      signal: AbortSignal.timeout(4000),
    });
    const data = (await res.json().catch(() => ({}))) as { Browser?: string };
    return NextResponse.json({ configured: true, reachable: res.ok, browser: data.Browser ?? null });
  } catch (e) {
    return NextResponse.json({ configured: true, reachable: false, error: (e as Error).message });
  }
}
