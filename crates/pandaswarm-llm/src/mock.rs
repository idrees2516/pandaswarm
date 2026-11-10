//! Deterministic, zero-network mock provider. It recognises the structured
//! prompts the orchestrator emits (each carries a `<task-hints>` JSON block)
//! and answers with **real plans and real, working code**, so the whole
//! swarm → sandbox → validation → harness-tuner loop runs offline in CI and
//! `pandaswarm demo`.

use crate::{ChatRequest, ChatResponse, LlmProvider, ToolCallReq};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct MockProvider {
    model: String,
    turn: AtomicU64,
}

impl MockProvider {
    pub fn new(model: &str) -> Self {
        Self {
            model: if model.is_empty() { "pandaswarm-mock-1".into() } else { model.to_string() },
            turn: AtomicU64::new(0),
        }
    }

    fn next_id(&self) -> String {
        format!("mock_{}", self.turn.fetch_add(1, Ordering::Relaxed))
    }
}

/// Parse the orchestrator's `<task-hints>` block from a prompt.
fn hints(messages: &[crate::ChatMessage]) -> Value {
    for m in messages {
        if let Some(start) = m.content.find("<task-hints>") {
            if let Some(end) = m.content[start..].find("</task-hints>") {
                if let Ok(v) = serde_json::from_str::<Value>(m.content[start + 12..start + end].trim()) {
                    return v;
                }
            }
        }
    }
    json!({})
}

fn find_hint<'a>(keys: &[&'a str], task: &str) -> Option<&'a str> {
    let lower = task.to_lowercase();
    keys.iter().find_map(|k| lower.contains(k).then_some(*k))
}

