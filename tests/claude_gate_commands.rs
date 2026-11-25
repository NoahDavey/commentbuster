//! Which commands `commentbuster claude-gate` gates, where it looks for the
//! repository, and when it asks for staging and committing to be separate
//! calls. Every case runs the real binary against real git repositories, with
//! the hook process started outside all of them.

mod common;
use common::{run_hook_from, Repo};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const PLAIN: &str = "package demo;

public class A {
  int a;
}
";

/// PLAIN with `n` line comments inside the class.
fn commented(n: usize) -> String {
    let lines: String = (0..n).map(|i| format!("  // note {i}\n")).collect();
    PLAIN.replace("  int a;\n", &format!("{lines}  int a;\n"))
}

fn payload(cwd: &Path, command: &str) -> String {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "cwd": cwd,
        "tool_input": { "command": command },
    })
    .to_string()
}

/// A directory outside every test repository for the hook process to run in.
fn elsewhere() -> PathBuf {
    let dir = std::env::temp_dir().join("commentbuster-elsewhere");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The denial reason for `command` run in `cwd`, or `None` when the hook stays
/// silent.
fn gate_in(cwd: &Path, command: &str) -> Option<String> {
    let (code, out, err) =
        run_hook_from(&elsewhere(), &["claude-gate"], &payload(cwd, command), &[]);
    assert_eq!(code, 0, "{command:?}: {err}");
    if out.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny", "{command:?}: {out}");
    Some(v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string())
}

fn gate(r: &Repo, command: &str) -> Option<String> {
    gate_in(&r.dir, command)
}

#[track_caller]
fn assert_over_budget(r: &Repo, command: &str) {
    let why = gate(r, command).unwrap_or_else(|| panic!("{command:?} must be gated"));
    assert!(why.contains("Comment budget exceeded"), "{command:?}: {why}");
}

#[track_caller]
fn assert_needs_split(r: &Repo, command: &str) {
    let why = gate(r, command).unwrap_or_else(|| panic!("{command:?} must be denied"));
    assert!(why.contains("Stage in one Bash call"), "{command:?}: {why}");
    assert!(why.contains("then run plain `git commit` in the next"), "{command:?}: {why}");
    assert!(!why.contains("Comment budget exceeded"), "{command:?}: {why}");
}

#[track_caller]
fn assert_allowed(r: &Repo, command: &str) {
    assert_eq!(gate(r, command), None, "{command:?} must be allowed");
}

/// PLAIN committed twice, so HEAD is not a root commit.
fn base(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("other.txt", "x\n");
    r.commit("root");
    r.write("A.java", PLAIN);
    r.commit("base");
    r
}

fn staged(name: &str, comments: usize) -> Repo {
    let r = base(name);
    r.write("A.java", &commented(comments));
    r.git(&["add", "-A"]);
    r
}

fn over_budget(name: &str) -> Repo {
    staged(name, 6)
}

fn within_budget(name: &str) -> Repo {
    staged(name, 2)
}

// --- 1. Command detection ---------------------------------------------------

#[test]
fn commits_are_found_wherever_the_shell_would_run_them() {
    let r = over_budget("detect_yes");
    for cmd in [
        "git commit",
        "  git commit -m 'x'",
        "git commit --no-verify -m x",
        "FOO=1 git commit -m x",
        "FOO=1 BAR='a b' git commit -m x",
        "COMMENTBUSTER_SKIP=1 git commit -m x",
        "export COMMENTBUSTER_SKIP=1; git commit -m x",
        "env FOO=1 git commit -m x",
        "env -i -u HOME FOO=1 git commit -m x",
        "/usr/bin/env FOO=\"$x\" git commit -m x",
        "/usr/bin/git commit -m x",
        "command git commit -m x",
        "time git commit -m x",
        "timeout 60 git commit -m x",
        "timeout -k 5 60 git commit -m x",
        "nohup git commit -m x",
        "nice -n 5 git commit -m x",
        "if true; then git commit -m x; fi",
        "if git diff --cached --quiet; then :; else git commit -m x; fi",
        "if a; then :; elif b; then git commit -m x; fi",
        "if ! git commit -m x; then echo failed; fi",
        "for i in 1; do git commit -m x; done",
        "while true; do git commit -m x; break; done",
        "until git commit -m x; do sleep 1; done",
        "case $x in a) git commit -m x;; esac",
        "(git commit -q)",
        "(cd . && git commit -q)",
        "{ git commit -q; }",
        "! git commit -q",
        "out=$(git commit -m x)",
        "out=`git commit -m x`",
        "echo \"result: $(git commit -m x)\"",
        "diff <(git commit -m x) /dev/null",
        "bash -c \"git commit -m 'x'\"",
        "sh -c 'git commit -m x'",
        "bash -lc 'git commit -m x'",
        "bash -euo pipefail -c 'git commit -m x'",
        "/bin/sh -c 'bash -c \"git commit -m x\"'",
        "git -C . commit -m x",
        "git -c user.name=x commit -m x",
        "git -c user.name='x y' -C . --no-pager commit -m x",
        "git -c \"user.name=$NAME\" commit -m x",
        "git \"commit\" -m x",
        "git \\\n  commit -m x",
        "cargo test && git commit -m x",
        "cargo test; git commit -m x | tail -3",
        "git diff --cached --quiet || git commit -m x",
        "echo done | git commit -F -",
        "git commit -m x > /dev/null 2>&1",
        "git commit > /dev/null -m x",
        "git commit -m x &",
        "git commit -m \"$(cat msg)\"",
        "git commit -m \"$MSG\" && git push",
        "cat <<'EOF' > /dev/null\nnote\nEOF\ngit commit -m x",
        "cat > /dev/null <<'EOF'; git commit -m x\nit's\nEOF",
        "git commit -F - <<'EOF'\nit's a \"message\" (with $things `quoted`\nEOF",
        "git commit -F - <<'EOF' && git push\nmessage\nEOF",
        "git commit -F - <<'EOF'; git push\nmessage\nEOF",
        "git commit -F - <<'EOF' 2>&1 | tail -3\nmessage\nEOF",
        "git commit -F - <<EOF &\nmessage\nEOF\nwait",
        "cat <<'EOF' | git commit -F -\nmessage\nEOF",
        "cat <<A <<'B'; git commit -m x\nit's a\nA\nit's b\nB",
        "git commit -m \"$(cat <<'EOF'\nfeat: a (b) it's \"q\n\nbody\nEOF\n)\"",
        "git commit -m \"$(cat <<-EOF\n\tfeat: it's\n\tEOF\n)\" --no-verify",
    ] {
        assert_over_budget(&r, cmd);
    }
}

#[test]
fn commit_must_be_the_git_subcommand() {
    let r = over_budget("detect_no");
    for cmd in [
        "git log --grep=commit",
        "git log --grep commit",
        "git show commit",
        "git commit-tree HEAD^{tree}",
        "git -C . log commit",
        "git -c alias.x=commit log",
        "git help commit",
        "git --help commit",
        "git checkout commit",
        "git add commit",
        "git config commit.gpgsign false",
        "git committed",
        "gitk commit",
        "git\ncommit",
        "echo git commit",
        "echo 'remember to git commit'",
        "echo \"git add . && git commit -m x\"",
        "printf '%s\\n' git commit",
        "grep -rn 'git commit' .",
        "rg \"; git commit\" docs",
        "# git commit -m x",
        "ls # then git commit",
        "cat <<'EOF' > notes.md\nrun git commit -m x\n$(git commit)\nEOF",
        "cat <<EOF > notes.md\ngit add -A\ngit commit -m x\nEOF",
        "cat <<'A' > a.sh; cat <<'B' > b.sh\ngit commit -m 1\nA\ngit commit -m 2\nB",
        "python3 - <<'PY'\nsubprocess.run('git commit')\nPY",
        "git tag -m 'git commit -am x' v1",
        "x='git commit'",
        "arr=(git commit)",
        "command -v git commit",
        "f() { git commit -m x; }",
    ] {
        assert_allowed(&r, cmd);
    }
}

#[test]
fn a_message_that_mentions_git_commands_is_only_a_message() {
    // In budget, so the only possible denial is a mention read as a command.
    let r = within_budget("detect_messages");
    for cmd in [
        "git commit -m 'do not git add -A && git commit -am x'",
        "git commit -m \"a; git add .; git commit -a\"",
        "git commit -m \"run \\`git commit -a\\` first\"",
        "git commit -m \"$(cat <<'EOF'\ndocs: explain `git add -A && git commit -am x`\n\ngit add .\ngit commit -m y\nEOF\n)\"",
        "git commit -F - <<'EOF'\ngit add -A\ngit commit -a\nEOF",
        "git commit -F - <<'EOF'; git push\nit's: git add -A\ngit commit -a\nEOF",
        "git commit -F - <<EOF 2>&1 | tail -1\ngit add -A && git commit -a\nEOF",
    ] {
        assert_allowed(&r, cmd);
    }
}

#[test]
fn dry_runs_and_help_pass() {
    let r = over_budget("detect_dry_run");
    for cmd in [
        "git commit --dry-run",
        "git commit --dry-run -am x",
        "git commit -m x --dry",
        "git commit --help",
        "git commit -h",
        "git commit --short",
        "git commit --porcelain",
        "git add -A && git commit --dry-run",
    ] {
        assert_allowed(&r, cmd);
    }
    assert_over_budget(&r, "git commit -m --dry-run");
    assert_over_budget(&r, "git commit -m --help");
    assert_over_budget(&r, "git commit --dry-run --no-dry-run -m x");
    assert_over_budget(&r, "git commit --dry-run; git commit -m x");
}

#[test]
fn what_cannot_be_resolved_statically_is_allowed() {
    let r = over_budget("detect_dynamic");
    for cmd in [
        "eval 'git commit -m x'",
        "$GIT commit -m x",
        "\"$GIT\" commit -m x",
        "git $SUB -m commit",
        "git $OPTS commit -m x",
        "git ci -m commit",
        "git -c alias.ci=commit ci -m x",
        "alias gc='git commit'; gc",
        "./commit.sh",
        "bash commit.sh",
        "bash -c \"cd $DIR && git commit -m x\"",
        "sh -c \"$CMD\" # commit",
        "bash <<'EOF'\ngit commit -m x\nEOF",
        "GIT_DIR=/elsewhere git commit -m x",
        "GIT_INDEX_FILE=/elsewhere git commit -m x",
        "export GIT_DIR=/elsewhere; git commit -m x",
        "git --git-dir=/elsewhere commit -m x",
        "git --work-tree /elsewhere commit -m x",
    ] {
        assert_allowed(&r, cmd);
    }
}

#[test]
fn unparseable_shell_is_allowed() {
    let r = over_budget("detect_broken");
    for cmd in ["echo 'unterminated\ngit commit -m x", "if ((; git commit", ")) git commit -m x"] {
        assert_allowed(&r, cmd);
    }
}

// --- 2. Target repository ---------------------------------------------------

/// `<name>/over` and `<name>/ok`, so a wrong directory flips the answer.
fn two_repos(name: &str) -> (PathBuf, Repo, Repo) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let over = over_budget(&format!("{name}/over"));
    let ok = within_budget(&format!("{name}/ok"));
    std::fs::create_dir_all(over.dir.join("sub dir")).unwrap();
    (root, over, ok)
}

