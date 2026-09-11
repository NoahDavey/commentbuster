//! Threshold evaluation. With no thresholds configured the tool reports and
//! says nothing about pass or fail.

use crate::analyze::Totals;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Thresholds {
    pub max_net: Option<i64>,
    pub max_ratio: Option<f64>,
    /// Net ceiling that applies only below `min_code_lines`, where the ratio
    /// has nothing to normalise against and is skipped. Complementary to
    /// `max_ratio`: each rule governs one side of the boundary, so a large,
    /// proportionately-commented diff is never flagged the way an
    /// unconditional `max_net` would flag it.
    pub max_net_small: Option<i64>,
    pub min_code_lines: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub rule: &'static str,
    pub limit: Value,
    pub actual: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkippedRule {
    pub rule: &'static str,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Gate {
    pub configured: bool,
    pub passed: bool,
    pub failures: Vec<Failure>,
    /// Rules that were configured but did not apply — recorded so a green
    /// result never quietly means "the gate never ran".
    pub skipped_rules: Vec<SkippedRule>,
}

pub fn evaluate(totals: &Totals, t: &Thresholds) -> Gate {
    let configured = t.max_net.is_some() || t.max_ratio.is_some() || t.max_net_small.is_some();
    let mut failures = Vec::new();
    let mut skipped_rules = Vec::new();
    let small = totals.code_lines_added < t.min_code_lines;

    if let Some(limit) = t.max_net {
        if totals.comment_lines_net > limit {
            failures.push(Failure {
                rule: "max_net",
                limit: json!(limit),
                actual: json!(totals.comment_lines_net),
            });
        }
    }

    if let Some(limit) = t.max_ratio {
        match totals.comment_ratio {
            None => skipped_rules.push(SkippedRule {
                rule: "max_ratio",
                reason: "diff adds no code lines".into(),
            }),
            Some(_) if small => {
                skipped_rules.push(SkippedRule {
                    rule: "max_ratio",
                    reason: format!(
                        "{} added code lines is below --min-code-lines {}",
                        totals.code_lines_added, t.min_code_lines
                    ),
                })
            }
            Some(actual) => {
                if actual > limit {
                    failures.push(Failure {
                        rule: "max_ratio",
                        limit: json!(limit),
                        actual: json!(actual),
                    });
                }
            }
        }
    }

    if let Some(limit) = t.max_net_small {
        if !small {
            skipped_rules.push(SkippedRule {
                rule: "max_net_small",
                reason: format!(
                    "{} added code lines is not below --min-code-lines {}",
                    totals.code_lines_added, t.min_code_lines
                ),
            });
        } else if totals.comment_lines_net > limit {
            failures.push(Failure {
                rule: "max_net_small",
                limit: json!(limit),
                actual: json!(totals.comment_lines_net),
            });
        }
    }

    Gate { configured, passed: failures.is_empty(), failures, skipped_rules }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(comment_added: u64, code_added: u64) -> Totals {
        Totals {
            comment_lines_added: comment_added,
            comment_lines_removed: 0,
            comment_lines_net: comment_added as i64,
            code_lines_added: code_added,
            code_lines_removed: 0,
            comment_ratio: crate::analyze::ratio(comment_added, code_added),
        }
    }

    /// The hook's configuration: ratio above the floor, net below it.
    fn complementary() -> Thresholds {
        Thresholds {
            max_net: None,
            max_ratio: Some(0.15),
            max_net_small: Some(5),
            min_code_lines: 20,
        }
    }

    fn rules(items: &[SkippedRule]) -> Vec<&'static str> {
        items.iter().map(|s| s.rule).collect()
    }

    #[test]
    fn below_the_floor_the_net_decides_and_the_ratio_steps_aside() {
        let g = evaluate(&totals(6, 1), &complementary());
        assert!(!g.passed);
        assert_eq!(g.failures[0].rule, "max_net_small");
        assert_eq!(g.failures[0].actual, json!(6));
        assert_eq!(rules(&g.skipped_rules), ["max_ratio"]);

        let g = evaluate(&totals(5, 1), &complementary());
        assert!(g.passed, "the limit is inclusive");
    }

    #[test]
    fn above_the_floor_the_ratio_decides_and_the_net_steps_aside() {
        // 6 comment lines on 50 code lines is 12%: within the ratio, and over
        // the small-commit net ceiling that must not apply here.
        let g = evaluate(&totals(6, 50), &complementary());
        assert!(g.passed, "{:?}", g.failures);
        assert_eq!(rules(&g.skipped_rules), ["max_net_small"]);

        let g = evaluate(&totals(10, 25), &complementary());
        assert!(!g.passed);
        assert_eq!(g.failures[0].rule, "max_ratio");
        assert_eq!(rules(&g.skipped_rules), ["max_net_small"]);
    }

    #[test]
    fn the_floor_itself_counts_as_large() {
        let g = evaluate(&totals(6, 20), &complementary());
        assert_eq!(rules(&g.skipped_rules), ["max_net_small"]);
        assert!(!g.passed, "6/20 is 30%, over the ratio");
        assert_eq!(g.failures[0].rule, "max_ratio");
    }

    #[test]
    fn a_comments_only_diff_is_small() {
        let g = evaluate(&totals(6, 0), &complementary());
        assert!(!g.passed);
        assert_eq!(g.failures[0].rule, "max_net_small");
        assert_eq!(g.skipped_rules[0].reason, "diff adds no code lines");
    }

    #[test]
    fn max_net_small_alone_counts_as_configured() {
        let t = Thresholds { max_net: None, max_ratio: None, max_net_small: Some(5), min_code_lines: 20 };
        let g = evaluate(&totals(0, 0), &t);
        assert!(g.configured);
        assert!(g.passed);
    }

    #[test]
    fn unconditional_max_net_still_applies_everywhere() {
        let t = Thresholds { max_net: Some(5), max_ratio: None, max_net_small: None, min_code_lines: 20 };
        let g = evaluate(&totals(6, 50), &t);
        assert_eq!(g.failures[0].rule, "max_net");
        assert!(g.skipped_rules.is_empty());
    }
}