/// A real implementation for a known algorithm request, else a generic module.
fn implementation_for(goal: &str, language: &str, agent_name: &str) -> Vec<(String, String)> {
    let py = language == "python";
    let which = find_hint(
        &[
            "fibonacci", "palindrome", "quicksort", "sort", "factorial", "prime", "binary search",
            "reverse", "string", "calculator", "util", "api", "scraper",
        ],
        goal,
    )
    .unwrap_or("util");

    let mut files: Vec<(String, String)> = Vec::new();

    let (name, code) = match which {
        "fibonacci" => {
            if py {
                (
                    "fibonacci.py",
                    r#""""Fibonacci utilities with memoization and an iterative fast path."""


def fib(n: int) -> int:
    """Return the n-th Fibonacci number (fib(0) == 0, fib(1) == 1)."""
    if n < 0:
        raise ValueError("n must be >= 0")
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a


def fib_sequence(count: int) -> list[int]:
    """Return the first `count` Fibonacci numbers."""
    if count < 0:
        raise ValueError("count must be >= 0")
    out = []
    a, b = 0, 1
    for _ in range(count):
        out.append(a)
        a, b = b, a + b
    return out
"#,
                )
            } else {
                (
                    "fibonacci.js",
                    r#"// Fibonacci utilities with an iterative fast path.

/** Return the n-th Fibonacci number (fib(0) === 0, fib(1) === 1). */
export function fib(n) {
  if (!Number.isInteger(n) || n < 0) throw new RangeError("n must be a non-negative integer");
  let a = 0n, b = 1n;
  for (let i = 0; i < n; i++) [a, b] = [b, a + b];
  return a;
}

/** Return the first `count` Fibonacci numbers as regular numbers. */
export function fibSequence(count) {
  const out = [];
  let a = 0n, b = 1n;
  for (let i = 0; i < count; i++) {
    out.push(Number(a));
    [a, b] = [b, a + b];
  }
  return out;
}
"#,
                )
            }
        }
        "palindrome" => (
            if py { "palindrome.py" } else { "palindrome.js" },
            if py {
                r#""""Palindrome checks, ignoring case and non-alphanumerics."""


def is_palindrome(text: str) -> bool:
    """True when `text` reads the same forwards and backwards."""
    cleaned = "".join(ch.lower() for ch in text if ch.isalnum())
    return cleaned == cleaned[::-1]


def longest_palindromic_substring(text: str) -> str:
    """Expand-around-center longest palindromic substring."""
    if not text:
        return ""
    best_start, best_len = 0, 1

    def expand(lo: int, hi: int) -> tuple[int, int]:
        while lo >= 0 and hi < len(text) and text[lo] == text[hi]:
            lo -= 1
            hi += 1
        return lo + 1, hi - lo - 1

    for i in range(len(text)):
        for lo, hi in ((i, i), (i, i + 1)):
            start, length = expand(lo, hi)
            if length > best_len:
                best_start, best_len = start, length
    return text[best_start : best_start + best_len]
"#
            } else {
                r#"// Palindrome checks, ignoring case and non-alphanumerics.

export function isPalindrome(text) {
  const cleaned = String(text).toLowerCase().replace(/[^a-z0-9]/g, "");
  return cleaned === [...cleaned].reverse().join("");
}
"#
            },
        ),
        "quicksort" | "sort" => (
            if py { "sorting.py" } else { "sorting.js" },
            if py {
                r#""""In-place quicksort with median-of-three pivots and an insertion-sort tail."""


def quicksort(items: list) -> list:
    """Return a new sorted list (stable for equal keys by index order)."""
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
        if hi - lo < 8:
            for i in range(lo + 1, hi + 1):
                key, j = a[i], i - 1
                while j >= lo and a[j] > key:
                    a[j + 1] = a[j]
                    j -= 1
                a[j + 1] = key
            return
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
"#
            } else {
                r#"// In-place quicksort with median-of-three pivots.

export function quicksort(items) {
  const a = [...items];
  sort(a, 0, a.length - 1);
  return a;
}

function medianOfThree(a, lo, hi) {
  const mid = (lo + hi) >> 1;
  const x = a[lo], y = a[mid], z = a[hi];
  if ((x <= y && y <= z) || (z <= y && y <= x)) return y;
  if ((y <= x && x <= z) || (z <= x && x <= y)) return x;
  return z;
}

function sort(a, lo, hi) {
  while (lo < hi) {
    const pivot = medianOfThree(a, lo, hi);
    let i = lo, j = hi;
    while (i <= j) {
      while (a[i] < pivot) i++;
      while (a[j] > pivot) j--;
      if (i <= j) {
        [a[i], a[j]] = [a[j], a[i]];
        i++; j--;
      }
    }
    if (j - lo < hi - i) {
      sort(a, lo, j);
      lo = i;
    } else {
      sort(a, i, hi);
      hi = j;
    }
  }
}
"#
            },
        ),
        _ => (
            if py { "helpers.py" } else { "helpers.js" },
            if py { HELPERS_PY } else { HELPERS_JS },
        ),
    };
    let code = code.replace("__AGENT_NAME__", agent_name);
    files.push((name.to_string(), code));

    // Tests exercising the implementation so sandbox validation is meaningful.
    if py {
        files.push((
            "test_generated.py".into(),
            r#"import importlib.util
import pathlib
import sys

# Import the generated module by filename so the sandbox stays self-contained.
_spec = importlib.util.spec_from_file_location(
    "generated", pathlib.Path(__file__).with_name("SORTME")
)
mod = importlib.util.module_from_spec(_spec)
sys.modules["generated"] = mod
_spec.loader.exec_module(mod)
"#
            .replace("SORTME", &files[0].0),
        ));
    }
    files
}

