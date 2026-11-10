// Sandbox providers for the embedded engine: local (plain, timeout-bounded,
// staged in .pandaswarm/sandbox) and instabox.sh cloud VMs through the
// official CLI (INSTABOX_API_KEY). Mirrors crates/pandaswarm-sandbox.

import { spawn } from "child_process";
import { mkdir, writeFile, rm } from "fs/promises";
import path from "path";
import type { ExecResult, TestReport, HarnessSuggestion } from "./types";
import { db } from "@/lib/db";

export type SandboxBackendName = "auto" | "local" | "instabox";

export interface SandboxSettings {
  backend: SandboxBackendName;
  instaboxKey: string;
  instaboxTemplate: string;
  instaboxMode: "one_shot" | "persistent";
  defaultTimeoutSecs: number;
}

export const DEFAULT_SANDBOX: SandboxSettings = {
  backend: "local",
  instaboxKey: "",
  instaboxTemplate: "ci",
  instaboxMode: "one_shot",
  defaultTimeoutSecs: 90,
};

export async function getSandboxSettings(): Promise<SandboxSettings> {
  const envKey = process.env.INSTABOX_API_KEY ?? "";
  const rows = await db.setting.findMany({ where: { key: { startsWith: "sandbox." } } });
  const map = Object.fromEntries(rows.map((r) => [r.key, r.value]));
  return {
    backend: (map["sandbox.backend"] as SandboxBackendName) || "local",
    instaboxKey: map["sandbox.instaboxkey"] || envKey,
    instaboxTemplate: map["sandbox.instaboxtemplate"] || "ci",
    instaboxMode: (map["sandbox.instaboxmode"] as "one_shot" | "persistent") || "one_shot",
    defaultTimeoutSecs: Number(map["sandbox.timeout"] ?? 90),
  };
}

const ROOT = path.join(process.cwd(), ".pandaswarm", "sandbox");

export interface RunInput {
  files: Array<{ path: string; content: string }>;
  cmd: string;
  timeoutSecs: number;
  backend?: SandboxBackendName;
  sessionId?: string | null;
  agentId?: string | null;
}

/** Execute `cmd` under the chosen backend after staging `files`. */
export async function runInSandbox(input: RunInput): Promise<ExecResult> {
  const settings = await getSandboxSettings();
  const backend = input.backend ?? settings.backend;
  const timeout = Math.max(1, input.timeoutSecs || settings.defaultTimeoutSecs);
  const result =
    backend === "instabox"
      ? await instaboxRun(input, settings, timeout)
      : await localRun(input, timeout);

  // persist the run + parsed report
  const report = parseTestReport(result);
  try {
    await db.sandboxRun.create({
      data: {
        sessionId: input.sessionId ?? null,
        agentId: input.agentId ?? null,
        backend: result.backend,
        cmd: input.cmd,
        exitCode: result.exitCode,
        stdout: result.stdout.slice(0, 60_000),
        stderr: result.stderr.slice(0, 20_000),
        durationMs: result.durationMs,
        timedOut: result.timedOut,
        reportJson: JSON.stringify(report),
      },
    });
  } catch (e) {
    console.warn("[sandbox] could not persist run:", (e as Error).message);
  }
  return result;
}

// ─── local backend ──────────────────────────────────────────────────────────

async function localRun(input: RunInput, timeoutSecs: number): Promise<ExecResult> {
  const dir = path.join(ROOT, `run-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`);
  await mkdir(dir, { recursive: true });
  for (const f of input.files) {
    const safe = f.path.replace(/\.\./g, "").replace(/^\/+/, "");
    const target = path.join(dir, safe);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, f.content);
  }
  const started = Date.now();
  const result = await new Promise<ExecResult>((resolve) => {
    const child = spawn("bash", ["-c", `timeout ${timeoutSecs} bash -c ${shellQuote(input.cmd)}`], {
      cwd: dir,
      env: {
        PATH: process.env.PATH ?? "/usr/local/bin:/usr/bin:/bin",
        HOME: process.env.HOME ?? "/tmp",
        LANG: "C.UTF-8",
        PYTHONUNBUFFERED: "1",
      },
    });
    let stdout = "",
      stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, (timeoutSecs + 5) * 1000);
    child.stdout.on("data", (d) => (stdout += d.toString()));
    child.stderr.on("data", (d) => (stderr += d.toString()));
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({
        stdout: stdout.slice(0, 200_000),
        stderr: stderr.slice(0, 100_000),
        exitCode: timedOut ? 124 : (code ?? -1),
        timedOut,
        durationMs: Date.now() - started,
        backend: "local:plain",
      });
    });
  });
  // keep the staging dir for inspection, best-effort cleanup of old ones
  rm(dir, { recursive: true, force: true }).catch(() => {});
  return result;
}

