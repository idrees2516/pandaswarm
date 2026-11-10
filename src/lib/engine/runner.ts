// The swarm runner: plans, executes agent turns (LLM + tools + sandbox),
// streams events into the DB. This is the embedded engine's tier-2
// coordinator — the Next.js twin of crates/pandaswarm-orchestrator/src/swarm.rs.

import { mkdir, readFile, readdir, writeFile, cp } from "fs/promises";
import path from "path";
import { db } from "@/lib/db";
import { chat, parseHints, getLlmSettings } from "./llm";
import { planSwarm } from "./planner";
import { runInSandbox, parseTestReport, suggestHarnessPatches } from "./sandbox";
import { search as ragSearch, addDocument } from "./rag";
import { rememberFact } from "./memory";
import { newId, nowMs } from "./types";
import type { AgentSpec, ExecResult, TestReport } from "./types";

const WORKSPACE_ROOT = path.join(process.cwd(), ".pandaswarm", "workspace");

// guard against double-running the same session
const running = new Set<string>();

// per-agent conversation memory (tier-3 sub-agents keep context across turns)
const histories = new Map<string, Array<{ role: "assistant" | "user"; content: string }>>();

async function emit(
  sessionId: string,
  kind: string,
  payload: Record<string, unknown>
): Promise<void> {
  const last = await db.swarmEvent.findFirst({
    where: { sessionId },
    orderBy: { seq: "desc" },
    select: { seq: true },
  });
  await db.swarmEvent.create({
    data: {
      sessionId,
      seq: (last?.seq ?? 0) + 1,
      kind,
      payload: JSON.stringify(payload),
    },
  });
}

// ─── session creation + swarm kickoff ───────────────────────────────────────

export async function createTaskSession(prompt: string, title?: string): Promise<string> {
  const sessionId = newId("sess");
  await db.session.create({
    data: {
      id: sessionId,
      title: (title ?? prompt).slice(0, 80),
      mode: "task",
      status: "planning",
    },
  });
  await db.message.create({
    data: { sessionId, role: "user", content: prompt },
  });
  await emit(sessionId, "task.created", { prompt });

  const { plan, agents } = await planSwarm(prompt);
  await db.session.update({ where: { id: sessionId }, data: { status: "running" } });
  for (const a of agents) {
    await db.agent.create({
      data: {
        id: a.id,
        sessionId,
        name: a.name,
        role: a.role,
        goal: a.goal,
        workspace: a.workspace,
        model: a.model,
        maxTurns: a.maxTurns,
        dependsOn: JSON.stringify(a.dependsOn),
        status: "pending",
      },
    });
    await emit(sessionId, "agent.spawned", { agent: a });
  }
  await emit(sessionId, "plan.generated", {
    plan,
    agents,
    note: `${agents.length} agents across ${plan.milestones.length} milestones`,
  });
  await rememberFact(`task started: ${prompt}`, "task", sessionId);

  void driveSwarm(sessionId).catch(async (e) => {
    console.error("[runner] swarm crashed:", e);
    await db.session.update({ where: { id: sessionId }, data: { status: "failed", summary: (e as Error).message } });
    await emit(sessionId, "task.completed", { status: "failed", summary: (e as Error).message });
  });

  return sessionId;
}

// ─── the coordinator loop ───────────────────────────────────────────────────

async function driveSwarm(sessionId: string): Promise<void> {
  if (running.has(sessionId)) return;
  running.add(sessionId);
  try {
    for (;;) {
      const done = await tick(sessionId);
      if (done) break;
      await new Promise((r) => setTimeout(r, 50));
    }
  } finally {
    running.delete(sessionId);
  }
}

async function tick(sessionId: string): Promise<boolean> {
  const session = await db.session.findUnique({ where: { id: sessionId } });
  if (!session || session.status === "cancelled") return true;

  const agents = await db.agent.findMany({ where: { sessionId }, orderBy: { createdAt: "asc" } });
  const terminal = (s: string) => ["done", "failed", "cancelled"].includes(s);
  const byId = new Map(agents.map((a) => [a.id, a]));

  const next =
    agents.find(
      (a) =>
        a.status === "pending" &&
        JSON.parse(a.dependsOn || "[]").every((d: string) =>
          ["done", "cancelled"].includes(byId.get(d)?.status ?? "done")
        )
    ) ?? agents.find((a) => a.status === "running" && a.turns < a.maxTurns);

  if (!next) {
    if (agents.every((a) => terminal(a.status))) {
      const failed = agents.filter((a) => a.status === "failed").length;
      const artifactCount = await db.artifact.count({ where: { sessionId } });
      const runCount = await db.sandboxRun.count({ where: { sessionId } });
      const status = failed > 0 ? "failed" : "completed";
      const summary = `${agents.length} agents finished, ${artifactCount} artifact(s), ${runCount} sandbox run(s)`;
      await db.session.update({ where: { id: sessionId }, data: { status, summary } });
      await emit(sessionId, "task.completed", { status, summary });
      return true;
    }
    // dependency deadlock
    await db.session.update({
      where: { id: sessionId },
      data: { status: "failed", summary: "no runnable agent (dependency cycle?)" },
    });
    return true;
  }

  await runAgentTurn(sessionId, next.id);
  return false;
}