#[track_caller]
fn assert_over_in(cwd: &Path, command: &str) {
    let why = gate_in(cwd, command).unwrap_or_else(|| panic!("{command:?} must be gated"));
    assert!(why.contains("Comment budget exceeded"), "{command:?}: {why}");
}

#[test]
fn the_repository_comes_from_the_payload_cwd() {
    let (_, over, ok) = two_repos("target_cwd");
    let p = payload(&over.dir, "git commit -m x");
    let (_, out, _) = run_hook_from(&ok.dir, &["claude-gate"], &p, &[]);
    assert!(out.contains("deny"), "the payload cwd decides, not the hook's: {out}");

    let p = payload(&ok.dir, "git commit -m x");
    let (_, out, _) = run_hook_from(&over.dir, &["claude-gate"], &p, &[]);
    assert_eq!(out, "");

    assert_over_in(&over.dir.join("sub dir"), "git commit -m x");
}

#[test]
fn a_literal_cd_moves_the_commit() {
    let (root, over, ok) = two_repos("target_cd");
    let abs = over.path().to_string();
    for cmd in [
        "cd over && git commit -m x",
        "cd over; git commit -m x",
        "cd over\ngit commit -m x",
        "cd -- over && git commit -m x",
        "cd -P over && git commit -m x",
        "cd 'over' && cd \"sub dir\" && git commit -m x",
        "cd over/sub\\ dir && git commit -m x",
        "cd over && cd .. && cd over && git commit -m x",
        "cd ok && cd ../over && git commit -m x",
        "cd over && (cd ../ok) && git commit -m x",
        "cd over && x=$(cd ../ok && pwd) && git commit -m x",
        "cd over && cd ../ok | cat && git commit -m x",
        "cd over && bash -c 'cd ../ok' && git commit -m x",
        "cd ok && (cd ../over && git commit -m x)",
        "bash -c 'cd over && git commit -m x'",
        "{ cd over; }; git commit -m x",
        "if cd over; then git commit -m x; fi",
        "cd over || exit 1; git commit -m x",
        "cd over 2>/dev/null && git commit -m x",
        "cd ok && cd - && cd over && git commit -m x",
        "pushd over && git commit -m x",
        "pushd ok && popd && cd over && git commit -m x",
        "env -C over git commit -m x",
        "command cd over && git commit -m x",
        "cd over && git status && git diff --cached --quiet || git commit -m x",
    ] {
        assert_over_in(&root, cmd);
    }
    assert_over_in(&root, &format!("cd {abs} && git commit -m x"));
    assert_over_in(&ok.dir, &format!("cd {abs} && git commit -m x"));
    assert_over_in(&ok.dir, &format!("cd \"$DIR\" && cd {abs} && git commit -m x"));

    assert_eq!(gate_in(&over.dir, "cd ../ok && git commit -m x"), None);
    assert_eq!(gate_in(&over.dir, "cd sub\\ dir; cd ../../ok; git commit -m x"), None);
}

