//! Threshold evaluation. With no thresholds configured the tool reports and
//! says nothing about pass or fail.

use crate::analyze::Totals;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Thresholds {
    pub max_net: Option<i64>,
    pub max_ratio: Option<f64>,
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
    let configured = t.max_net.is_some() || t.max_ratio.is_some();
    let mut failures = Vec::new();
    let mut skipped_rules = Vec::new();

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
            Some(_) if totals.code_lines_added < t.min_code_lines => {
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

    Gate { configured, passed: failures.is_empty(), failures, skipped_rules }
}