// ─── one agent turn (tier 3) ────────────────────────────────────────────────

async function runAgentTurn(sessionId: string, agentId: string): Promise<void> {
  const agent = await db.agent.findUnique({ where: { id: agentId } });
  if (!agent) return;
  const session = await db.session.findUnique({ where: { id: sessionId } });
  if (!session) return;

  const workspace = path.join(WORKSPACE_ROOT, sessionId, agent.workspace);
  await mkdir(workspace, { recursive: true });

  // Layered-workspace merge: stage dependency artifacts into this workspace.
  for (const depId of JSON.parse(agent.dependsOn || "[]") as string[]) {
    const dep = await db.agent.findUnique({ where: { id: depId } });
    if (!dep) continue;
    const src = path.join(WORKSPACE_ROOT, sessionId, dep.workspace);
    try {
      for (const e of await readdir(src, { withFileTypes: true })) {
        if (e.isFile()) {
          const to = path.join(workspace, e.name);
          try {
            await readFile(to);
          } catch {
            await cp(path.join(src, e.name), to);
          }
        }
      }
    } catch {
      /* dependency workspace may not exist yet */
    }
  }

  const workspaceFiles = await listFiles(workspace);
  const hints = {
    role: agent.role,
    goal: agent.goal,
    agent_name: agent.name,
    language: "python",
    workspace_files: workspaceFiles,
    task: session.title,
    turn: agent.turns,
  };

  await db.agent.update({
    where: { id: agentId },
    data: { status: "running", turns: { increment: 1 }, startedAt: agent.startedAt ?? new Date() },
  });
  await emit(sessionId, "agent.status", { agent_id: agentId, agent: agent.name, status: "running", detail: "turn started" });

  const llmSettings = await getLlmSettings();
  const toolProtocol = llmSettings.provider === "mock" ? "" : `
TOOL PROTOCOL — to act, reply with ONLY this fenced json block (no other text):
\`\`\`json
{"tool_calls": [{"name": "write_file", "arguments": {"path": "fib.py", "content": "def fib(n): ..."}}]}
\`\`\`
Tools: write_file(path, content) · read_file(path) · sandbox_run(cmd, timeout_secs) · rag_search(query) · web_search(query) · done(summary).
Rules:
- write_file: one call per file, full content, no placeholders.
- After writing code, ALWAYS validate it in the same or next turn with sandbox_run (e.g. {"name":"sandbox_run","arguments":{"cmd":"python3 -m pytest -q || python3 fib.py"}}).
- When your goal is fully achieved and validated, reply (plain text, no json block) with a one-line summary.
- Never re-read files you just wrote; their names are listed in the hints.`;

  const roleGuide: Record<string, string> = {
    coder: "Write the implementation files for your goal, then validate with sandbox_run.",
    tester: "Write a pytest test file for the workspace modules, then run sandbox_run with: python3 -m pytest <testfile> -q || python3 <testfile>. IMPORTANT: import from the EXACT module filenames listed in workspace_files (e.g. 'from fibonacci import ...'), never invent module names.",
    reviewer: "Read the workspace files and reply with a short review verdict.",
    integrator: "Read the artifacts, write a SUMMARY.md merge note, then finish.",
  };

  const history = histories.get(agentId) ?? [];
  const resp = await chat({
    system:
      `You are ${agent.name} (role: ${agent.role}) in a PandaSwarm swarm working on: "${session.title}". ` +
      `Goal: ${agent.goal}. ${roleGuide[agent.role] ?? ""} Be concise and produce working code.` +
      toolProtocol,
    user: `<task-hints>${JSON.stringify(hints)}</task-hints>`,
    history,
  });

  if (resp.content && !resp.toolCalls.length) {
    await emit(sessionId, "log", { level: "info", agent: agent.name, message: resp.content.slice(0, 400) });
  }

  // record this turn in the agent's conversation memory
  const turnHistory = histories.get(agentId) ?? [];
  if (resp.content) turnHistory.push({ role: "assistant", content: resp.content.slice(0, 2000) });
  turnHistory.push({
    role: "user",
    content: resp.toolCalls.length
      ? `Tool results from your last turn:\n${resp.toolCalls
          .map((tc) => `- ${tc.name}(${tc.arguments.slice(0, 120)}...) executed.`)
          .join("\n")}\nContinue: validate with sandbox_run if you wrote code, then finish with a summary.`
      : `(turn ${agent.turns + 1})`,
  });
  if (turnHistory.length > 8) turnHistory.splice(0, turnHistory.length - 8);
  histories.set(agentId, turnHistory);

  let failed = false;
  for (const tc of resp.toolCalls) {
    try {
      await dispatchTool(sessionId, agentId, agent.name, workspace, tc.name, tc.arguments);
    } catch (e) {
      failed = true;
      await emit(sessionId, "log", { level: "error", agent: agent.name, message: `tool ${tc.name}: ${(e as Error).message}` });
    }
  }

  // Builders that reply plain text on their first turn get one insistent retry.
  const needsRetry =
    resp.toolCalls.length === 0 &&
    (agent.role === "coder" || agent.role === "tester") &&
    agent.turns + 1 < 2;
  if (needsRetry) {
    turnHistory.push({
      role: "user",
      content: 'You replied with plain text but wrote nothing yet. Reply with the ```json {"tool_calls":[...]} ``` block now to write your files.',
    });
    histories.set(agentId, turnHistory);
  }
  const noMoreWork = resp.toolCalls.length === 0 && !needsRetry;
  const turnsExhausted = agent.turns + 1 >= agent.maxTurns;
  if (failed) {
    await db.agent.update({
      where: { id: agentId },
      data: { status: "failed", error: "tool failure", finishedAt: new Date() },
    });
    await emit(sessionId, "agent.status", { agent_id: agentId, agent: agent.name, status: "failed", detail: "tool failure" });
  } else if (noMoreWork || turnsExhausted) {
    await db.agent.update({ where: { id: agentId }, data: { status: "done", finishedAt: new Date() } });
    await emit(sessionId, "agent.status", { agent_id: agentId, agent: agent.name, status: "done", detail: noMoreWork ? "no further tool calls" : "turn budget exhausted" });
    // milestone completion
    await emit(sessionId, "milestone.completed", { agent: agent.name, title: agent.goal });
  } else {
    await db.agent.update({ where: { id: agentId }, data: { status: "running" } });
  }
}

