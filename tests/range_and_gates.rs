//! SPEC.md §8: range semantics, invocation modes, and the gate matrix.

mod common;
use common::{totals, Repo};

const PLAIN: &str = "package demo;

public class A {
  int add(int a, int b) {
    return a + b;
  }
}
";

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

const TEN_COMMENTS: &str = "package demo;

// c1
// c2
// c3
// c4
// c5
// c6
// c7
// c8
// c9
// c10
public class Other {
}
";

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

/// main and feature both move on from a shared base.
fn diverged(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("src/A.java", PLAIN);
    r.commit("base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("src/A.java", THREE_COMMENTS);
    r.commit("feature: three comments");
    r.git(&["checkout", "-q", "main"]);
    r.write("src/Other.java", TEN_COMMENTS);
    r.commit("main: someone else's ten comments");
    r.git(&["checkout", "-q", "feature"]);
    r
}

#[test]
fn merge_base_ignores_commits_that_landed_on_the_base_branch() {
    let r = diverged("merge_base");
    let (code, v) = r.run(&["main", "feature"]);
    assert_eq!(code, 0);
    // Only the feature branch's own three comments.
    assert_eq!(totals(&v), (3, 0, 3, 0, 0));
    assert_eq!(v["range"]["mode"], "merge-base");
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(v["files"][0]["path"], "src/A.java");
}

#[test]
fn one_ref_means_that_ref_to_head() {
    let r = diverged("one_ref");
    let (_, two) = r.run(&["main", "feature"]);
    let (_, one) = r.run(&["main"]);
    assert_eq!(totals(&one), totals(&two));
    assert_eq!(one["range"]["head_ref"], "HEAD");
}

#[test]
fn direct_compares_literally_and_sees_the_moved_base() {
    let r = diverged("direct");
    let (code, v) = r.run(&["--direct", "main", "feature"]);
    assert_eq!(code, 0);
    // main's ten comments are absent from feature, so a literal comparison
    // reads them as removed. This is why merge-base is the default.
    assert_eq!(totals(&v), (3, 10, -7, 0, 3));
    assert_eq!(v["range"]["mode"], "direct");
}

#[test]
fn worktree_mode_sees_an_unstaged_edit() {
    let r = Repo::new("worktree");
    r.write("src/A.java", PLAIN);
    r.commit("base");
    r.write("src/A.java", THREE_COMMENTS);

    let (code, v) = r.run(&[]);
    assert_eq!(code, 0);
    assert_eq!(totals(&v), (3, 0, 3, 0, 0));
    assert_eq!(v["range"]["mode"], "worktree");
}

#[test]
fn staged_mode_sees_only_the_index() {
    let r = Repo::new("staged");
    r.write("src/A.java", PLAIN);
    r.write("src/B.java", PLAIN);
    r.commit("base");

    r.write("src/A.java", THREE_COMMENTS);
    r.git(&["add", "src/A.java"]);
    r.write("src/B.java", COMMENTED); // left unstaged

    let (code, v) = r.run(&["--staged"]);
    assert_eq!(code, 0);
    assert_eq!(totals(&v), (3, 0, 3, 0, 0));
    assert_eq!(v["range"]["mode"], "staged");
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(v["files"][0]["path"], "src/A.java");
}

#[test]
fn untracked_files_are_invisible_to_worktree_mode() {
    // Matches `git diff HEAD` semantics. Pinned so the limitation is a decision
    // rather than a surprise: stage the file, or use --staged, to see it.
    let r = Repo::new("untracked");
    r.write("src/A.java", PLAIN);
    r.commit("base");
    r.write("src/New.java", COMMENTED);

    let (_, v) = r.run(&[]);
    assert_eq!(totals(&v), (0, 0, 0, 0, 0));

    r.git(&["add", "src/New.java"]);
    let (_, staged) = r.run(&["--staged"]);
    assert_eq!(totals(&staged).0, 6);
}

#[test]
fn path_filter_limits_the_diff() {
    let r = diverged("path_filter");
    let (_, v) = r.run(&["--direct", "main", "feature", "--", "src/A.java"]);
    assert_eq!(totals(&v), (3, 0, 3, 0, 0));
}

/// A diff of 6 added comment lines against 1 added code line: ratio 600%.
fn budget_repo(name: &str) -> (Repo, String) {
    let r = Repo::new(name);
    r.write("src/A.java", PLAIN);
    let base = r.commit("base");
    r.write("src/A.java", COMMENTED);
    r.commit("comment it up");
    (r, base)
}

#[test]
fn no_gate_configured_always_exits_zero() {
    let (r, base) = budget_repo("gate_none");
    let (code, v) = r.run(&[&base]);
    assert_eq!(code, 0);
    assert_eq!(v["gate"]["configured"], false);
    assert_eq!(v["gate"]["passed"], true);
}

#[test]
fn max_net_gate() {
    let (r, base) = budget_repo("gate_net");

    let (code, v) = r.run(&[&base, "--max-net", "5"]);
    assert_eq!(code, 1);
    assert_eq!(v["gate"]["passed"], false);
    assert_eq!(v["gate"]["failures"][0]["rule"], "max_net");
    assert_eq!(v["gate"]["failures"][0]["limit"], 5);
    assert_eq!(v["gate"]["failures"][0]["actual"], 6);

    let (code, _) = r.run(&[&base, "--max-net", "6"]);
    assert_eq!(code, 0, "the limit is inclusive");
}

#[test]
fn max_ratio_gate() {
    let (r, base) = budget_repo("gate_ratio");

    let (code, v) = r.run(&[&base, "--max-ratio", "0.05", "--min-code-lines", "1"]);
    assert_eq!(code, 1);
    assert_eq!(v["gate"]["failures"][0]["rule"], "max_ratio");
    assert_eq!(v["totals"]["comment_ratio"], 6.0);

    // Ratios above 1 are legal: this diff really is 600% comments.
    let (code, _) = r.run(&[&base, "--max-ratio", "7", "--min-code-lines", "1"]);
    assert_eq!(code, 0);
}

#[test]
fn min_code_lines_suppresses_the_ratio_gate_on_tiny_diffs() {
    let (r, base) = budget_repo("gate_min_code");
    // 1 added code line is below the default floor of 20.
    let (code, v) = r.run(&[&base, "--max-ratio", "0.05"]);
    assert_eq!(code, 0);
    assert_eq!(v["gate"]["passed"], true);
    assert_eq!(v["gate"]["skipped_rules"][0]["rule"], "max_ratio");
    assert!(v["gate"]["skipped_rules"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("below --min-code-lines"));
}

#[test]
fn ratio_gate_reports_when_a_diff_adds_no_code_at_all() {
    let r = Repo::new("gate_no_code");
    r.write("src/A.java", PLAIN);
    let base = r.commit("base");
    r.write("src/A.java", THREE_COMMENTS);
    r.commit("comments only");

    let (code, v) = r.run(&[&base, "--max-ratio", "0.05", "--min-code-lines", "0"]);
    assert_eq!(code, 0);
    assert_eq!(v["gate"]["skipped_rules"][0]["reason"], "diff adds no code lines");
}

#[test]
fn max_net_gate_can_demand_a_net_reduction() {
    let r = Repo::new("gate_negative");
    r.write("src/A.java", COMMENTED);
    let base = r.commit("base");
    r.write("src/A.java", PLAIN);
    r.commit("strip");

    let (code, _) = r.run(&[&base, "--max-net", "-3"]);
    assert_eq!(code, 0, "net -6 is within a budget of -3");

    let (code, _) = r.run(&[&base, "--max-net", "-9"]);
    assert_eq!(code, 1, "net -6 does not reach a demanded -9");
}

#[test]
fn usage_errors_exit_two() {
    let (r, _) = budget_repo("usage");

    let (code, _, err) = r.run_raw(&["no-such-ref"]);
    assert_eq!(code, 2);
    assert!(err.contains("no-such-ref"), "{err}");

    let (code, _, err) = r.run_raw(&["--staged", "main"]);
    assert_eq!(code, 2);
    assert!(err.contains("--staged"), "{err}");

    let (code, _, _) = r.run_raw(&["a", "b", "c"]);
    assert_eq!(code, 2);
}

#[test]
fn outside_a_git_repo_exits_two() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("not_a_repo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_commentbuster"))
        .current_dir(&dir)
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("git work tree"));
}

