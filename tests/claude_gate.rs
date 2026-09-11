//! `commentbuster claude-gate`, the Claude Code PreToolUse hook, driven through
//! the real binary with a payload on stdin the way Claude Code runs it.

mod common;
use common::Repo;
use serde_json::{json, Value};
use std::path::Path;

const PLAIN: &str = "package demo;

public class A {
  int add(int a, int b) {
    return a + b;
  }
}
";

/// 3 comment lines, no code lines added: within the small-commit ceiling of 5.
const THREE_COMMENTS: &str = "package demo;

// f1
// f2
// f3
public class A {
  int add(int a, int b) {
    return a + b;
  }
}
";

/// 6 comment lines against 1 code line: over the small-commit ceiling of 5.
const COMMENTED: &str = "package demo;

/**
 * Adds two numbers together.
 *
 * @param a the first number
 * @param b the second number
 * @return the sum
 */
public class A {
  // Add the two numbers
  int add(int a, int b) {
    return a + b; // return the sum
  }
}
";

fn payload(command: &str) -> String {
    json!({
        "session_id": "s",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command, "description": "test" }
    })
    .to_string()
}

/// A repo whose index holds the COMMENTED change over a PLAIN HEAD.
fn staged_over_budget(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("src/A.java", PLAIN);
    r.commit("base");
    r.write("src/A.java", COMMENTED);
    r.git(&["add", "-A"]);
    r
}

/// A class with `comments` line comments and at least `methods` code lines.
fn big_file(methods: usize, comments: usize) -> String {
    let mut s = String::from("package demo;\n\npublic class Big {\n");
    for i in 0..comments {
        s.push_str(&format!("  // note {i}\n"));
    }
    for i in 0..methods {
        s.push_str(&format!("  int f{i}() {{ return {i}; }}\n"));
    }
    s.push_str("}\n");
    s
}

fn decision(stdout: &str) -> Value {
    let v: Value =
        serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad json ({e}): {stdout:?}"));
    v["hookSpecificOutput"].clone()
}

fn reason(stdout: &str) -> String {
    let d = decision(stdout);
    assert_eq!(d["permissionDecision"], "deny", "{stdout}");
    d["permissionDecisionReason"].as_str().unwrap().to_string()
}

#[test]
fn denies_an_over_commented_small_commit() {
    let r = staged_over_budget("hook_deny");
    let (code, out, err) = r.run_hook(&["claude-gate"], &payload("git commit -m 'x'"), &[]);
    assert_eq!(code, 0, "{err}");

    let d = decision(&out);
    assert_eq!(d["hookEventName"], "PreToolUse");
    assert_eq!(d["permissionDecision"], "deny");
    let why = d["permissionDecisionReason"].as_str().unwrap();
    assert!(why.contains("adds 6 net comment lines (6 added, 0 removed) on only 1 added code lines"), "{why}");
    assert!(why.contains("ceiling of 5"), "{why}");
    assert!(why.contains("src/A.java  +6"), "{why}");
    assert!(why.contains("re-stage the files and commit again"), "{why}");
}

#[test]
fn allows_a_small_commit_within_budget_by_printing_nothing() {
    let r = Repo::new("hook_allow_small");
    r.write("src/A.java", PLAIN);
    r.commit("base");
    r.write("src/A.java", THREE_COMMENTS);
    r.git(&["add", "-A"]);

    let (code, out, _) = r.run_hook(&["claude-gate"], &payload("git commit -m x"), &[]);
    assert_eq!(code, 0);
    // An explicit "allow" decision would skip Claude Code's permission prompt.
    assert_eq!(out, "");
}

#[test]
fn a_large_proportionate_commit_passes_where_a_flat_net_ceiling_would_fail() {
    let r = Repo::new("hook_large_ok");
    r.write("src/A.java", PLAIN);
    r.commit("base");
    r.write("src/Big.java", &big_file(50, 6));
    r.git(&["add", "-A"]);

    let (code, out, _) = r.run_hook(&["claude-gate"], &payload("git commit -m x"), &[]);
    assert_eq!((code, out.as_str()), (0, ""));

    // The same index breaches a flat --max-net 5. That gap is why the hook's
    // net rule applies only below the code floor.
    let (code, v) = r.run(&["--staged", "--max-net", "5"]);
    assert_eq!(code, 1);
    assert_eq!(v["totals"]["comment_lines_net"], 6);
    assert!(v["totals"]["code_lines_added"].as_u64().unwrap() >= 20);
}