async function dispatchTool(
  sessionId: string,
  agentId: string,
  agentName: string,
  workspace: string,
  tool: string,
  argsJson: string
): Promise<void> {
  let args: Record<string, unknown> = {};
  try {
    args = JSON.parse(argsJson);
  } catch {
    /* empty args */
  }

  switch (tool) {
    case "write_file": {
      const p = String(args.path ?? "").replace(/\.\./g, "").replace(/^\/+/, "");
      if (!p) throw new Error("write_file: empty path");
      const content = String(args.content ?? "");
      const target = path.join(workspace, p);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, content);
      const language = languageOf(p);
      const existing = await db.artifact.findFirst({ where: { sessionId, path: p }, orderBy: { createdAt: "desc" } });
      if (!existing || existing.content !== content) {
        await db.artifact.create({
          data: {
            sessionId,
            agentId,
            path: p,
            language,
            content,
            bytes: content.length,
          },
        });
      }
      addDocument(`${sessionId}/${agentName}/${p}`, content);
      await emit(sessionId, "artifact.written", { agent_id: agentId, agent: agentName, path: p, bytes: content.length });
      break;
    }
    case "read_file": {
      const p = String(args.path ?? "");
      const content = await readFile(path.join(workspace, p), "utf8");
      await emit(sessionId, "tool.call", {
        agent_id: agentId,
        agent: agentName,
        tool: "read_file",
        path: p,
        bytes: content.length,
      });
      break;
    }
    case "sandbox_run": {
      const cmd = String(args.cmd ?? "true");
      const timeoutSecs = Number(args.timeout_secs ?? 90);
      const files = await collectFiles(workspace);
      await emit(sessionId, "agent.status", { agent_id: agentId, agent: agentName, status: "waiting_sandbox", detail: cmd });
      const result: ExecResult = await runInSandbox({
        files,
        cmd,
        timeoutSecs,
        sessionId,
        agentId,
      });
      const report: TestReport = parseTestReport(result);
      await emit(sessionId, "sandbox.run", { agent_id: agentId, agent: agentName, cmd, result });
      await emit(sessionId, "sandbox.test_report", { agent_id: agentId, agent: agentName, report });
      if (!report.passed && (report.failed || report.errors)) {
        for (const s of suggestHarnessPatches(report)) {
          await emit(sessionId, "harness.suggestion", s);
        }
      }
      break;
    }
    case "rag_search": {
      const query = String(args.query ?? "");
      const hits = ragSearch(query, 5);
      await emit(sessionId, "tool.call", {
        agent_id: agentId,
        agent: agentName,
        tool: "rag_search",
        query,
        hits: hits.map((h) => ({ path: h.path, lines: `${h.lineStart}-${h.lineEnd}`, score: h.score })),
      });
      break;
    }
    case "web_search": {
      const query = String(args.query ?? "");
      const results = await webSearch(query);
      await emit(sessionId, "tool.call", {
        agent_id: agentId,
        agent: agentName,
        tool: "web_search",
        query,
        results,
      });
      break;
    }
    case "done":
      await emit(sessionId, "log", { level: "info", agent: agentName, message: String(args.summary ?? "done") });
      break;
    default:
      await emit(sessionId, "log", { level: "warn", agent: agentName, message: `unknown tool ${tool}` });
  }
}