#[test]
fn pretty_output_shows_the_breakdown_and_the_failure() {
    let (r, base) = budget_repo("pretty");
    let (code, out, _) = r.run_raw(&[
        &base, "--pretty", "--color", "never", "--max-ratio", "0.05", "--min-code-lines", "1",
    ]);
    assert_eq!(code, 1);
    for expected in [
        "commentbuster",
        "comment lines",
        "+6",
        "-0",
        "net +6",
        "ratio 600.0%",
        "src/A.java",
        "java",
        "FAIL",
        "--max-ratio",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
}

#[test]
fn json_is_the_default_and_reports_the_range() {
    let (r, base) = budget_repo("json_shape");
    let (_, v) = r.run(&[&base]);
    assert_eq!(v["version"], 1);
    assert!(v["duration_ms"].is_u64());
    assert_eq!(v["range"]["base_sha"], base);
    assert_eq!(v["range"]["head_ref"], "HEAD");
    for key in ["version", "range", "totals", "files", "skipped", "gate", "duration_ms"] {
        assert!(v.get(key).is_some(), "missing key {key}");
    }
}

#[test]
fn top_limits_the_pretty_table_but_never_the_json() {
    let r = Repo::new("top_flag");
    r.write("seed.md", "seed\n");
    let base = r.commit("base");
    for i in 0..5 {
        r.write(&format!("src/C{i}.java"), &format!("// c{i}\npublic class C{i} {{ }}\n"));
    }
    r.commit("five files");

    let (_, out, _) = r.run_raw(&[&base, "--pretty", "--color", "never", "--top", "2"]);
    assert!(out.contains("and 3 more files"), "{out}");
    assert_eq!(out.matches(".java").count(), 2);

    let (_, out, _) = r.run_raw(&[&base, "--pretty", "--color", "never", "--top", "0"]);
    assert!(!out.contains("more file"), "{out}");
    assert_eq!(out.matches(".java").count(), 5);

    // JSON is unaffected by --top.
    let (_, v) = r.run(&[&base, "--top", "2"]);
    assert_eq!(v["files"].as_array().unwrap().len(), 5);
}

#[test]
fn a_closed_pipe_is_not_an_error() {
    // `commentbuster … | jq .totals` and `| head` close stdout early. That must
    // exit quietly, not panic with a Rust backtrace.
    let (r, base) = budget_repo("broken_pipe");
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "'{}' {base} | head -1",
            env!("CARGO_BIN_EXE_commentbuster")
        ))
        .current_dir(&r.dir)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "panicked on a closed pipe:\n{err}");
    assert!(err.is_empty(), "unexpected stderr: {err}");
}