#[test]
fn git_dash_c_moves_the_commit() {
    let (root, over, ok) = two_repos("target_dash_c");
    assert_over_in(&root, "git -C over commit -m x");
    assert_over_in(&root, "git -C over -C 'sub dir' commit -m x");
    assert_over_in(&root, "git -C ok -C ../over commit -m x");
    assert_over_in(&ok.dir, "cd .. && git -C over commit -m x");
    assert_over_in(&ok.dir, &format!("git -C {} commit -m x", over.path()));
    assert_over_in(&ok.dir, &format!("cd \"$DIR\"; git -C {} commit -m x", over.path()));
    assert_eq!(gate_in(&over.dir, "git -C ../ok commit -m x"), None);
    // `git add` in another repository's directory is still part of the call,
    // and -C on one command does not leak into the next.
    assert_over_in(&over.dir, "git -C ../ok status && git commit -m x");
}

#[test]
fn a_tilde_is_the_home_directory() {
    let (root, _over, ok) = two_repos("target_tilde");
    let p = payload(&ok.dir, "cd ~/over && git commit -m x");
    let home = root.to_str().unwrap();
    let (_, out, _) = run_hook_from(&elsewhere(), &["claude-gate"], &p, &[("HOME", home)]);
    assert!(out.contains("Comment budget exceeded"), "{out}");
}

