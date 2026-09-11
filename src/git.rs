//! Everything that talks to git. Two invocations do the work: one `diff --raw`
//! to enumerate what changed (with rename detection), and one `cat-file
//! --batch` to fetch every blob we need. Line-level diffing happens in
//! process, so there is no per-file git call.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

pub const NULL_SHA: &str = "0000000000000000000000000000000000000000";

#[derive(Clone, Debug)]
pub struct RawEntry {
    pub old_mode: String,
    pub new_mode: String,
    pub old_sha: String,
    pub new_sha: String,
    /// Raw git status letter with optional score: `M`, `A`, `D`, `T`, `R100`, `C80`.
    pub status: String,
    pub old_path: String,
    pub new_path: String,
}

impl RawEntry {
    pub fn status_word(&self) -> &'static str {
        match self.status.as_bytes().first() {
            Some(b'A') => "added",
            Some(b'D') => "deleted",
            Some(b'R') => "renamed",
            Some(b'C') => "copied",
            Some(b'T') => "type_changed",
            _ => "modified",
        }
    }

    pub fn is_rename_or_copy(&self) -> bool {
        matches!(self.status.as_bytes().first(), Some(b'R') | Some(b'C'))
    }

    /// Submodules and symlinks have no source text to parse.
    pub fn is_nonfile(&self) -> bool {
        for m in [&self.old_mode, &self.new_mode] {
            if m.starts_with("160000") || m.starts_with("120000") {
                return true;
            }
        }
        false
    }
}

fn git(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .args(args)
        .stderr(Stdio::piped())
        .output()
        .context("failed to run git; is it installed and on PATH?")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("git {} failed: {}", args.join(" "), err);
    }
    Ok(out.stdout)
}

fn git_str(args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git(args)?).trim().to_string())
}

pub fn repo_root() -> Result<String> {
    git_str(&["rev-parse", "--show-toplevel"]).context("not inside a git work tree")
}

pub fn rev_parse(rev: &str) -> Result<String> {
    git_str(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
        .with_context(|| format!("cannot resolve ref `{rev}`"))
        .and_then(|s| {
            if s.is_empty() {
                bail!("cannot resolve ref `{rev}`")
            } else {
                Ok(s)
            }
        })
}

pub fn merge_base(a: &str, b: &str) -> Result<String> {
    let s = git_str(&["merge-base", a, b]).with_context(|| {
        format!("cannot find a merge base for `{a}` and `{b}`; try --direct for a literal comparison")
    })?;
    if s.is_empty() {
        bail!("no merge base between `{a}` and `{b}`; try --direct");
    }
    Ok(s)
}

pub fn short_sha(sha: &str) -> String {
    sha.chars().take(9).collect()
}

/// `git diff --raw`, NUL-delimited so paths need no unquoting.
pub fn raw_diff(diff_args: &[String], paths: &[String]) -> Result<Vec<RawEntry>> {
    let mut args: Vec<String> = vec![
        "diff".into(),
        "--raw".into(),
        "-z".into(),
        "--abbrev=40".into(),
        "--find-renames".into(),
        "--find-copies".into(),
        "--no-ext-diff".into(),
    ];
    args.extend(diff_args.iter().cloned());
    if !paths.is_empty() {
        args.push("--".into());
        args.extend(paths.iter().cloned());
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = git(&refs)?;

    let mut fields = out.split(|b| *b == 0).map(|s| String::from_utf8_lossy(s).to_string());
    let mut entries = Vec::new();
    while let Some(header) = fields.next() {
        if header.is_empty() {
            continue;
        }
        if !header.starts_with(':') {
            bail!("unexpected git diff --raw record: {header:?}");
        }
        let parts: Vec<&str> = header[1..].split(' ').collect();
        if parts.len() < 5 {
            bail!("malformed git diff --raw record: {header:?}");
        }
        let status = parts[4].to_string();
        let first = fields.next().unwrap_or_default();
        let (old_path, new_path) = if matches!(status.as_bytes()[0], b'R' | b'C') {
            let second = fields.next().unwrap_or_default();
            (first, second)
        } else {
            (first.clone(), first)
        };
        entries.push(RawEntry {
            old_mode: parts[0].to_string(),
            new_mode: parts[1].to_string(),
            old_sha: parts[2].to_string(),
            new_sha: parts[3].to_string(),
            status,
            old_path,
            new_path,
        });
    }
    Ok(entries)
}

/// Fetch many blobs in one process. stdin is written from a thread so a large
/// request list cannot deadlock against a large response.
pub fn cat_file_batch(shas: &[String]) -> Result<HashMap<String, Vec<u8>>> {
    let mut result = HashMap::new();
    if shas.is_empty() {
        return Ok(result);
    }

    let mut child = Command::new("git")
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn git cat-file")?;

    let mut stdin = child.stdin.take().expect("stdin piped");
    let request: Vec<u8> = shas.iter().flat_map(|s| format!("{s}\n").into_bytes()).collect();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&request);
        drop(stdin);
    });

    let mut buf = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout piped")
        .read_to_end(&mut buf)
        .context("failed reading git cat-file output")?;
    let _ = writer.join();
    let status = child.wait()?;
    if !status.success() {
        bail!("git cat-file exited with {status}");
    }

    let mut pos = 0usize;
    while pos < buf.len() {
        let nl = match buf[pos..].iter().position(|b| *b == b'\n') {
            Some(n) => pos + n,
            None => break,
        };
        let header = String::from_utf8_lossy(&buf[pos..nl]).to_string();
        pos = nl + 1;
        let parts: Vec<&str> = header.split(' ').collect();
        if parts.len() < 3 {
            // "<sha> missing" — skip and let the caller treat it as empty.
            continue;
        }
        let size: usize = parts[2].parse().unwrap_or(0);
        let end = (pos + size).min(buf.len());
        result.insert(parts[0].to_string(), buf[pos..end].to_vec());
        pos = end + 1; // trailing newline after the object body
    }

    Ok(result)
}
