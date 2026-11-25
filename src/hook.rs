//! The Claude Code `PreToolUse` hook behind `commentbuster claude-gate`.
//!
//! Claude Code pipes a JSON description of the pending tool call to the hook's
//! stdin. The hook allows by printing nothing and exiting 0, or denies by
//! printing a `hookSpecificOutput` decision and exiting 0. It never prints an
//! explicit "allow": that would skip Claude Code's own permission prompt for
//! the command.
//!
//! Every failure of the gate itself allows. A gate that blocks commits because
//! of its own problems is a gate people turn off.

use crate::analyze::{analyze, Analysis, Mode};
use crate::gate::{self, Gate, Thresholds};
use crate::git::{AmendBase, Repo};
use crate::git_args;
use crate::shell::{self, GitCall};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub enum Decision {
    Allow,
    Deny(String),
}

/// Decide the pending tool call described by `payload`.
///
/// Anything that does not run a `git commit` is allowed without touching git.
/// A commit is gated on the index as it stands now, so a call that would also
/// change the index is sent back to be split.
pub fn decide(payload: &str, thresholds: &Thresholds) -> Result<Decision> {
    let Some(call) = ToolCall::parse(payload) else {
        return Ok(Decision::Allow);
    };
    if !call.command.contains("commit") {
        return Ok(Decision::Allow);
    }
    let cwd = match call.cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir()?,
    };

    let calls = shell::git_calls(&call.command, &cwd);
    let commits: Vec<PendingCommit> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.subcommand == "commit")
        .map(|(at, call)| PendingCommit { at, call, args: git_args::parse_commit(&call.args) })
        .filter(|c| !c.args.no_commit)
        .collect();

    let mut failure = None;
    for commit in &commits {
        let Some(dir) = &commit.call.dir else { continue };
        match check(commit, dir, &calls, commits.len(), thresholds) {
            Ok(Decision::Allow) => {}
            Ok(deny) => return Ok(deny),
            Err(e) => failure = Some(e),
        }
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(Decision::Allow),
    }
}

struct PendingCommit<'a> {
    /// Position among the git commands of the call.
    at: usize,
    call: &'a GitCall,
    args: git_args::Commit,
}

fn check(
    commit: &PendingCommit,
    dir: &Path,
    calls: &[GitCall],
    commits: usize,
    thresholds: &Thresholds,
) -> Result<Decision> {
    let repo = Repo::discover(dir)?.at_root();
    if repo.operation_in_progress()? {
        return Ok(Decision::Allow);
    }
    if let Some(cause) = index_not_final(commit, calls, commits) {
        return Ok(Decision::Deny(split_reason(&cause)));
    }

    let mode = if commit.args.amend {
        match repo.amend_base()? {
            AmendBase::Merge => return Ok(Decision::Allow),
            AmendBase::Commit(base) => Mode::StagedFrom(base),
        }
    } else {
        Mode::Staged
    };
    let analysis = analyze(&repo, &mode, &[])?;
    let gate = gate::evaluate(&analysis.totals, thresholds);
    if gate.passed {
        return Ok(Decision::Allow);
    }
    Ok(Decision::Deny(deny_reason(&analysis, &gate, commit.args.amend)))
}

/// Why the index the hook sees is not what this commit will record.
fn index_not_final(commit: &PendingCommit, calls: &[GitCall], commits: usize) -> Option<String> {
    if commits > 1 {
        return Some("the call runs more than one `git commit`".into());
    }
    if let Some(stages) = commit.args.stages {
        return Some(stages.into());
    }
    let earlier = calls.iter().enumerate().find(|(at, call)| {
        // Later in the same loop body is before the next iteration's commit.
        let runs_before = *at < commit.at
            || call.loops.iter().any(|l| commit.call.loops.contains(l));
        runs_before && git_args::changes_index(&call.subcommand, &call.args)
    });
    earlier.map(|(_, call)| {
        format!("`git {}` changes the index in the same call as the commit", call.subcommand)
    })
}

/// The Bash tool call about to run.
struct ToolCall {
    command: String,
    cwd: Option<PathBuf>,
}