#[test]
fn a_directory_that_is_not_literal_is_allowed() {
    let (_, over, _) = two_repos("target_dynamic");
    for cmd in [
        "cd \"$DIR\" && git commit -m x",
        "cd $DIR; git commit -m x",
        "cd \"$DIR/sub\" && git commit -m x",
        "cd \"$(git rev-parse --show-toplevel)\" && git commit -m x",
        "cd sub* && git commit -m x",
        "cd ~someone/repo && git commit -m x",
        "git -C \"$DIR\" commit -m x",
        "pushd \"$DIR\" && git commit -m x",
        "popd; git commit -m x",
        "cd \"$DIR\" && git add -A && git commit -m x",
        "cd \"$DIR\" && git commit -am x",
        "for d in a b; do (cd \"$d\" && git commit -m x); done",
        "cd ../ok || cd ../over; git commit -m x",
        "if a; then cd ../ok; fi; git commit -m x",
    ] {
        assert_allowed(&over, cmd);
    }
}

#[test]
fn a_directory_that_does_not_exist_is_allowed() {
    let r = over_budget("target_missing");
    let p = payload(&r.dir, "cd missing && git commit -m x");
    let (code, out, err) = run_hook_from(&elsewhere(), &["claude-gate"], &p, &[]);
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(err.contains("allowing the commit"), "{err}");
}

#[test]
fn a_linked_worktree_is_gated_on_its_own_index() {
    let main = base("worktree_main");
    let linked = main.dir.with_file_name("worktree_linked");
    let _ = std::fs::remove_dir_all(&linked);
    main.git(&["worktree", "add", "-q", "-b", "side", linked.to_str().unwrap()]);

    let side = Repo { dir: linked };
    side.write("A.java", &commented(6));
    side.git(&["add", "-A"]);

    assert_over_budget(&side, "git commit -m x");
    assert_over_budget(&main, "cd ../worktree_linked && git commit -m x");
    assert_over_budget(&main, "git -C ../worktree_linked commit -m x");
    assert_allowed(&main, "git commit -m x");
    assert_allowed(&side, "cd ../worktree_main && git commit -m x");

    side.write("A.java", &commented(2));
    side.git(&["add", "-A"]);
    main.write("A.java", &commented(6));
    main.git(&["add", "-A"]);
    assert_allowed(&side, "git commit -m x");
    assert_over_budget(&main, "git commit -m x");
}

