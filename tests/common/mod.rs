#![allow(dead_code)]

//! A throwaway git repo per test, driven through the real binary.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Repo {
    pub dir: PathBuf,
}

impl Repo {
    pub fn new(name: &str) -> Repo {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create repo dir");
        let r = Repo { dir };
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["config", "user.email", "test@example.com"]);
        r.git(&["config", "user.name", "Test"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r.git(&["config", "core.autocrlf", "false"]);
        r
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    pub fn write(&self, path: &str, content: &str) {
        let full = self.dir.join(path);
        if let Some(p) = full.parent() {
            std::fs::create_dir_all(p).expect("mkdir");
        }
        std::fs::write(full, content).expect("write file");
    }

    pub fn write_bytes(&self, path: &str, content: &[u8]) {
        let full = self.dir.join(path);
        if let Some(p) = full.parent() {
            std::fs::create_dir_all(p).expect("mkdir");
        }
        std::fs::write(full, content).expect("write file");
    }

    pub fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn run_raw(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_commentbuster"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .expect("run commentbuster");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    pub fn run(&self, args: &[&str]) -> (i32, Value) {
        let (code, stdout, stderr) = self.run_raw(args);
        let json: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("bad json ({e}) from {args:?}\nstdout: {stdout}\nstderr: {stderr}"));
        (code, json)
    }
}

/// `totals` as (comment_added, comment_removed, net, code_added, code_removed).
pub fn totals(v: &Value) -> (u64, u64, i64, u64, u64) {
    let t = &v["totals"];
    (
        t["comment_lines_added"].as_u64().unwrap(),
        t["comment_lines_removed"].as_u64().unwrap(),
        t["comment_lines_net"].as_i64().unwrap(),
        t["code_lines_added"].as_u64().unwrap(),
        t["code_lines_removed"].as_u64().unwrap(),
    )
}

impl Repo {
    /// Run the binary the way Claude Code runs a hook: a JSON payload on stdin
    /// and any tuning in the environment. The gate's own variables are cleared
    /// first so a developer's shell cannot leak into the assertions.
    pub fn run_hook(&self, args: &[&str], stdin: &str, envs: &[(&str, &str)]) -> (i32, String, String) {
        use std::io::Write as _;
        use std::process::Stdio;

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_commentbuster"));
        cmd.args(args).current_dir(&self.dir);
        for var in [
            "COMMENTBUSTER_SKIP",
            "COMMENTBUSTER_MAX_RATIO",
            "COMMENTBUSTER_MAX_NET_SMALL",
            "COMMENTBUSTER_MIN_CODE_LINES",
        ] {
            cmd.env_remove(var);
        }
        cmd.envs(envs.iter().copied());
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn commentbuster");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(stdin.as_bytes())
            .expect("write payload");
        let out = child.wait_with_output().expect("wait for commentbuster");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }
}