impl ToolCall {
    fn parse(payload: &str) -> Option<ToolCall> {
        let v: Value = serde_json::from_str(payload).ok()?;
        Some(ToolCall {
            command: v.get("tool_input")?.get("command")?.as_str()?.to_owned(),
            cwd: v.get("cwd").and_then(Value::as_str).map(PathBuf::from),
        })
    }
}

/// The denial for a commit the hook cannot check before the command runs.
fn split_reason(cause: &str) -> String {
    format!(
        "The comment budget is checked against the index before this command runs, and {cause}. \
         Stage in one Bash call, then run plain `git commit` in the next: \
         no -a, -p, -i, -o or paths, and one commit per call."
    )
}

/// The denial Claude Code shows the model: which rule failed and by how much,
/// the files carrying the growth, and what to do about it.
pub fn deny_reason(analysis: &Analysis, gate: &Gate, amend: bool) -> String {
    let t = &analysis.totals;
    let failure = gate.failures.first();
    let headline = match failure.map(|f| f.rule) {
        Some("max_ratio") => format!(
            "adds {} comment lines against {} code lines: {:.0}%, over the {:.0}% ceiling",
            t.comment_lines_added,
            t.code_lines_added,
            t.comment_ratio.unwrap_or(0.0) * 100.0,
            failure.and_then(|f| f.limit.as_f64()).unwrap_or(0.0) * 100.0,
        ),
        Some("max_net_small") => format!(
            "adds {} net comment lines ({} added, {} removed) on only {} added code lines, \
             over the ceiling of {} for a commit this small",
            t.comment_lines_net,
            t.comment_lines_added,
            t.comment_lines_removed,
            t.code_lines_added,
            failure.and_then(|f| f.limit.as_i64()).unwrap_or(0),
        ),
        _ => format!(
            "adds {} net comment lines ({} added, {} removed), over the ceiling of {}",
            t.comment_lines_net,
            t.comment_lines_added,
            t.comment_lines_removed,
            failure.map(|f| f.limit.to_string()).unwrap_or_default(),
        ),
    };

    let worst: Vec<String> = analysis
        .files
        .iter()
        .filter(|f| f.comment_lines_net > 0)
        .take(6)
        .map(|f| format!("  {}  +{}", f.path, f.comment_lines_net))
        .collect();

    let subject = if amend {
        "the amended commit (what HEAD already has plus the index)"
    } else {
        "this commit"
    };
    format!(
        "Comment budget exceeded: {subject} {headline}.\n\n{}\n\n\
         Revise the comments before committing. Cut the ones that restate the code: \
         a doc block that repeats the signature, a line narrating the statement below it, \
         commented-out code, a comment that names what a well-named function already says. \
         Keep the ones that explain why a choice was made, record a constraint or invariant, \
         or warn about something non-obvious. Then re-stage the files and commit again, \
         in separate Bash calls.\n\n\
         Trim to budget and commit. Do not bypass the gate. \
         If a comment is essential for an extremely strong reason and does not fit, \
         leave it out and report the file, the exact comment and the reason for a person \
         to review: in the pull request description, marked as needing human eyes, \
         or in the final message if there is no PR.",
        worst.join("\n")
    )
}

/// The stdout that makes Claude Code refuse the tool call and show `reason`
/// to the model.
pub fn deny_output(reason: &str) -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_and_directory_come_from_the_payload() {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "cwd": "/repo",
            "tool_input": { "command": "git commit -m x", "description": "commit" }
        })
        .to_string();
        let call = ToolCall::parse(&payload).unwrap();
        assert_eq!(call.command, "git commit -m x");
        assert_eq!(call.cwd, Some(PathBuf::from("/repo")));

        assert!(ToolCall::parse("not json").is_none());
        assert!(ToolCall::parse("{}").is_none());
        assert!(ToolCall::parse(r#"{"tool_input":{"command":7}}"#).is_none());
    }

    #[test]
    fn deny_output_is_a_pretooluse_decision() {
        let v: Value = serde_json::from_str(&deny_output("why")).unwrap();
        let out = &v["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], "PreToolUse");
        assert_eq!(out["permissionDecision"], "deny");
        assert_eq!(out["permissionDecisionReason"], "why");
    }
}
