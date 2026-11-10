import { NextRequest, NextResponse } from "next/server";
import { runInSandbox, parseTestReport, suggestHarnessPatches, instaboxStatus } from "@/lib/engine/sandbox";

export async function POST(req: NextRequest) {
  const body = (await req.json().catch(() => ({}))) as {
    cmd?: string;
    files?: Array<{ path: string; content: string }>;
    timeout_secs?: number;
    backend?: "local" | "instabox";
  };
  const cmd = (body.cmd ?? "").trim();
  if (!cmd) return NextResponse.json({ error: "cmd is required" }, { status: 400 });
  try {
    const result = await runInSandbox({
      files: body.files ?? [],
      cmd,
      timeoutSecs: body.timeout_secs ?? 90,
      backend: body.backend ?? undefined,
    });
    const report = parseTestReport(result);
    return NextResponse.json({
      result,
      report,
      suggestions: suggestHarnessPatches(report, (body.files ?? []).map((f) => f.path)),
    });
  } catch (e) {
    return NextResponse.json({ error: (e as Error).message }, { status: 500 });
  }
}

export async function GET() {
  const instabox = await instaboxStatus();
  return NextResponse.json({
    backends: [
      { id: "local", name: "Local (timeout + tempdir)", available: true },
      { id: "instabox", name: "instabox.sh cloud VM (4 vCPU / 8 GB)", available: instabox.ok },
    ],
    instabox,
  });
}
