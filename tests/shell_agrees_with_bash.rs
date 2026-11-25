//! The walker's reading of a command, compared with what bash does when it
//! runs the same command with a `git` that records its directory and words.

use commentbuster::shell::{git_calls, GitCall};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Commands whose every branch runs, so the two lists must match in full.
const COMMANDS: &[&str] = &[
    "git commit -m a",
    "git commit -m 'it'\"'\"'s' -m \"a \\\"b\\\" \\$c \\`d\\` \\\\ \\e\" -m a\\ b\\\"",
    "git commit -m $'l1\\nl2\\t\\'q\\'' -m '' -m \"\"",
    "git commit -m \"multi\nline 'quoted' text\" -- 'a b' \\-x",
    "git \\\n  commit \\\n  -m x",
    "git 'commit' -m x; g\"it\" com\\mit",
    "git commit -m \"line \\\ncontinued\"",
    "FOO=1 git commit -m x; env -u BAR FOO='a b' git commit -m y; env -i PATH=\"$PATH\" git commit",
    "command git commit; nohup git commit; nice -n 1 git commit; nice -1 git commit",
    "time git commit -m x; time { git commit -m y; }; time (git commit -m z)",
    "builtin cd d1 && git commit; command cd ../d2 && git commit",
    "cd d1 && git commit; cd ../d2; git commit; cd - >/dev/null; git commit",
    "cd d1/sub && cd .. && git commit; (cd sub && git commit); git commit",
    "cd ./d1/../d2 && git commit; cd ../'d 3' || exit 1; git commit",
    "pushd d1 >/dev/null; git commit; pushd ../d2 >/dev/null; git commit; popd >/dev/null; git commit",
    "cd d1 | git commit; git status",
    "cd d1 & wait; git commit",
    "x=$(cd d1 && git commit); git commit; y=`cd d2; git commit`; git commit",
    "echo \"a $(git commit -m \"nested $(git log -1)\") b\"; git status",
    "cat <(cd d1 && git commit -m x); git status",
    "if git commit; then git add .; fi; if false; then :; else git status; fi",
    "for f in a b; do :; done; while cd d1; do git commit; break; done",
    "case a in a) cd d1; git commit;; esac",
    "{ cd d1; git commit; }; git status",
    "bash -c 'cd d1 && git commit -m \"x y\"'; git status; sh -c \"git commit -m 'z'\"",
    "bash -euo pipefail -c 'git commit'; bash -c -- 'git commit'; bash -c 'bash -c \"git commit\"'",
    "env -C d1 git commit; git status",
    "echo a b | xargs git add; echo a | xargs -n 1 git add -v",
    "git commit > out -m x -- f; > out git commit -q; git commit 2>&1 -m y | cat",
    "git commit -m x <<< \"str\" -q; git commit < /dev/null -m z",
    "cat <<'EOF' > out\ngit add .\nit's $(git commit)\nEOF\ngit commit",
    "cat <<EOF > out\ngit add .\nEOF\ngit commit",
    "git commit -F - <<'EOF'\nit's (a \"b\n\ngit add .\nEOF\ngit status",
    "git commit -F - <<'EOF' && git push && git status || echo\nm\nEOF",
    "git commit -F - <<'EOF'; git push; git status\nit's\nEOF\ngit log",
    "git commit -F - <<'EOF' 2>&1 | cat && git push\nit's\nEOF",
    "git commit -q <<'EOF' -F - --no-verify\nit's\nEOF",
    "git commit -F - <<-EOF\n\tit is\n\tEOF\ngit status",
    "cat <<A <<'B'; git commit\nit's a\nA\nit's b\nB\ngit status",
    "git commit -F - <<\"E OF\"\nEOF\nE OF\ngit status",
    "(cd d1 && git commit -F - <<'EOF'\nit's\nEOF\n); git status",
    "git commit -m \"$(cat <<'EOF'\nfeat: (a) \"q\" `x` $y\n\ngit add .\nEOF\n)\" && git status",
    "git commit -m \"$(cat <<-'EOF'\n\tfeat\n\tEOF\n)\" -m x; git status",
    "if true; then\n  git commit -F - <<'EOF'\nit's\nEOF\nfi && git status",
    "# git add .\ngit commit # git status\ngit log",
    "echo git commit; printf '%s' 'git commit'; echo \"git add . && git commit\"; git log",
    "git -C d1 commit; git -C d1 -C ../d2 commit; git -c a.b='c d' --no-pager commit -m x",
    "true && git commit -m x || false; false || git status; ! git log",
    "git commit -m x &\nwait\ngit status",
    "[[ -n x && x =~ ^x$ ]] && git commit -m \"a [b] {c}\"; [ -n x ] && git log",
    "git commit -m \"$((1 << 2))\" -q; echo $((1<<3)); git status",
    "git commit -m \"a\" -m \"b $HOME c\" -m x$HOME; git status",
];