// ─── web search (zai SDK → Tavily/Brave fallback chain) ────────────────────

async function webSearch(query: string): Promise<Array<Record<string, string>>> {
  try {
    const { default: ZAI } = await import("z-ai-web-dev-sdk");
    const zai = await ZAI.create();
    const results = await zai.functions.invoke("web_search", { query, num: 5 });
    return (results ?? []).map((r) => ({
      title: r.name,
      url: r.url,
      snippet: (r.snippet ?? "").slice(0, 200),
    }));
  } catch (e) {
    return [{ error: `web search unavailable: ${(e as Error).message}` }];
  }
}

// ─── chat sessions (tier 1 master agent) ───────────────────────────────────

export async function chatTurn(sessionId: string, content: string): Promise<string> {
  await db.message.create({ data: { sessionId, role: "user", content } });
  const { recallFacts } = await import("./memory");
  const facts = await recallFacts(content, 4, 0).catch(() => []);
  const hits = ragSearch(content, 4);
  const context =
    `Semantic memory:\n${facts.map((f) => `- (${f.salience.toFixed(2)}) ${f.content}`).join("\n")}\n` +
    `Relevant indexed code:\n${hits.map((h) => `- ${h.path}:${h.lineStart}-${h.lineEnd} (${h.score.toFixed(3)})`).join("\n")}`;
  const reply = await chat({
    system: `You are the PandaSwarm master agent (tier 1). Answer using the context when relevant.\n${context}`,
    user: content,
  });
  await db.message.create({ data: { sessionId, role: "assistant", content: reply.content } });
  await db.session.update({ where: { id: sessionId }, data: { updatedAt: new Date() } });
  return reply.content;
}

export async function cancelSession(sessionId: string, reason: string): Promise<void> {
  await db.session.update({ where: { id: sessionId }, data: { status: "cancelled", summary: reason } });
  await db.agent.updateMany({
    where: { sessionId, status: { in: ["pending", "running", "waiting_tool", "waiting_sandbox"] } },
    data: { status: "cancelled" },
  });
  await emit(sessionId, "log", { level: "warn", message: `cancelled: ${reason}` });
}

// ─── helpers ────────────────────────────────────────────────────────────────

async function listFiles(dir: string): Promise<string[]> {
  try {
    return (await readdir(dir, { withFileTypes: true }))
      .filter((e) => e.isFile())
      .map((e) => e.name)
      .sort();
  } catch {
    return [];
  }
}

async function collectFiles(dir: string): Promise<Array<{ path: string; content: string }>> {
  const out: Array<{ path: string; content: string }> = [];
  async function walk(d: string, prefix: string) {
    try {
      for (const e of await readdir(d, { withFileTypes: true })) {
        if (e.isDirectory()) await walk(path.join(d, e.name), path.join(prefix, e.name));
        else out.push({ path: path.join(prefix, e.name), content: await readFile(path.join(d, e.name), "utf8") });
      }
    } catch {
      /* missing dir */
    }
  }
  await walk(dir, "");
  return out;
}

function languageOf(p: string): string {
  const ext = p.split(".").pop() ?? "";
  const map: Record<string, string> = {
    py: "python",
    js: "javascript",
    mjs: "javascript",
    ts: "typescript",
    rs: "rust",
    go: "go",
    sh: "bash",
    md: "markdown",
  };
  return map[ext] ?? "text";
}

export { parseHints, nowMs };