function shellQuote(s: string): string {
  return `'${s.replace(/'/g, "'\\''")}'`;
}

// ─── instabox.sh backend ────────────────────────────────────────────────────

async function instaboxBin(): Promise<string> {
  const candidates = [
    process.env.INSTABOX_BIN,
    path.join(process.env.HOME ?? "/home/z", ".local/bin/instabox"),
    "/usr/local/bin/instabox",
    "instabox",
  ].filter(Boolean) as string[];
  for (const c of candidates) {
    if (await probe(c)) return c;
  }
  throw new Error(
    "instabox CLI not found — install it with `npx instabox@latest` (it lands in ~/.local/bin)"
  );
}

function probe(bin: string): Promise<boolean> {
  return new Promise((resolve) => {
    const p = spawn(bin, ["--version"], { stdio: "ignore" });
    p.on("error", () => resolve(false));
    p.on("close", (code) => resolve(code === 0));
  });
}

function execCli(
  bin: string,
  args: string[],
  apiKey: string,
  timeoutSecs: number
): Promise<{ stdout: string; stderr: string; code: number }> {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, {
      env: { ...process.env, INSTABOX_API_KEY: apiKey },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "",
      stderr = "";
    const timer = setTimeout(() => child.kill("SIGKILL"), (timeoutSecs + 60) * 1000);
    child.stdout.on("data", (d) => (stdout += d.toString()));
    child.stderr.on("data", (d) => (stderr += d.toString()));
    child.on("error", (e) => {
      clearTimeout(timer);
      reject(e);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ stdout, stderr, code: code ?? -1 });
    });
  });
}

async function instaboxRun(
  input: RunInput,
  settings: SandboxSettings,
  timeoutSecs: number
): Promise<ExecResult> {
  if (!settings.instaboxKey.startsWith("ibk_")) {
    throw new Error(
      "INSTABOX_API_KEY (ibk_…) not configured — add it in Settings → Sandbox"
    );
  }
  const bin = await instaboxBin();
  const started = Date.now();

  // Stage files outside any git repo so `push` takes everything.
  const stage = path.join("/tmp", `ps-stage-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`);
  await mkdir(stage, { recursive: true });
  for (const f of input.files) {
    const safe = f.path.replace(/\.\./g, "").replace(/^\/+/, "");
    const target = path.join(stage, safe);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, f.content);
  }

  try {
    // one-shot: create + push + run + delete in a single CLI call
    const out = await execCli(
      bin,
      [
        "run",
        "--template",
        settings.instaboxTemplate || "ci",
        "--push",
        stage,
        "--timeout",
        `${Math.min(timeoutSecs, 540)}s`,
        "--",
        "bash",
        "-lc",
        input.cmd,
      ],
      settings.instaboxKey,
      timeoutSecs
    );
    return {
      stdout: out.stdout.slice(0, 200_000),
      stderr: out.stderr.slice(0, 100_000),
      exitCode: out.code,
      timedOut: false,
      durationMs: Date.now() - started,
      backend: "instabox",
    };
  } finally {
    rm(stage, { recursive: true, force: true }).catch(() => {});
  }
}

/** Best-effort `instabox whoami` for the settings page. */
export async function instaboxStatus(): Promise<{ ok: boolean; detail: string }> {
  try {
    const settings = await getSandboxSettings();
    if (!settings.instaboxKey.startsWith("ibk_")) {
      return { ok: false, detail: "no ibk_ key configured" };
    }
    const bin = await instaboxBin();
    const out = await execCli(bin, ["whoami"], settings.instaboxKey, 20);
    const first = (out.stdout + out.stderr).split("\n").find((l) => l.trim()) ?? "";
    return { ok: out.code === 0, detail: first.trim().slice(0, 120) };
  } catch (e) {
    return { ok: false, detail: (e as Error).message.slice(0, 120) };
  }
}