#[test]
fn max_net_small_gate_applies_below_the_code_floor() {
    let (r, base) = budget_repo("gate_net_small");
    // 1 added code line is below the default floor of 20, so the small-commit rule decides.
    let (code, v) = r.run(&[&base, "--max-net-small", "5"]);
    assert_eq!(code, 1);
    assert_eq!(v["gate"]["failures"][0]["rule"], "max_net_small");
    assert_eq!(v["gate"]["failures"][0]["limit"], 5);
    assert_eq!(v["gate"]["failures"][0]["actual"], 6);

    let (code, _) = r.run(&[&base, "--max-net-small", "6"]);
    assert_eq!(code, 0, "the limit is inclusive");

    let (code, out, _) = r.run_raw(&[&base, "--pretty", "--color", "never", "--max-net-small", "5"]);
    assert_eq!(code, 1);
    assert!(out.contains("FAIL  net 6 exceeds --max-net-small 5"), "{out}");
}

#[test]
fn max_net_small_gate_steps_aside_at_or_above_the_code_floor() {
    let (r, base) = budget_repo("gate_net_small_skipped");
    let (code, v) = r.run(&[&base, "--max-net-small", "0", "--min-code-lines", "1"]);
    assert_eq!(code, 0);
    assert_eq!(v["gate"]["configured"], true);
    assert_eq!(v["gate"]["passed"], true);
    assert_eq!(v["gate"]["skipped_rules"][0]["rule"], "max_net_small");
    assert!(v["gate"]["skipped_rules"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("not below --min-code-lines 1"));
}

#[test]
fn ratio_and_net_small_never_both_apply_to_one_diff() {
    let (r, base) = budget_repo("gate_complementary");

    // Below the floor: the net decides, the ratio is recorded as skipped.
    let (code, v) = r.run(&[&base, "--max-ratio", "0.15", "--max-net-small", "5"]);
    assert_eq!(code, 1);
    assert_eq!(v["gate"]["failures"].as_array().unwrap().len(), 1);
    assert_eq!(v["gate"]["failures"][0]["rule"], "max_net_small");
    assert_eq!(v["gate"]["skipped_rules"][0]["rule"], "max_ratio");

    // Floor lowered under the diff: the ratio decides, the net is skipped.
    let (code, v) = r.run(&[&base, "--max-ratio", "0.15", "--max-net-small", "5", "--min-code-lines", "1"]);
    assert_eq!(code, 1);
    assert_eq!(v["gate"]["failures"].as_array().unwrap().len(), 1);
    assert_eq!(v["gate"]["failures"][0]["rule"], "max_ratio");
    assert_eq!(v["gate"]["skipped_rules"][0]["rule"], "max_net_small");
}
