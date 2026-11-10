// LLM providers for the embedded engine: zai (default in this preview),
// mock (deterministic, zero-network) and openai-compatible (custom endpoints).
// Server-side only — z-ai-web-dev-sdk must never reach the client bundle.

import type { AgentSpec, LlmResponse, ToolCall } from "./types";
import { newId } from "./types";
import { extractJson } from "./text";
import { db } from "@/lib/db";

export type ProviderName = "zai" | "mock" | "openai";

export interface LlmSettings {
  provider: ProviderName;
  model: string;
  apiKey: string;
  baseUrl: string;
  temperature: number;
}

export const DEFAULT_LLM: LlmSettings = {
  provider: "zai",
  model: "glm-4.6",
  apiKey: "",
  baseUrl: "",
  temperature: 0.2,
};

export async function getLlmSettings(): Promise<LlmSettings> {
  const rows = await db.setting.findMany({ where: { key: { startsWith: "llm." } } });
  const map = Object.fromEntries(rows.map((r) => [r.key, r.value]));
  return {
    provider: (map["llm.provider"] as ProviderName) || "zai",
    model: map["llm.model"] || DEFAULT_LLM.model,
    apiKey: map["llm.apikey"] || "",
    baseUrl: map["llm.baseurl"] || "",
    temperature: Number(map["llm.temperature"] ?? DEFAULT_LLM.temperature),
  };
}

export interface ChatTurnRequest {
  system: string;
  user: string;
  tools?: Array<{ name: string; description: string; parameters: Record<string, unknown> }>;
  /** prior turns for multi-turn agents (role: assistant | user) */
  history?: Array<{ role: "assistant" | "user"; content: string }>;
}

/** Chat with the configured provider. Falls back to the mock on errors. */
export async function chat(req: ChatTurnRequest, settings?: LlmSettings): Promise<LlmResponse> {
  const s = settings ?? (await getLlmSettings());
  try {
    if (s.provider === "mock") return await mockChat(req);
    if (s.provider === "openai") return await openaiChat(req, s);
    return await zaiChat(req);
  } catch (err) {
    console.warn("[llm] provider failed, falling back to mock:", (err as Error).message);
    return await mockChat(req);
  }
}

// ─── zai (this environment's SDK) ───────────────────────────────────────────

async function zaiChat(req: ChatTurnRequest): Promise<LlmResponse> {
  const { default: ZAI } = await import("z-ai-web-dev-sdk");
  const zai = await ZAI.create();
  const completion = await zai.chat.completions.create({
    messages: [
      { role: "system" as const, content: req.system },
      ...(req.history ?? []).map((m) => ({ role: m.role, content: m.content })),
      { role: "user" as const, content: req.user },
    ],
    thinking: { type: "disabled" },
  });
  const content = completion.choices[0]?.message?.content ?? "";
  return { content, toolCalls: parseToolCallsFromText(content), provider: "zai", model: "glm" };
}

// ─── OpenAI-compatible endpoint ─────────────────────────────────────────────

async function openaiChat(req: ChatTurnRequest, s: LlmSettings): Promise<LlmResponse> {
  if (!s.baseUrl) throw new Error("openai provider needs a base URL");
  const res = await fetch(`${s.baseUrl.replace(/\/$/, "")}/chat/completions`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(s.apiKey ? { authorization: `Bearer ${s.apiKey}` } : {}),
    },
    body: JSON.stringify({
      model: s.model || "gpt-4o-mini",
      temperature: s.temperature,
      messages: [
        { role: "system", content: req.system },
        ...(req.history ?? []).map((m) => ({ role: m.role, content: m.content })),
        { role: "user", content: req.user },
      ],
    }),
  });
  if (!res.ok) throw new Error(`openai-compatible provider: ${res.status}`);
  const data = (await res.json()) as {
    choices?: Array<{ message?: { content?: string } }>;
  };
  const content = data.choices?.[0]?.message?.content ?? "";
  return {
    content,
    toolCalls: parseToolCallsFromText(content),
    provider: "openai",
    model: s.model,
  };
}

