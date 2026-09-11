//! Output rendering. JSON is the default because CI is the main consumer;
//! `--pretty` is for a human reading a failure.

use crate::analyze::{Analysis, Mode};
use crate::gate::Gate;
use crate::git::short_sha;
use serde::Serialize;
use std::fmt::Write as _;

#[derive(Serialize)]
pub struct Report {
    #[serde(flatten)]
    pub analysis: Analysis,
    pub gate: Gate,
    pub duration_ms: u64,
}

pub fn json(report: &Report) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

struct Style {
    on: bool,
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }
    /// Positive net comment growth is the thing being hunted, so it reads red;
    /// a net reduction reads green.
    ///
    /// Padding happens before painting: `{:>5}` applied to an already-coloured
    /// string counts the escape sequences toward the width and breaks the
    /// columns.
    fn net(&self, n: i64, width: usize) -> String {
        let s = format!("{:>width$}", format!("{n:+}"));
        if n > 0 {
            self.red(&s)
        } else if n < 0 {
            self.green(&s)
        } else {
            self.dim(&s)
        }
    }

    fn added(&self, n: u64, width: usize) -> String {
        self.red(&format!("{:>width$}", format!("+{n}")))
    }

    fn removed(&self, n: u64, width: usize) -> String {
        self.green(&format!("{:>width$}", format!("-{n}")))
    }
}

fn header(mode: &Mode, a: &Analysis) -> String {
    match mode {
        Mode::Worktree => format!(
            "working tree vs HEAD ({})",
            a.range.base_sha.as_deref().map(short_sha).unwrap_or_default()
        ),
        Mode::Staged => format!(
            "index vs HEAD ({})",
            a.range.base_sha.as_deref().map(short_sha).unwrap_or_default()
        ),
        Mode::Range { base_ref, head_ref, base_sha, merge_base: true, .. } => format!(
            "{base_ref}...{head_ref} (merge-base {})",
            short_sha(base_sha)
        ),
        Mode::Range { base_ref, head_ref, .. } => {
            format!("{base_ref}..{head_ref} (direct)")
        }
    }
}

fn pct(r: Option<f64>) -> String {
    match r {
        Some(v) => format!("{:.1}%", v * 100.0),
        None => "n/a".to_string(),
    }
}

pub fn pretty(report: &Report, mode: &Mode, color: bool, top: usize) -> String {
    let s = Style { on: color };
    let a = &report.analysis;
    let t = &a.totals;
    let mut o = String::new();

    let _ = writeln!(
        o,
        "\n{}  {}\n",
        s.bold("commentbuster"),
        s.cyan(&header(mode, a))
    );
    let _ = writeln!(
        o,
        "  comment lines  {} {}   net {}",
        s.added(t.comment_lines_added, 6),
        s.removed(t.comment_lines_removed, 6),
        s.bold(&s.net(t.comment_lines_net, 0)),
    );
    let _ = writeln!(
        o,
        "  code lines     {:>6} {:>6}   ratio {}",
        format!("+{}", t.code_lines_added),
        format!("-{}", t.code_lines_removed),
        s.bold(&pct(t.comment_ratio)),
    );

    if a.files.is_empty() {
        let _ = writeln!(o, "\n  {}", s.dim("no parseable files changed"));
    } else {
        let shown = if top == 0 { a.files.len() } else { top.min(a.files.len()) };
        let width = a.files[..shown]
            .iter()
            .map(|f| f.path.len())
            .max()
            .unwrap_or(4)
            .clamp(4, 62);
        let _ = writeln!(
            o,
            "\n  {}",
            s.dim(&format!(
                "{:<width$}  {:>5} {:>5} {:>5}  {}",
                "path", "net", "+", "-", "lang"
            ))
        );
        for f in &a.files[..shown] {
            let path = if f.path.len() > width {
                format!("…{}", &f.path[f.path.len() - width + 1..])
            } else {
                f.path.clone()
            };
            let renamed = match &f.previous_path {
                Some(p) => s.dim(&format!("  (from {p})")),
                None => String::new(),
            };
            let _ = writeln!(
                o,
                "  {:<width$}  {} {:>5} {:>5}  {}{}",
                path,
                s.net(f.comment_lines_net, 5),
                f.comment_lines_added,
                f.comment_lines_removed,
                s.dim(f.language),
                renamed,
            );
        }
        if shown < a.files.len() {
            let _ = writeln!(
                o,
                "  {}",
                s.dim(&format!(
                    "… and {} more file{} (--top 0 for all; JSON always has every file)",
                    a.files.len() - shown,
                    if a.files.len() - shown == 1 { "" } else { "s" }
                ))
            );
        }
    }

    if a.skipped.count > 0 {
        let mut parts = Vec::new();
        for (n, label) in [
            (a.skipped.unsupported, "unsupported"),
            (a.skipped.parse_error, "parse error"),
            (a.skipped.binary, "binary"),
            (a.skipped.too_large, "too large"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {label}"));
            }
        }
        let _ = writeln!(
            o,
            "\n  {}",
            s.dim(&format!(
                "{} file{} skipped ({})",
                a.skipped.count,
                if a.skipped.count == 1 { "" } else { "s" },
                parts.join(", ")
            ))
        );
    }

    let g = &report.gate;
    for sk in &g.skipped_rules {
        let _ = writeln!(
            o,
            "\n  {}  --{} not applied: {}",
            s.dim("SKIP"),
            sk.rule.replace('_', "-"),
            s.dim(&sk.reason)
        );
    }
    if g.configured {
        if g.passed {
            let _ = writeln!(o, "\n  {}  within budget", s.green("PASS"));
        } else {
            for f in &g.failures {
                let (actual, limit) = match f.rule {
                    "max_ratio" => (
                        pct(f.actual.as_f64()),
                        pct(f.limit.as_f64()),
                    ),
                    _ => (f.actual.to_string(), f.limit.to_string()),
                };
                let _ = writeln!(
                    o,
                    "\n  {}  {} {} exceeds --{} {}",
                    s.red("FAIL"),
                    if f.rule == "max_ratio" { "ratio" } else { "net" },
                    s.bold(&actual),
                    f.rule.replace('_', "-"),
                    limit
                );
            }
        }
    }

    let _ = writeln!(o, "\n  {}", s.dim(&format!("{} ms", report.duration_ms)));
    o
}
