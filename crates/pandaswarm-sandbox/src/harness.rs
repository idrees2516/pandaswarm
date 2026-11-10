//! The validation harness: parse raw sandbox output into a structured
//! `TestReport`, recognise failure signatures, and propose harness patches
//! for recurring failure modes (the HarnessPro loop).

use pandaswarm_core::TestReport;
use serde::{Deserialize, Serialize};

/// Parse `stdout + stderr + exit code` into counts + failure patterns.
/// Understands pytest, TAP, node runners, go test and generic "N passed"
/// lines; anything unrecognized still yields a green/red verdict from the
/// exit code.
pub fn parse_test_report(result: &pandaswarm_core::ExecResult) -> TestReport {
    let raw = format!("{}\n{}", result.stdout, result.stderr);
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut errors = 0u32;
    let mut skipped = 0u32;

    // pytest / jest / generic: "5 passed", "3 failed", "2 errors", "1 skipped"
    let mut passed = count_word(&raw, "passed");
    let mut failed = count_word(&raw, "failed");
    let mut errors = count_word(&raw, "error");
    let mut skipped = count_word(&raw, "skipped");

    // TAP: "ok 3", "not ok 4"
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("ok ") && !t.starts_with("not ok") {
            passed += 1;
        } else if t.starts_with("not ok") {
            failed += 1;
        }
    }
    // node smoke runner: "ok - name" lines counted as passed by TAP above;
    // bare "FAIL " lines:
    failed += raw.lines().filter(|l| l.trim_start().starts_with("FAIL ")).count() as u32;
    // go test
    failed += raw.lines().filter(|l| l.starts_with("--- FAIL:")).count() as u32;
    passed += raw.lines().filter(|l| l.starts_with("--- PASS:")).count() as u32;

    // If nothing was parsed, derive a single green/red from the exit code.
    if passed + failed + errors + skipped == 0 {
        if result.exit_code == 0 {
            passed = 1;
        } else {
            failed = 1;
        }
    }

    let failure_patterns = collect_failure_patterns(&raw);
    TestReport {
        passed,
        failed,
        errors,
        skipped,
        duration_ms: result.duration_ms,
        backend: result.backend.clone(),
        raw_output: truncate(&raw, 8_000),
        failure_patterns,
    }
}

fn count_word(hay: &str, word: &str) -> u32 {
    let mut total = 0u32;
    for line in hay.lines() {
        let l = line.trim();
        // match "5 passed", "3 failed, 5 passed", "1 error", "2 errors"
        let lower = l.to_lowercase();
        let mut chars = lower.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c.is_ascii_digit() {
                let start = i;
                let mut end = i;
                while let Some(&(j, cj)) = chars.peek() {
                    if cj.is_ascii_digit() {
                        end = j;
                        chars.next();
                    } else {
                        break;
                    }
                }
                let num: u32 = lower[start..=end].parse().unwrap_or(0);
                let after = &lower[end + 1..];
                let after = after.trim_start_matches([' ', ',']);
                if after.starts_with(word)
                    && (after.len() == word.len()
                        || after.as_bytes().get(word.len()).map(|b| !b.is_ascii_alphanumeric()).unwrap_or(true))
                {
                    total += num;
                }
            }
        }
    }
    total
}

const KNOWN_FAILURE_PATTERNS: &[(&str, &str)] = &[
    ("AssertionError", "assertion"),
    ("ModuleNotFoundError", "missing_module"),
    ("ImportError", "import_error"),
    ("SyntaxError", "syntax_error"),
    ("NameError", "name_error"),
    ("TypeError", "type_error"),
    ("TimeoutError", "timeout"),
    ("ConnectionError", "network"),
    ("No space left on device", "disk_full"),
    ("permission denied", "permissions"),
    ("Segmentation fault", "segfault"),
    ("exit status 1", "exit_nonzero"),
];

fn collect_failure_patterns(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (needle, label) in KNOWN_FAILURE_PATTERNS {
        if raw.contains(needle) {
            out.push(label.to_string());
        }
    }
    out.sort();
    out.dedup();
    out.truncate(6);
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}