/**
 * The embedded engine uses a JSON tool protocol instead of provider-native
 * function calling (works identically across zai/openai/mock): the model is
 * asked to reply with an optional ```json {tool_calls:[...]}``` block.
 */
function parseToolCallsFromText(text: string): ToolCall[] {
  const m = text.match(/```json\s*([\s\S]*?)```/);
  if (!m) return [];
  const parsed = extractJson(m[1]);
  const calls = parsed?.tool_calls;
  if (!Array.isArray(calls)) return [];
  return calls
    .filter((c): c is Record<string, unknown> => !!c && typeof c === "object")
    .map((c, i) => ({
      id: String(c.id ?? `call_${i}`),
      name: String(c.name ?? ""),
      arguments:
        typeof c.arguments === "string" ? c.arguments : JSON.stringify(c.arguments ?? {}),
    }))
    .filter((c) => c.name);
}

// ─── Deterministic mock (offline demo / CI) ─────────────────────────────────

export interface TaskHints {
  role: string;
  goal: string;
  agentName: string;
  language: string;
  workspaceFiles: string[];
  task: string;
  turn: number;
}

export function parseHints(text: string): Partial<TaskHints> {
  const m = text.match(/<task-hints>([\s\S]*?)<\/task-hints>/);
  if (!m) return {};
  try {
    return JSON.parse(m[1]);
  } catch {
    return {};
  }
}

async function mockChat(req: ChatTurnRequest): Promise<LlmResponse> {
  const h = parseHints(req.user);
  const role = h.role ?? "chat";
  const goal = h.goal ?? "";
  const turn = h.turn ?? 0;
  const language = h.language ?? "python";
  const py = language === "python";

  if (role === "planner") {
    const task = (h.task ?? "implement utilities").toLowerCase();
    const milestones: Array<{ title: string; acceptance: string[] }> = [];
    const add = (title: string, acceptance: string) =>
      milestones.push({ title, acceptance: [acceptance] });
    if (task.includes("fibonacci")) add("Implement fibonacci module", "fib(0..10) correct");
    if (task.includes("palindrome")) add("Implement palindrome checks", "case-insensitive checks pass");
    if (task.includes("sort")) add("Implement sorting", "quicksort matches sorted() on random data");
    if (milestones.length === 0) {
      add("Implement the requested utilities", "module imports cleanly and tests pass");
      add("Add a test suite", "pytest exits 0 in the sandbox");
    }
    milestones.push({ title: "Review and integrate", acceptance: ["reviewer sign-off"] });
    return {
      content: JSON.stringify({ objective: h.task ?? "", milestones }, null, 2),
      toolCalls: [],
      provider: "mock",
      model: "pandaswarm-mock-1",
    };
  }

  if (role === "coder") {
    if (turn > 0) {
      return {
        content: `Files for \`${goal}\` are written; handing off to the tester.`,
        toolCalls: [],
        provider: "mock",
        model: "mock",
      };
    }
    const files = mockImplementation(goal, py, h.agentName ?? "agent");
    const toolCalls: ToolCall[] = files.map(([path, content]) => ({
      id: newId("call"),
      name: "write_file",
      arguments: JSON.stringify({ path, content }),
    }));
    return {
      content: `Implementing \`${goal}\` — writing ${files.length} file(s).`,
      toolCalls,
      provider: "mock",
      model: "mock",
    };
  }

  if (role === "tester") {
    if (turn > 0) {
      return {
        content: "Validation run complete; awaiting the harness verdict.",
        toolCalls: [],
        provider: "mock",
        model: "mock",
      };
    }
    const mainFile =
      (h.workspaceFiles ?? []).find((f) => /\.(py|js|mjs)$/.test(f)) ??
      (py ? "helpers.py" : "helpers.js");
    const testPath = py ? "test_generated.py" : "test_generated.mjs";
    const body = py ? pythonTests(goal, mainFile) : jsTests(goal, mainFile);
    const cmd = py
      ? "python3 -m pytest test_generated.py -q || python3 test_generated.py"
      : "node test_generated.mjs";
    return {
      content: "Writing the test suite and validating it in the sandbox.",
      toolCalls: [
        { id: newId("call"), name: "write_file", arguments: JSON.stringify({ path: testPath, content: body }) },
        { id: newId("call"), name: "sandbox_run", arguments: JSON.stringify({ cmd, timeout_secs: 90 }) },
      ],
      provider: "mock",
      model: "mock",
    };
  }

  if (role === "reviewer") {
    return {
      content: `Review of \`${goal}\`:\n- modules are small and dependency-free ✅\n- edge cases handled ✅\nSign-off: LGTM.`,
      toolCalls: [],
      provider: "mock",
      model: "mock",
    };
  }

  return {
    content: `[mock] Received: ${req.user.slice(0, 200)}\nSwitch the LLM provider in Settings for live intelligence.`,
    toolCalls: [],
    provider: "mock",
    model: "mock",
  };
}

