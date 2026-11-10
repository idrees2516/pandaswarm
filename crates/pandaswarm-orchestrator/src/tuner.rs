//! HarnessPro-style tuner: aggregates `TestReport`s across runs, finds
//! *recurring* failure modes and escalates them into harness-patch
//! suggestions — diagnostics become harness changes, not just error text.

use pandaswarm_core::TestReport;
use pandaswarm_sandbox::HarnessSuggestion;

/// Aggregate recurring failure patterns across reports.
/// A pattern seen in ≥ 2 runs, or in the latest run with priority 5, gets a
/// suggestion with escalated priority.
pub fn aggregate(reports: &[TestReport]) -> Vec<HarnessSuggestion> {
    let mut counts: std::collections::HashMap<String, (u32, u64)> = Default::default(); // pattern -> (occurrences, max_priority)
    for r in reports {
        if r.is_green() {
            continue;
        }
        for p in &r.failure_patterns {
            let entry = counts.entry(p.clone()).or_insert((0, 0));
            entry.0 += 1;
        }
    }
    let mut out: Vec<HarnessSuggestion> = counts
        .into_iter()
        .filter(|(_, (occurrences, _))| *occurrences >= 1)
        .map(|(pattern, (occurrences, _))| {
            let base = pandaswarm_sandbox::suggest_harness_patches(
                &TestReport {
                    passed: 1,
                    failed: occurrences,
                    errors: 0,
                    skipped: 0,
                    duration_ms: 0,
                    backend: "aggregate".into(),
                    raw_output: String::new(),
                    failure_patterns: vec![pattern.clone()],
                },
                &[],
            );
            let mut s = base
                .into_iter()
                .find(|s| s.pattern == pattern)
                .unwrap_or(HarnessSuggestion {
                    pattern: pattern.clone(),
                    suggestion: format!("failure pattern `{pattern}` seen {occurrences}× — inspect and tighten the harness"),
                    priority: 2,
                });
            // escalate recurring patterns
            if occurrences >= 2 {
                s.priority = 5;
                s.suggestion = format!("[recurring ×{occurrences}] {}", s.suggestion);
            }
            s
        })
        .collect();
    out.sort_by(|a, b| b.priority.cmp(&a.priority));
    out.truncate(5);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red(patterns: Vec<String>) -> TestReport {
        TestReport {
            passed: 0,
            failed: 2,
            errors: 0,
            skipped: 0,
            duration_ms: 10,
            backend: "test".into(),
            raw_output: String::new(),
            failure_patterns: patterns,
        }
    }

    #[test]
    fn recurring_patterns_escalate() {
        let reports = vec![
            red(vec!["assertion".into(), "timeout".into()]),
            red(vec!["assertion".into()]),
        ];
        let suggestions = aggregate(&reports);
        let top = suggestions.first().unwrap();
        assert_eq!(top.pattern, "assertion");
        assert_eq!(top.priority, 5);
        assert!(top.suggestion.contains("recurring"));
    }
}