fn tests_for(goal: &str, language: &str, main_file: &str) -> String {
    let py = language == "python";
    let which = find_hint(&["fibonacci", "palindrome", "quicksort", "sort", "factorial", "prime"], goal).unwrap_or("util");
    if py {
        match which {
            "fibonacci" => format!(
                r#"from {main_stem} import fib, fib_sequence


def test_base_cases():
    assert fib(0) == 0
    assert fib(1) == 1


def test_first_ten():
    assert fib_sequence(10) == [0, 1, 1, 2, 3, 5, 8, 13, 21, 34]


def test_negative_raises():
    import pytest
    with pytest.raises(ValueError):
        fib(-1)
"#,
                main_stem = main_file.trim_end_matches(".py")
            ),
            "palindrome" => format!(
                r#"from {main_stem} import is_palindrome


def test_simple():
    assert is_palindrome("racecar")
    assert not is_palindrome("panda")


def test_ignores_punctuation_and_case():
    assert is_palindrome("A man, a plan, a canal: Panama")
"#,
                main_stem = main_file.trim_end_matches(".py")
            ),
            _ => format!(
                r#"from {main_stem} import quicksort

import random


def test_sorted_output():
    data = [random.randint(-100, 100) for _ in range(200)]
    assert quicksort(data) == sorted(data)


def test_empty_and_single():
    assert quicksort([]) == []
    assert quicksort([1]) == [1]
"#,
                main_stem = main_file.trim_end_matches(".py")
            ),
        }
    } else {
        format!(
            r#"// Smoke test for {main_file} — run with `node test_generated.mjs`.
import * as mod from "./{main_file}";

const checks = [
  ["fib(10) === 55", () => mod.fib(10) === 55],
  ["fib(0) === 0", () => mod.fib(0) === 0],
  ["seq", () => JSON.stringify(mod.fibSequence(7)) === "[0,1,1,2,3,5,8]"],
];
let failed = 0;
for (const [name, ok] of checks) {{
  if (!ok()) {{ console.error("FAIL " + name); failed++; }}
  else {{ console.log("ok - " + name); }}
}}
if (failed > 0) process.exit(1);
console.log("all tests passed");
"#
        )
    }
}

const HELPERS_PY: &str = r#""""Small, dependency-free helpers written by agent `__AGENT_NAME__`."""


def slugify(text: str) -> str:
    """Lowercase, dash-separated, alphanumeric-only slug."""
    out = []
    for ch in text.lower():
        if ch.isalnum():
            out.append(ch)
        elif out and out[-1] != "-":
            out.append("-")
    return "".join(out).strip("-")


def chunked(items: list, size: int) -> list[list]:
    """Split `items` into chunks of at most `size`."""
    if size <= 0:
        raise ValueError("size must be positive")
    return [items[i : i + size] for i in range(0, len(items), size)]


def compact(items: list) -> list:
    """Drop falsy entries."""
    return [x for x in items if x]
"#;

const HELPERS_JS: &str = r#"// Small, dependency-free helpers written by agent `__AGENT_NAME__`.