#[test]
fn a_repository_with_diff_relative_set_is_counted_whole() {
    let r = over_budget("target_relative");
    r.git(&["config", "diff.relative", "true"]);
    r.write("sub/x.txt", "x\n");
    assert_over_budget(&r, "cd sub && git commit -m x");
}

// --- 3. Staging in the same call --------------------------------------------

#[test]
fn changing_the_index_before_the_commit_is_denied() {
    let r = within_budget("stage_before");
    for cmd in [
        "git add -A && git commit -m x",
        "git add A.java; git commit -m x",
        "git add .\ngit commit -m x",
        "git stage . && git commit -m x",
        "git rm other.txt && git commit -m x",
        "git mv a b && git commit -m x",
        "git reset && git commit -m x",
        "git reset --soft HEAD~1 && git commit -m x",
        "git restore --staged A.java && git commit -m x",
        "git restore -S A.java && git commit -m x",
        "git restore -SW A.java && git commit -m x",
        "git stash && git commit -m x",
        "git stash pop && git commit -m x",
        "git stash push -m list && git commit -m x",
        "git apply --index p.diff && git commit -m x",
        "git apply --cached p.diff && git commit -m x",
        "git apply -3 p.diff && git commit -m x",
        "git apply --cached <<'EOF' && git commit -m x\ndiff\nEOF",
        "git update-index --add A.java && git commit -m x",
        "git read-tree HEAD && git commit -m x",
        "git merge --no-commit topic && git commit -m x",
        "git merge --squash topic && git commit -m x",
        "git pull && git commit -m x",
        "git rebase main && git commit -m x",
        "git cherry-pick -n abc && git commit -m x",
        "git revert -n abc && git commit -m x",
        "git am p.mbox && git commit -m x",
        "git checkout main && git commit -m x",
        "git checkout main -- A.java && git commit -m x",
        "git checkout -b topic origin/main && git commit -m x",
        "git switch main && git commit -m x",
        "git submodule add u p && git commit -m x",
        "git -C . add -A && git commit -m x",
        "git -c core.autocrlf=false add -A && git commit -m x",
        "/usr/bin/git add -A && env A=1 git commit -m x",
        "git diff --name-only | xargs git add && git commit -m x",
        "git ls-files -z | xargs -0 -n 1 git add && git commit -m x",
        "(git add -A) && git commit -m x",
        "x=$(git add -A) && git commit -m x",
        "bash -c 'git add -A' && git commit -m x",
        "bash -c 'git add -A && git commit -m x'",
        "for f in a b; do git add $f; done; git commit -m x",
        "while read -r f; do git add \"$f\"; done < list; git commit -m x",
        "if true; then git add -A; fi && git commit -m x",
        "git add \"$FILES\" && git commit -m \"$MSG\"",
        "git commit -m \"$(git add -A; echo x)\"",
        "git add -A && git commit -F - <<'EOF'\nmessage\nEOF",
        "git add -A && git commit -m \"$(cat <<'EOF'\nit's\nEOF\n)\" && git push",
        "for i in 1 2; do git commit -m $i; git add next; done",
    ] {
        assert_needs_split(&r, cmd);
    }
}

#[test]
fn commands_that_leave_the_index_alone_are_fine_before_a_commit() {
    let r = within_budget("stage_harmless");
    for cmd in [
        "git status && git commit -m x",
        "git diff --cached && git commit -m x",
        "git add -n . && git commit -m x",
        "git add --dry-run . && git commit -m x",
        "git rm --dry-run other.txt && git commit -m x",
        "git restore A.java && git commit -m x",
        "git stash list && git commit -m x",
        "git stash show && git commit -m x",
        "git apply p.diff && git commit -m x",
        "git apply --check p.diff && git commit -m x",
        "git checkout -b topic && git commit -m x",
        "git checkout -- A.java && git commit -m x",
        "git switch -c topic && git commit -m x",
        "git fetch && git commit -m x",
        "git log add && git commit -m x",
        "echo git add -A && git commit -m x",
        "npm add left-pad && git commit -m x",
        "git commit -m x && git add -A",
        "git commit -m x; git stash pop",
        "git commit -m x && git push && git status",
        "f() { git add -A; }; git commit -m x",
    ] {
        assert_allowed(&r, cmd);
    }
}