/// A proposed harness patch (HarnessPro-style: recurring diagnostics become
/// harness changes, not just error messages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessSuggestion {
    pub pattern: String,
    pub suggestion: String,
    pub priority: u8, // 0..=5, 5 = do it now
}

/// Suggest harness patches from a test report + the artifacts involved.
pub fn suggest_harness_patches(report: &TestReport, artifact_paths: &[String]) -> Vec<HarnessSuggestion> {
    let mut out = Vec::new();
    for p in &report.failure_patterns {
        let s = match p.as_str() {
            "assertion" => HarnessSuggestion {
                pattern: p.clone(),
                suggestion: format!(
                    "Assertion failures in {} — add property-based cases around edge inputs and tighten fixtures before the next agent run.",
                    artifact_paths.first().cloned().unwrap_or_else(|| "the suite".into())
                ),
                priority: 4,
            },
            "missing_module" | "import_error" => HarnessSuggestion {
                pattern: p.clone(),
                suggestion: "Declare the dependency in the manifest (requirements.txt / package.json) and re-run; the harness can pin the version.".into(),
                priority: 5,
            },
            "timeout" => HarnessSuggestion {
                pattern: p.clone(),
                suggestion: "Raise the per-test timeout or split long-running cases into background jobs the swarm can await.".into(),
                priority: 3,
            },
            "syntax_error" => HarnessSuggestion {
                pattern: p.clone(),
                suggestion: "Insert a fast formatter/parser gate (ruff/biome) before tests so syntax errors never burn a sandbox run.".into(),
                priority: 5,
            },
            "network" => HarnessSuggestion {
                pattern: p.clone(),
                suggestion: "Network is disabled in the sandbox profile — either allow net for this agent or mock the endpoint with a recorded fixture.".into(),
                priority: 3,
            },
            "disk_full" => HarnessSuggestion {
                pattern: p.clone(),
                suggestion: "Clean build caches between runs (harness pre-step) or mount a larger scratch dir.".into(),
                priority: 2,
            },
            "permissions" => HarnessSuggestion {
                pattern: p.clone(),
                suggestion: "chmod/chown the workspace in the harness setup step; sandboxes run as an unprivileged user by design.".into(),
                priority: 3,
            },
            _ => continue,
        };
        out.push(s);
    }
    // Mixed pass/fail → flakiness hint
    if report.passed > 0 && report.failed > 0 {
        out.push(HarnessSuggestion {
            pattern: "flaky_mix".into(),
            suggestion: format!(
                "{} passed / {} failed in one run — treat as flaky: mark, retry once, and only then fail the agent turn.",
                report.passed, report.failed
            ),
            priority: 2,
        });
    }
    out.sort_by(|a, b| b.priority.cmp(&a.priority));
    out.truncate(3);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(stdout: &str, code: i32) -> pandaswarm_core::ExecResult {
        pandaswarm_core::ExecResult {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: code,
            timed_out: false,
            duration_ms: 420,
            backend: "test".into(),
        }
    }

    #[test]
    fn parses_pytest_summary() {
        let r = exec("=== FAILURES ===\nAssertionError: boom\n2 failed, 5 passed, 1 skipped in 0.10s", 1);
        let rep = parse_test_report(&r);
        assert_eq!(rep.passed, 5);
        assert_eq!(rep.failed, 2);
        assert_eq!(rep.skipped, 1);
        assert!(rep.failure_patterns.contains(&"assertion".to_string()));
        assert!(!rep.is_green());
    }

    #[test]
    fn parses_tap_and_suggests() {
        let r = exec("ok - fib(10) === 55\nnot ok - fib(0) === 0\nall tests passed", 1);
        let rep = parse_test_report(&r);
        assert!(rep.failed >= 1);
        let suggestions = suggest_harness_patches(&rep, &["fib.js".into()]);
        assert!(!suggestions.is_empty());
    }

    #[test]
    fn green_when_exit_zero() {
        let rep = parse_test_report(&exec("no counts at all", 0));
        assert!(rep.is_green());
        assert_eq!(rep.passed, 1);
    }
}