function mockImplementation(goal: string, py: boolean, agent: string): Array<[string, string]> {
  const g = goal.toLowerCase();
  if (g.includes("fibonacci")) {
    return py
      ? [
          [
            "fibonacci.py",
            `"""Fibonacci utilities with an iterative fast path."""


def fibonacci(n: int) -> int:
    """Return the n-th Fibonacci number (fib(0) == 0, fib(1) == 1)."""
    if n < 0:
        raise ValueError("n must be >= 0")
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a


def fib_sequence(count: int) -> list[int]:
    """Return the first \`count\` Fibonacci numbers."""
    if count < 0:
        raise ValueError("count must be >= 0")
    out = []
    a, b = 0, 1
    for _ in range(count):
        out.append(a)
        a, b = b, a + b
    return out
`,
          ],
        ]
      : [
          [
            "fibonacci.js",
            `// Fibonacci utilities with an iterative fast path.
export function fib(n) {
  if (!Number.isInteger(n) || n < 0) throw new RangeError("n must be a non-negative integer");
  let a = 0n, b = 1n;
  for (let i = 0; i < n; i++) [a, b] = [b, a + b];
  return a;
}
`,
          ],
        ];
  }
  if (g.includes("palindrome")) {
    return py
      ? [
          [
            "palindrome.py",
            `"""Palindrome checks, ignoring case and non-alphanumerics."""


def is_palindrome(text: str) -> bool:
    """True when \`text\` reads the same forwards and backwards."""
    cleaned = "".join(ch.lower() for ch in text if ch.isalnum())
    return cleaned == cleaned[::-1]
`,
          ],
        ]
      : [
          [
            "palindrome.js",
            `export function isPalindrome(text) {
  const cleaned = String(text).toLowerCase().replace(/[^a-z0-9]/g, "");
  return cleaned === [...cleaned].reverse().join("");
}
`,
          ],
        ];
  }
  if (g.includes("sort")) {
    return py
      ? [
          [
            "sorting.py",
            `"""In-place quicksort with median-of-three pivots."""


def quicksort(items: list) -> list:
    """Return a new sorted list."""
    out = list(items)
    _quicksort(out, 0, len(out) - 1)
    return out


def _median_of_three(a: list, lo: int, hi: int):
    mid = (lo + hi) // 2
    x, y, z = a[lo], a[mid], a[hi]
    if x <= y <= z or z <= y <= x:
        return y
    if y <= x <= z or z <= x <= y:
        return x
    return z


def _quicksort(a: list, lo: int, hi: int) -> None:
    while lo < hi:
        pivot = _median_of_three(a, lo, hi)
        i, j = lo, hi
        while i <= j:
            while a[i] < pivot:
                i += 1
            while a[j] > pivot:
                j -= 1
            if i <= j:
                a[i], a[j] = a[j], a[i]
                i += 1
                j -= 1
        if j - lo < hi - i:
            _quicksort(a, lo, j)
            lo = i
        else:
            _quicksort(a, i, hi)
            hi = j
`,
          ],
        ]
      : [
          [
            "sorting.js",
            `export function quicksort(items) {
  const a = [...items];
  sort(a, 0, a.length - 1);
  return a;
}
function sort(a, lo, hi) {
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const pivot = [a[lo], a[mid], a[hi]].sort((x, y) => x - y)[1];
    let i = lo, j = hi;
    while (i <= j) {
      while (a[i] < pivot) i++;
      while (a[j] > pivot) j--;
      if (i <= j) { [a[i], a[j]] = [a[j], a[i]]; i++; j--; }
    }
    if (j - lo < hi - i) { sort(a, lo, j); lo = i; } else { sort(a, i, hi); hi = j; }
  }
}
`,
          ],
        ];
  }
  // generic helpers
  return py
    ? [
        [
          "helpers.py",
          `"""Small, dependency-free helpers written by agent \`${agent}\`."""


def slugify(text: str) -> str:
    out = []
    for ch in text.lower():
        if ch.isalnum():
            out.append(ch)
        elif out and out[-1] != "-":
            out.append("-")
    return "".join(out).strip("-")


def chunked(items: list, size: int) -> list[list]:
    if size <= 0:
        raise ValueError("size must be positive")
    return [items[i : i + size] for i in range(0, len(items), size)]
`,
        ],
      ]
    : [
        [
          "helpers.js",
          `// Small, dependency-free helpers written by agent \`${agent}\`.
export function slugify(text) {
  return String(text).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}
export function chunked(items, size) {
  if (!Number.isInteger(size) || size <= 0) throw new RangeError("size must be positive");
  const out = [];
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
  return out;
}
`,
        ],
      ];
}