#[test]
fn a_commit_that_stages_for_itself_is_denied() {
    let r = within_budget("stage_flags");
    for cmd in [
        "git commit -a -m x",
        "git commit -am x",
        "git commit -am \"$MSG\"",
        "git commit -qam x",
        "git commit -vsam x",
        "git commit --all -m x",
        "git commit -m x -a",
        "git commit -m 'a' -a",
        "git commit -p",
        "git commit --patch -m x",
        "git commit --interactive",
        "git commit -i A.java -m x",
        "git commit --include A.java -m x",
        "git commit -o A.java -m x",
        "git commit --only A.java -m x",
        "git commit A.java -m x",
        "git commit -m x A.java",
        "git commit -m x -- A.java",
        "git commit -m x -- -a",
        "git commit -m x -- \"$f\"",
        "git commit -m x src/$f",
        "git commit -m x *.java",
        "git commit -m x .",
        "git commit > /dev/null -m x A.java",
        "git commit -uno x -m y",
        "git commit -u no -m y",
        "git commit -S key -m y",
        "git commit --gpg-sign key -m y",
        "git commit --pathspec-from-file=list -m x",
        "git commit --pathspec-from-file list -m x",
        "git commit --amend -a --no-edit",
        "git commit --no-all --all -m x",
    ] {
        assert_needs_split(&r, cmd);
    }
}

#[test]
fn option_arguments_are_not_pathspecs_or_flags() {
    let r = within_budget("stage_option_args");
    for cmd in [
        "git commit -m a",
        "git commit -ma",
        "git commit -m -a",
        "git commit -m -- -q",
        "git commit -m --all",
        "git commit -m a -m b",
        "git commit -m 'a' -m \"$B\" -m $C",
        "git commit -qm a",
        "git commit -m\"$MSG\"",
        "git commit --message a",
        "git commit --message=a",
        "git commit --message=\"$MSG\"",
        "git commit --mess a",
        "git commit -F msg.txt",
        "git commit -Fa",
        "git commit --file msg.txt",
        "git commit -t tpl -m x",
        "git commit --template tpl -m x",
        "git commit -C HEAD",
        "git commit -c HEAD",
        "git commit --reuse-message HEAD",
        "git commit --reedit-message HEAD",
        "git commit --fixup HEAD",
        "git commit --squash HEAD",
        "git commit --author 'A <a@b>' -m x",
        "git commit --date now -m x",
        "git commit --cleanup strip -m x",
        "git commit --trailer 'A: b' -m x",
        "git commit -S -m x",
        "git commit -Sabc -m x",
        "git commit --gpg-sign=abc -m x",
        "git commit --no-gpg-sign -m x",
        "git commit -uall -m x",
        "git commit -uno -m x",
        "git commit --untracked-files=all -m x",
        "git commit -U 3 -m x",
        "git commit -s -n -e -v -q -m x",
        "git commit --all --no-all -m x",
        "git commit --allow-empty -m x",
        "git commit --allow-empty-message -m x",
        "git commit --no-verify --no-edit --signoff -m x",
        "git commit -m x --",
        "git commit $OPTS -m x",
        "git commit -m x > out.txt 2>&1",
        "git commit -m x 2> err.txt < /dev/null",
    ] {
        assert_allowed(&r, cmd);
    }
}

#[test]
fn more_than_one_commit_is_denied() {
    let r = within_budget("stage_multiple");
    assert_needs_split(&r, "git commit -m a && git commit -m b");
    assert_needs_split(&r, "git commit -m a; git commit --amend --no-edit");
    assert_needs_split(&r, "(git commit -m a); echo $(git commit -m b)");
    assert_needs_split(&r, "bash -c 'git commit -m a'; git commit -m b");
    assert_needs_split(&r, "if x; then git commit -m a; else git commit -m b; fi");
    assert_allowed(&r, "git commit --dry-run && git commit -m a");
}

#[test]
fn the_split_denial_names_the_cause() {
    let r = within_budget("stage_reason");
    let why = gate(&r, "git add -A && git commit -m x").unwrap();
    assert!(why.contains("`git add`"), "{why}");
    let why = gate(&r, "git commit -am x").unwrap();
    assert!(why.contains("--all"), "{why}");
    let why = gate(&r, "git commit -m x A.java").unwrap();
    assert!(why.contains("path"), "{why}");
    let why = gate(&r, "git commit -m x; git commit -m y").unwrap();
    assert!(why.contains("more than one `git commit`"), "{why}");
}

// --- 4. Merges, rebases and amends ------------------------------------------