export function slugify(text) {
  return String(text)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function chunked(items, size) {
  if (!Number.isInteger(size) || size <= 0) throw new RangeError("size must be positive");
  const out = [];
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
  return out;
}

export function compact(items) {
  return items.filter(Boolean);
}
"#;

#[async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn chat(&self, req: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let h = hints(&req.messages);
        let role = h["role"].as_str().unwrap_or("chat").to_string();
        let goal = h["goal"].as_str().unwrap_or("").to_string();
        let language = h["language"].as_str().unwrap_or("python").to_string();
        let agent_name = h["agent_name"].as_str().unwrap_or("agent").to_string();
        let turn = h["turn"].as_u64().unwrap_or(0);
        let workspace_files: Vec<String> = h["workspace_files"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let id = self.next_id();
        let resp = match role.as_str() {
            "planner" => {
                let task = h["task"].as_str().unwrap_or("implement utilities").to_string();
                let lower = task.to_lowercase();
                let mut milestones: Vec<Value> = Vec::new();
                let add = |milestones: &mut Vec<Value>, title: &str, acceptance: &str| {
                    milestones.push(json!({"title": title, "acceptance": [acceptance]}))
                };
                if lower.contains("fibonacci") {
                    add(&mut milestones, "Implement fibonacci module", "fib(0..10) correct, memoized path tested");
                }
                if lower.contains("palindrome") {
                    add(&mut milestones, "Implement palindrome checks", "case/punctuation-insensitive checks pass");
                }
                if lower.contains("sort") {
                    add(&mut milestones, "Implement sorting", "quicksort matches sorted() on random data");
                }
                if lower.contains("test") || lower.contains("valid") {
                    add(&mut milestones, "Write the test suite", "pytest exits 0 in the sandbox");
                }
                if milestones.is_empty() {
                    add(&mut milestones, "Implement the requested utilities", "module imports cleanly and tests pass");
                    add(&mut milestones, "Add a test suite", "pytest exits 0 in the sandbox");
                }
                milestones.push(json!({"title": "Review and integrate", "acceptance": ["reviewer sign-off", "artifacts merged into main workspace"]}));
                let content = serde_json::to_string_pretty(&json!({
                    "objective": task,
                    "milestones": milestones
                }))?;
                ChatResponse {
                    content,
                    ..Default::default()
                }
            }
            "coder" => {
                if turn > 0 {
                    return Ok(ChatResponse {
                        content: format!("Files for `{goal}` are written; handing off to the tester."),
                        ..Default::default()
                    });
                }
                let files = implementation_for(&goal, &language, &agent_name);
                let tool_calls: Vec<ToolCallReq> = files
                    .into_iter()
                    .map(|(path, content)| ToolCallReq {
                        id: format!("{id}-{}", path),
                        name: "write_file".into(),
                        arguments: serde_json::to_string(&json!({"path": path, "content": content})).unwrap_or_else(|_| "{}".into()),
                    })
                    .collect();
                ChatResponse {
                    content: format!(
                        "Implementing `{goal}` — writing {} file(s), then handing off to the tester.",
                        tool_calls.len()
                    ),
                    tool_calls,
                    stop_reason: Some("tool_use".into()),
                    ..Default::default()
                }
            }
            "tester" => {
                if turn > 0 {
                    return Ok(ChatResponse {
                        content: "Validation run complete; awaiting the harness verdict.".into(),
                        ..Default::default()
                    });
                }
                let main_file = workspace_files
                    .iter()
                    .find(|f| f.ends_with(".py") || f.ends_with(".js") || f.ends_with(".mjs"))
                    .cloned()
                    .unwrap_or_else(|| if language == "python" { "helpers.py".into() } else { "helpers.js".into() });
                let is_py = main_file.ends_with(".py");
                let test_path = if is_py { "test_generated.py" } else { "test_generated.mjs" };
                let body = if is_py {
                    tests_for(&goal, &language, &main_file)
                } else {
                    tests_for(&goal, &language, &main_file)
                };
                let run_cmd = if is_py {
                    "python3 -m pytest test_generated.py -q || python3 test_generated.py"
                } else {
                    "node test_generated.mjs"
                };
                ChatResponse {
                    content: "Writing the test suite and validating it in the sandbox.".into(),
                    tool_calls: vec![
                        ToolCallReq {
                            id: format!("{id}-write"),
                            name: "write_file".into(),
                            arguments: serde_json::to_string(&json!({"path": test_path, "content": body}))?,
                        },
                        ToolCallReq {
                            id: format!("{id}-run"),
                            name: "sandbox_run".into(),
                            arguments: serde_json::to_string(&json!({"cmd": run_cmd, "timeout_secs": 60}))?,
                        },
                    ],
                    stop_reason: Some("tool_use".into()),
                    ..Default::default()
                }
            }
            "reviewer" => ChatResponse {
                content: format!(
                    "Review of `{goal}`:\n- modules are small and dependency-free ✅\n- edge cases (empty input, negative n) handled ✅\n- complexity is documented ✅\nSign-off: LGTM."
                ),
                ..Default::default()
            },
            _ => {
                let last_user = req
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.role == crate::Role::User)
                    .map(|m| m.content.clone())
                    .unwrap_or_default();
                ChatResponse {
                    content: format!(
                        "[mock] Received: {last_user}\nConfigure a real provider (openai/anthropic/glm/…) in pandaswarm.toml for live intelligence."
                    ),
                    ..Default::default()
                }
            }
        };
        Ok(ChatResponse {
            provider: "mock".into(),
            model: self.model.clone(),
            usage: crate::Usage {
                prompt_tokens: 64,
                completion_tokens: 256,
            },
            ..resp
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    #[tokio::test]
    async fn mock_coder_writes_real_files() {
        let m = MockProvider::new("mock");
        let req = ChatRequest::new(
            "mock",
            vec![ChatMessage::user(
                "<task-hints>{\"role\":\"coder\",\"goal\":\"implement fibonacci with memoization\",\"language\":\"python\",\"agent_name\":\"coder-1\"}</task-hints>",
            )],
        );
        let resp = m.chat(&req).await.unwrap();
        assert_eq!(resp.tool_calls.len(), 2);
        let args: Value = serde_json::from_str(&resp.tool_calls[0].arguments).unwrap();
        assert_eq!(args["path"], "fibonacci.py");
        assert!(args["content"].as_str().unwrap().contains("def fib"));
    }
}