#[test]
fn a_large_commit_over_the_ratio_is_denied() {
    let r = Repo::new("hook_large_ratio");
    r.write("src/A.java", PLAIN);
    r.commit("base");
    r.write("src/Big.java", &big_file(25, 12));
    r.git(&["add", "-A"]);

    let (_, out, _) = r.run_hook(&["claude-gate"], &payload("git commit -m x"), &[]);
    let why = reason(&out);
    assert!(why.contains("adds 12 comment lines against"), "{why}");
    assert!(why.contains("over the 15% ceiling"), "{why}");
    assert!(why.contains("src/Big.java  +12"), "{why}");
}

#[test]
fn only_a_real_git_commit_is_checked() {
    let r = staged_over_budget("hook_filter");

    for cmd in [
        "cargo test",
        "git status",
        "git log --oneline -3",
        "echo 'remember to git commit'",
    ] {
        let (code, out, _) = r.run_hook(&["claude-gate"], &payload(cmd), &[]);
        assert_eq!((code, out.as_str()), (0, ""), "{cmd:?} must pass through untouched");
    }

    for cmd in [
        "git add -A && git commit -m 'x'",
        "cd src && git commit -q -m x",
        "git -C . commit --amend --no-edit",
    ] {
        let (_, out, _) = r.run_hook(&["claude-gate"], &payload(cmd), &[]);
        assert_eq!(decision(&out)["permissionDecision"], "deny", "{cmd:?} is a commit");
    }
}

#[test]
fn skip_variable_bypasses_the_gate() {
    let r = staged_over_budget("hook_skip");
    let (code, out, _) =
        r.run_hook(&["claude-gate"], &payload("git commit -m x"), &[("COMMENTBUSTER_SKIP", "1")]);
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn thresholds_come_from_flags_or_the_environment_and_flags_win() {
    let r = staged_over_budget("hook_tuning");
    let p = payload("git commit -m x");

    let (_, out, _) = r.run_hook(&["claude-gate"], &p, &[("COMMENTBUSTER_MAX_NET_SMALL", "6")]);
    assert_eq!(out, "", "net 6 is within a ceiling of 6 from the environment");

    let (_, out, _) = r.run_hook(&["claude-gate", "--max-net-small", "6"], &p, &[]);
    assert_eq!(out, "", "net 6 is within a ceiling of 6 from the flag");

    let (_, out, _) = r.run_hook(
        &["claude-gate", "--max-net-small", "5"],
        &p,
        &[("COMMENTBUSTER_MAX_NET_SMALL", "6")],
    );
    assert_eq!(decision(&out)["permissionDecision"], "deny", "the flag wins over the variable");

    // Lowering the floor under this 1-code-line commit hands it to the ratio rule.
    let (_, out, _) = r.run_hook(&["claude-gate", "--min-code-lines", "1"], &p, &[]);
    let why = reason(&out);
    assert!(why.contains("600%, over the 15% ceiling"), "{why}");
}

#[test]
fn an_unreadable_payload_allows() {
    let r = staged_over_budget("hook_garbage");
    for stdin in ["", "not json", "{}", r#"{"tool_input":{"command":7}}"#] {
        let (code, out, _) = r.run_hook(&["claude-gate"], stdin, &[]);
        assert_eq!((code, out.as_str()), (0, ""), "{stdin:?}");
    }
}

#[test]
fn outside_a_git_repo_allows_and_says_why() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hook_not_a_repo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ceiling = dir.parent().unwrap().to_str().unwrap().to_string();
    let r = Repo { dir };

    let (code, out, err) = r.run_hook(
        &["claude-gate"],
        &payload("git commit -m x"),
        &[("GIT_CEILING_DIRECTORIES", ceiling.as_str())],
    );
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(err.contains("git work tree"), "{err}");
    assert!(err.contains("allowing the commit"), "{err}");
}

#[test]
fn a_bad_flag_is_a_usage_error_that_blocks() {
    // Exit 2 is the one code Claude Code blocks on, so misconfiguration is
    // noticed rather than silently disabling the gate.
    let r = staged_over_budget("hook_bad_flag");
    let (code, _, err) = r.run_hook(&["claude-gate", "--max-ratio", "abc"], &payload("git commit"), &[]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = r.run_hook(&["claude-gate", "--max-ratio", "-1"], &payload("git commit"), &[]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = r.run_hook(&["claude-gate", "--max-ratio", "nan"], &payload("git commit"), &[]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("non-negative"), "{err}");
}

#[test]
fn the_subcommand_does_not_swallow_ref_arguments() {
    let r = staged_over_budget("hook_refs_intact");
    let head = r.git(&["rev-parse", "HEAD"]);
    // A plain ref still runs the main command; the JSON report proves it.
    let (code, v) = r.run(&[&head]);
    assert_eq!(code, 0);
    assert_eq!(v["range"]["mode"], "merge-base");
}