/// `topic` and `main` conflict over A.java, and `topic` adds 12 comments to
/// B.java, so concluding a merge of it would be over budget.
fn conflicting(name: &str) -> Repo {
    let r = base(name);
    r.git(&["checkout", "-q", "-b", "topic"]);
    r.write("A.java", &PLAIN.replace("int a", "int topic"));
    r.write("B.java", &commented(12));
    r.commit("topic");
    r.git(&["checkout", "-q", "main"]);
    r.write("A.java", &PLAIN.replace("int a", "int main"));
    r.commit("main");
    r
}

/// Run git expecting the conflict that stops it, then stage a resolution.
fn conflict(r: &Repo, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(&r.dir)
        .env("GIT_EDITOR", "true")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{args:?} was expected to conflict");
    r.write("A.java", &commented(8));
    r.git(&["add", "-A"]);
}

#[test]
fn a_merge_in_progress_is_allowed() {
    let r = conflicting("merge_progress");
    conflict(&r, &["merge", "topic"]);
    assert_allowed(&r, "git commit --no-edit");
    assert_allowed(&r, "git add -A && git commit --no-edit");
    assert_allowed(&r, "git commit -am x");
    std::fs::create_dir_all(r.dir.join("deep/er")).unwrap();
    assert_allowed(&r, "cd deep/er && git commit --no-edit");

    r.git(&["commit", "-q", "--no-edit"]);
    r.write("A.java", &commented(20));
    r.git(&["add", "-A"]);
    assert_over_budget(&r, "git commit -m x");
}

#[test]
fn a_rebase_in_progress_is_allowed() {
    for backend in ["--merge", "--apply"] {
        let r = conflicting(&format!("rebase_progress{backend}"));
        r.git(&["checkout", "-q", "topic"]);
        conflict(&r, &["rebase", backend, "main"]);
        assert_allowed(&r, "git commit --no-edit");
        assert_allowed(&r, "git add -A && git commit --no-edit");
        r.git(&["rebase", "--abort"]);
        r.write("A.java", &commented(8));
        r.git(&["add", "-A"]);
        assert_over_budget(&r, "git commit --no-edit");
    }
}

#[test]
fn a_cherry_pick_in_progress_is_allowed() {
    let r = conflicting("pick_progress");
    conflict(&r, &["cherry-pick", "topic"]);
    assert_allowed(&r, "git commit --no-edit");
    assert_allowed(&r, "git add -A && git commit --no-edit");
}

#[test]
fn a_revert_in_progress_is_allowed() {
    let r = conflicting("revert_progress");
    r.write("A.java", &PLAIN.replace("int a", "int again"));
    r.commit("again");
    conflict(&r, &["revert", "HEAD~1"]);
    assert_allowed(&r, "git commit --no-edit");
    assert_allowed(&r, "git add -A && git commit --no-edit");
}

#[test]
fn a_multi_commit_pick_is_in_progress_between_commits() {
    let r = conflicting("pick_sequence");
    r.git(&["checkout", "-q", "topic"]);
    r.write("C.java", PLAIN);
    r.commit("second");
    r.git(&["checkout", "-q", "main"]);
    conflict(&r, &["cherry-pick", "topic~1", "topic"]);
    r.git(&["commit", "-q", "--no-edit"]);
    r.write("A.java", &commented(30));
    r.git(&["add", "-A"]);
    assert_allowed(&r, "git commit -m x");
}

#[test]
fn a_merge_in_a_linked_worktree_is_seen_only_there() {
    let main = conflicting("merge_wt_main");
    let linked = main.dir.with_file_name("merge_wt_linked");
    let _ = std::fs::remove_dir_all(&linked);
    main.git(&["worktree", "add", "-q", "-b", "side", linked.to_str().unwrap()]);
    let side = Repo { dir: linked };
    conflict(&side, &["merge", "topic"]);
    main.write("A.java", &commented(8));
    main.git(&["add", "-A"]);

    assert_allowed(&side, "git commit --no-edit");
    assert_allowed(&side, "git add -A && git commit --no-edit");
    assert_allowed(&main, "cd ../merge_wt_linked && git add -A && git commit --no-edit");
    assert_over_budget(&main, "git commit -m x");
}