/// Calls as bash made them: working directory, then every word.
fn bash_calls(dir: &Path, bin: &Path, log: &Path, command: &str) -> Vec<Vec<String>> {
    let _ = std::fs::remove_file(log);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let status = Command::new("bash")
        .args(["-c", command])
        .current_dir(dir)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.code().is_some(), "{command:?}: {status}");
    let recorded = std::fs::read_to_string(log).unwrap_or_default();
    recorded
        .split("\u{1}\n")
        .filter(|r| !r.is_empty())
        .map(|r| r.split('\0').map(str::to_owned).collect::<Vec<_>>())
        .map(|mut fields| {
            fields.pop();
            fields
        })
        .collect()
}

/// The directory and words a recorded call resolves to once git has applied
/// its global options.
fn resolved(call: &[String]) -> (PathBuf, Vec<String>) {
    let mut dir = PathBuf::from(&call[0]);
    let mut words = call[1..].to_vec();
    while words.first().is_some_and(|w| w.starts_with('-')) {
        match words.remove(0).as_str() {
            "-C" => dir = dir.join(words.remove(0)),
            "-c" => drop(words.remove(0)),
            _ => {}
        }
    }
    (dir.canonicalize().unwrap(), words)
}

fn agrees(call: &GitCall, (dir, words): &(PathBuf, Vec<String>)) -> bool {
    let same_dir = call.dir.as_ref().is_some_and(|d| d.canonicalize().ok().as_ref() == Some(dir));
    let first = words.first().is_some_and(|w| *w == call.subcommand);
    // A word that depends on the run is only known up to its first expansion,
    // and xargs adds words of its own.
    let rest = call.args.iter().zip(&words[1..]).all(|(ours, theirs)| {
        if ours.literal {
            ours.text == *theirs
        } else {
            theirs.starts_with(&ours.text)
        }
    });
    same_dir && first && rest && call.args.len() < words.len()
}

#[test]
fn the_same_git_commands_run_in_the_same_places() {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("shell_agrees");
    let _ = std::fs::remove_dir_all(&base);
    for d in ["bin", "work/d1/sub", "work/d2", "work/d 3"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    let (bin, work, log) = (base.join("bin"), base.join("work"), base.join("log"));
    let stub = format!(
        "#!/bin/bash\n{{ printf '%s\\0' \"$PWD\" \"$@\"; printf '\\1\\n'; }} >> '{}'\n",
        log.display()
    );
    std::fs::write(bin.join("git"), stub).unwrap();
    std::fs::set_permissions(bin.join("git"), PermissionsExt::from_mode(0o755)).unwrap();

    let mut disagreements = Vec::new();
    for command in COMMANDS {
        let theirs = bash_calls(&work, &bin, &log, command);
        let theirs: Vec<_> = theirs.iter().map(|c| resolved(c)).collect();
        let ours = git_calls(command, &work);
        // Pipelines and substitutions may record in either order.
        let matched = theirs.iter().all(|t| ours.iter().any(|o| agrees(o, t)))
            && ours.iter().all(|o| theirs.iter().any(|t| agrees(o, t)));
        let extra_runs = command.contains("xargs -n 1");
        if !matched || theirs.is_empty() || (ours.len() != theirs.len() && !extra_runs) {
            disagreements.push(format!("{command:?}\n  bash: {theirs:?}\n  ours: {ours:?}"));
        }
    }
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n\n"));
}