function pythonTests(goal: string, mainFile: string): string {
  const stem = mainFile.replace(/\.py$/, "");
  const g = goal.toLowerCase();
  if (g.includes("fibonacci")) {
    return `from ${stem} import fibonacci, fib_sequence


def test_base_cases():
    assert fibonacci(0) == 0
    assert fibonacci(1) == 1


def test_first_ten():
    assert fib_sequence(10) == [0, 1, 1, 2, 3, 5, 8, 13, 21, 34]


def test_negative_raises():
    import pytest
    with pytest.raises(ValueError):
        fibonacci(-1)
`;
  }
  if (g.includes("palindrome")) {
    return `from ${stem} import is_palindrome


def test_simple():
    assert is_palindrome("racecar")
    assert not is_palindrome("panda")


def test_ignores_punctuation_and_case():
    assert is_palindrome("A man, a plan, a canal: Panama")
`;
  }
  if (g.includes("sort")) {
    return `from ${stem} import quicksort

import random


def test_sorted_output():
    data = [random.randint(-100, 100) for _ in range(200)]
    assert quicksort(data) == sorted(data)


def test_empty_and_single():
    assert quicksort([]) == []
    assert quicksort([1]) == [1]
`;
  }
  return `from ${stem} import slugify, chunked


def test_slugify():
    assert slugify("Hello World!") == "hello-world"


def test_chunked():
    assert chunked([1, 2, 3, 4, 5], 2) == [[1, 2], [3, 4], [5]]
`;
}

function jsTests(_goal: string, mainFile: string): string {
  return `import * as mod from "./${mainFile}";
const checks = [
  ["fib(10) === 55", () => (mod.fib ? mod.fib(10) === 55n : true)],
  ["slugify", () => (mod.slugify ? mod.slugify("Hello World!") === "hello-world" : true)],
];
let failed = 0;
for (const [name, ok] of checks) {
  if (!ok()) { console.error("FAIL " + name); failed++; }
  else console.log("ok - " + name);
}
if (failed > 0) process.exit(1);
console.log("all tests passed");
`;
}