#[test]
fn repeated_amends_are_measured_from_the_parent() {
    let r = base("amend_rounds");
    r.write("A.java", &commented(5));
    r.git(&["add", "-A"]);
    assert_allowed(&r, "git commit -m x");
    r.git(&["commit", "-q", "-m", "x"]);

    // Another +5, on its own within budget, +10 against the parent.
    r.write("A.java", &commented(10));
    r.git(&["add", "-A"]);
    let why = gate(&r, "git commit --amend --no-edit").expect("round two is over");
    assert!(why.contains("adds 10 net comment lines"), "{why}");
    assert!(why.contains("what HEAD already has plus the index"), "{why}");
    assert_over_budget(&r, "git commit --am --no-edit");
    assert_over_budget(&r, "git commit -m --amend --amend");
    assert_allowed(&r, "git commit -m x");
    assert_allowed(&r, "git commit -m --amend");

    // Trimmed back within budget, the amend goes through, and round three
    // still counts from the parent.
    r.write("A.java", &commented(4));
    r.git(&["add", "-A"]);
    assert_allowed(&r, "git commit --amend --no-edit");
    r.git(&["commit", "-q", "--amend", "--no-edit"]);
    r.write("A.java", &commented(9));
    r.git(&["add", "-A"]);
    assert_over_budget(&r, "git commit --amend --no-edit");
    assert_allowed(&r, "git commit --amend --no-amend -m x");
}

#[test]
fn amending_a_root_commit_is_measured_from_the_empty_tree() {
    let r = Repo::new("amend_root");
    r.write("A.java", &commented(4));
    r.commit("root");
    assert_allowed(&r, "git commit --amend --no-edit");
    r.write("A.java", &commented(8));
    r.git(&["add", "-A"]);
    let why = gate(&r, "git commit --amend --no-edit").expect("8 against nothing");
    assert!(why.contains("adds 8 net comment lines"), "{why}");
}

#[test]
fn amending_a_merge_commit_is_allowed() {
    let r = conflicting("amend_merge");
    conflict(&r, &["merge", "topic"]);
    r.git(&["commit", "-q", "--no-edit"]);
    r.write("A.java", &commented(30));
    r.git(&["add", "-A"]);
    assert_allowed(&r, "git commit --amend --no-edit");
    assert_over_budget(&r, "git commit -m x");
    // Only the budget comparison is skipped: the index must still be final.
    assert_needs_split(&r, "git add -A && git commit --amend --no-edit");
}

#[test]
fn an_amend_in_a_linked_worktree_uses_that_worktrees_head() {
    let main = base("amend_wt_main");
    let linked = main.dir.with_file_name("amend_wt_linked");
    let _ = std::fs::remove_dir_all(&linked);
    main.git(&["worktree", "add", "-q", "-b", "side", linked.to_str().unwrap()]);
    let side = Repo { dir: linked };
    side.write("A.java", &commented(5));
    side.commit("side");
    side.write("A.java", &commented(10));
    side.git(&["add", "-A"]);
    assert_over_budget(&side, "git commit --amend --no-edit");
    assert_over_budget(&main, "git -C ../amend_wt_linked commit --amend --no-edit");
}

// --- 5. Denial text ---------------------------------------------------------

#[test]
fn the_budget_denial_says_to_trim_and_how_to_escalate() {
    let r = over_budget("denial_text");
    let why = gate(&r, "git commit -m x").unwrap();
    let keep = why.find("Keep the ones that explain why").expect("existing advice");
    let trim = why
        .find("Trim to budget and commit. Do not bypass the gate.")
        .expect("trim instruction");
    assert!(keep < trim, "{why}");
    for phrase in [
        "extremely strong reason",
        "leave it out",
        "the file, the exact comment and the reason",
        "pull request description",
        "needing human eyes",
        "final message if there is no PR",
    ] {
        assert!(why[trim..].contains(phrase), "missing {phrase:?} in {why}");
    }
}

#[test]
fn no_denial_mentions_the_skip_variable() {
    let r = over_budget("denial_no_skip");
    for cmd in ["git commit -m x", "git commit -am x", "git add . && git commit -m x"] {
        let why = gate(&r, cmd).unwrap();
        assert!(!why.contains("COMMENTBUSTER"), "{why}");
        assert!(!why.to_lowercase().contains("skip"), "{why}");
    }
}

#[test]
fn the_skip_variable_still_works_from_the_hooks_own_environment() {
    let r = over_budget("denial_skip_env");
    let p = payload(&r.dir, "git add -A && git commit -m x");
    let (code, out, _) =
        run_hook_from(&elsewhere(), &["claude-gate"], &p, &[("COMMENTBUSTER_SKIP", "1")]);
    assert_eq!((code, out.as_str()), (0, ""));
}
