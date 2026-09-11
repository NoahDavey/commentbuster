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
use crate::git;
use anyhow::Result;
use serde_json::{json, Value};

pub enum Decision {
    Allow,
    Deny(String),
}

/// Decide the pending tool call described by `payload`.
///
/// Anything that is not a `git commit` is allowed without touching git. A
/// commit is gated on the index against HEAD, the same diff the commit will
/// record.
pub fn decide(payload: &str, thresholds: &Thresholds) -> Result<Decision> {
    let Some(command) = command_from_payload(payload) else {
        return Ok(Decision::Allow);
    };
    if !is_git_commit(&command) {
        return Ok(Decision::Allow);
    }

    let root = git::repo_root()?;
    let analysis = analyze(&root, &Mode::Staged, &[])?;
    let gate = gate::evaluate(&analysis.totals, thresholds);
    if gate.passed {
        return Ok(Decision::Allow);
    }
    Ok(Decision::Deny(deny_reason(&analysis, &gate)))
}

/// The shell command a Bash tool call is about to run, if the payload has one.
pub fn command_from_payload(payload: &str) -> Option<String> {
    let v: Value = serde_json::from_str(payload).ok()?;
    v.get("tool_input")?.get("command")?.as_str().map(str::to_owned)
}

/// Whether the command runs `git commit`.
///
/// `git` must start a shell segment and `commit` must follow as a whole word
/// in the same segment: `git add -A && git commit -m x` and `$(git commit)`
/// match, while `echo "run git commit"` and a commit message that mentions
/// the words do not.
pub fn is_git_commit(command: &str) -> bool {
    command
        .split(|c| matches!(c, ';' | '&' | '|' | '(' | '\n'))
        .any(|segment| match segment.trim_start().strip_prefix("git") {
            Some(rest) if !rest.starts_with(is_word_char) => has_word(rest, "commit"),
            _ => false,
        })
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn has_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(i, _)| {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

/// The denial Claude Code shows the model: which rule failed and by how much,
/// the files carrying the growth, and what to do about it.
pub fn deny_reason(analysis: &Analysis, gate: &Gate) -> String {
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

    format!(
        "Comment budget exceeded: this commit {headline}.\n\n{}\n\n\
         Revise the comments before committing. Cut the ones that restate the code: \
         a doc block that repeats the signature, a line narrating the statement below it, \
         commented-out code, a comment that names what a well-named function already says. \
         Keep the ones that explain why a choice was made, record a constraint or invariant, \
         or warn about something non-obvious. Then re-stage the files and commit again.",
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
    fn commits_are_recognised_wherever_they_sit_in_the_command() {
        for cmd in [
            "git commit -m 'x'",
            "git commit",
            "  git commit --amend",
            "git add -A && git commit -m x",
            "git add -A; git commit -q -m x",
            "git -C repo commit -m x",
            "(git commit -q)",
            "echo done | git commit -F -",
            "cd sub\ngit commit -m x",
            "git commit -m \"$(cat msg)\"",
        ] {
            assert!(is_git_commit(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn mentions_and_lookalikes_are_not_commits() {
        for cmd in [
            "",
            "git",
            "git log",
            "git status && cargo test",
            "echo git commit",
            "echo 'remember to git commit'",
            "gitk commit",
            "git committed",
            "git\ncommit",
            "cargo test",
        ] {
            assert!(!is_git_commit(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn the_command_comes_from_tool_input() {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m x", "description": "commit" }
        })
        .to_string();
        assert_eq!(command_from_payload(&payload).as_deref(), Some("git commit -m x"));

        assert_eq!(command_from_payload("not json"), None);
        assert_eq!(command_from_payload("{}"), None);
        assert_eq!(command_from_payload(r#"{"tool_input":{"command":7}}"#), None);
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