// ─── harness: report parsing + suggestions (port of pandaswarm-sandbox) ─────

const KNOWN_PATTERNS: Array<[string, string]> = [
  ["AssertionError", "assertion"],
  ["ModuleNotFoundError", "missing_module"],
  ["ImportError", "import_error"],
  ["SyntaxError", "syntax_error"],
  ["NameError", "name_error"],
  ["TypeError", "type_error"],
  ["TimeoutError", "timeout"],
  ["ConnectionError", "network"],
  ["No space left on device", "disk_full"],
  ["permission denied", "permissions"],
  ["Segmentation fault", "segfault"],
];

function countWord(hay: string, word: string): number {
  let total = 0;
  for (const line of hay.split("\n")) {
    const re = new RegExp(`(\\d+)\\s+${word}s?\\b`, "gi");
    let m: RegExpExecArray | null;
    while ((m = re.exec(line))) total += parseInt(m[1], 10) || 0;
  }
  return total;
}

export function parseTestReport(result: ExecResult): TestReport {
  const raw = `${result.stdout}\n${result.stderr}`;
  let passed = countWord(raw, "passed");
  let failed = countWord(raw, "failed");
  const errors = countWord(raw, "error");
  const skipped = countWord(raw, "skipped");
  for (const line of raw.split("\n")) {
    const t = line.trim();
    if (/^ok\s/.test(t) && !/^not ok/.test(t)) passed++;
    else if (/^not ok/.test(t)) failed++;
    if (t.startsWith("FAIL ")) failed++;
    if (t.startsWith("--- FAIL:")) failed++;
    if (t.startsWith("--- PASS:")) passed++;
  }
  if (passed + failed + errors + skipped === 0) {
    if (result.exitCode === 0) passed = 1;
    else failed = 1;
  }
  const failurePatterns = KNOWN_PATTERNS.filter(([needle]) => raw.includes(needle)).map(
    ([, label]) => label
  );
  return {
    passed,
    failed,
    errors,
    skipped,
    durationMs: result.durationMs,
    backend: result.backend,
    rawOutput: raw.slice(0, 8_000),
    failurePatterns: [...new Set(failurePatterns)].slice(0, 6),
  };
}

export function suggestHarnessPatches(
  report: TestReport,
  artifactPaths: string[] = []
): HarnessSuggestion[] {
  const out: HarnessSuggestion[] = [];
  const firstArtifact = artifactPaths[0] ?? "the suite";
  for (const p of report.failurePatterns) {
    switch (p) {
      case "assertion":
        out.push({ pattern: p, suggestion: `Assertion failures in ${firstArtifact} — add property-based cases around edge inputs and tighten fixtures before the next agent run.`, priority: 4 });
        break;
      case "missing_module":
      case "import_error":
        out.push({ pattern: p, suggestion: "Declare the dependency in the manifest (requirements.txt / package.json) and re-run; the harness can pin the version.", priority: 5 });
        break;
      case "timeout":
        out.push({ pattern: p, suggestion: "Raise the per-test timeout or split long-running cases into background jobs the swarm can await.", priority: 3 });
        break;
      case "syntax_error":
        out.push({ pattern: p, suggestion: "Insert a fast formatter/parser gate (ruff/biome) before tests so syntax errors never burn a sandbox run.", priority: 5 });
        break;
      case "network":
        out.push({ pattern: p, suggestion: "Network is disabled in the sandbox profile — allow net for this agent or mock the endpoint with a recorded fixture.", priority: 3 });
        break;
      case "permissions":
        out.push({ pattern: p, suggestion: "chmod/chown the workspace in the harness setup step; sandboxes run as an unprivileged user by design.", priority: 3 });
        break;
      default:
        break;
    }
  }
  if (report.passed > 0 && report.failed > 0) {
    out.push({
      pattern: "flaky_mix",
      suggestion: `${report.passed} passed / ${report.failed} failed in one run — treat as flaky: mark, retry once, and only then fail the agent turn.`,
      priority: 2,
    });
  }
  return out.sort((a, b) => b.priority - a.priority).slice(0, 3);
}
