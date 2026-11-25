//! Combines the three inputs — what git says changed, what the AST says is a
//! comment, and which lines the line-diff moved — into the report.

use crate::classify::{classify, LineFlags};
use crate::git::{RawEntry, Repo, NULL_SHA};
use crate::lang::Language;
use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;

pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Mode {
    Worktree,
    Staged,
    /// Index against a tree or commit other than HEAD.
    StagedFrom(String),
    Range {
        base_ref: String,
        head_ref: String,
        base_sha: String,
        head_sha: String,
        merge_base: bool,
    },
}

impl Mode {
    fn diff_args(&self) -> Vec<String> {
        match self {
            Mode::Worktree => vec!["HEAD".into()],
            Mode::Staged => vec!["--cached".into(), "HEAD".into()],
            Mode::StagedFrom(base) => vec!["--cached".into(), base.clone()],
            Mode::Range { base_sha, head_sha, .. } => vec![base_sha.clone(), head_sha.clone()],
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Mode::Worktree => "worktree",
            Mode::Staged | Mode::StagedFrom(_) => "staged",
            Mode::Range { merge_base: true, .. } => "merge-base",
            Mode::Range { merge_base: false, .. } => "direct",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RangeInfo {
    pub mode: String,
    pub base_ref: Option<String>,
    pub head_ref: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Totals {
    pub comment_lines_added: u64,
    pub comment_lines_removed: u64,
    pub comment_lines_net: i64,
    pub code_lines_added: u64,
    pub code_lines_removed: u64,
    pub comment_ratio: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileStat {
    pub path: String,
    pub previous_path: Option<String>,
    pub language: &'static str,
    pub status: &'static str,
    pub comment_lines_added: u64,
    pub comment_lines_removed: u64,
    pub comment_lines_net: i64,
    pub code_lines_added: u64,
    pub code_lines_removed: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Skipped {
    pub count: u64,
    pub unsupported: u64,
    pub parse_error: u64,
    pub binary: u64,
    pub too_large: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub version: u32,
    pub range: RangeInfo,
    pub totals: Totals,
    pub files: Vec<FileStat>,
    pub skipped: Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Skip {
    ParseError,
    Binary,
    TooLarge,
}

enum Outcome {
    Counted(FileStat),
    Skipped(Skip),
}

pub fn analyze(repo: &Repo, mode: &Mode, paths: &[String]) -> Result<Analysis> {
    let entries = repo.raw_diff(&mode.diff_args(), paths)?;

    // Which files can we even parse? Decide before fetching any content.
    let mut in_scope: Vec<(RawEntry, Option<Language>, Option<Language>)> = Vec::new();
    let mut skipped = Skipped::default();
    for e in entries {
        if e.is_nonfile() {
            skipped.count += 1;
            skipped.unsupported += 1;
            continue;
        }
        let old_lang = Language::from_path(&e.old_path);
        let new_lang = Language::from_path(&e.new_path);
        if old_lang.is_none() && new_lang.is_none() {
            skipped.count += 1;
            skipped.unsupported += 1;
            continue;
        }
        in_scope.push((e, old_lang, new_lang));
    }

    let mut wanted: Vec<String> = Vec::new();
    for (e, old_lang, new_lang) in &in_scope {
        if old_lang.is_some() && e.old_sha != NULL_SHA {
            wanted.push(e.old_sha.clone());
        }
        if new_lang.is_some() && e.new_sha != NULL_SHA {
            wanted.push(e.new_sha.clone());
        }
    }
    wanted.sort();
    wanted.dedup();
    let blobs = repo.cat_file_batch(&wanted)?;

    let outcomes: Vec<Outcome> = in_scope
        .par_iter()
        .map(|(e, old_lang, new_lang)| analyze_one(&repo.root, e, *old_lang, *new_lang, &blobs))
        .collect();

    let mut files = Vec::new();
    let mut totals = Totals::default();
    for o in outcomes {
        match o {
            Outcome::Counted(f) => {
                totals.comment_lines_added += f.comment_lines_added;
                totals.comment_lines_removed += f.comment_lines_removed;
                totals.code_lines_added += f.code_lines_added;
                totals.code_lines_removed += f.code_lines_removed;
                files.push(f);
            }
            Outcome::Skipped(reason) => {
                skipped.count += 1;
                match reason {
                    Skip::ParseError => skipped.parse_error += 1,
                    Skip::Binary => skipped.binary += 1,
                    Skip::TooLarge => skipped.too_large += 1,
                }
            }
        }
    }

    totals.comment_lines_net =
        totals.comment_lines_added as i64 - totals.comment_lines_removed as i64;
    totals.comment_ratio = ratio(totals.comment_lines_added, totals.code_lines_added);

    files.sort_by(|a, b| {
        b.comment_lines_net
            .cmp(&a.comment_lines_net)
            .then_with(|| a.path.cmp(&b.path))
    });

    let range = match mode {
        Mode::Worktree | Mode::Staged => RangeInfo {
            mode: mode.label().to_string(),
            base_ref: Some("HEAD".into()),
            head_ref: None,
            base_sha: repo.rev_parse("HEAD").ok(),
            head_sha: None,
        },
        Mode::StagedFrom(base) => RangeInfo {
            mode: mode.label().to_string(),
            base_ref: Some(base.clone()),
            head_ref: None,
            base_sha: Some(base.clone()),
            head_sha: None,
        },
        Mode::Range { base_ref, head_ref, base_sha, head_sha, .. } => RangeInfo {
            mode: mode.label().to_string(),
            base_ref: Some(base_ref.clone()),
            head_ref: Some(head_ref.clone()),
            base_sha: Some(base_sha.clone()),
            head_sha: Some(head_sha.clone()),
        },
    };

    Ok(Analysis { version: 1, range, totals, files, skipped })
}

/// Comment lines added per code line added. `None` when a diff adds no code at
/// all, so callers never divide by zero or report a meaningless percentage.
pub fn ratio(comment_added: u64, code_added: u64) -> Option<f64> {
    if code_added == 0 {
        return None;
    }
    let r = comment_added as f64 / code_added as f64;
    Some((r * 1e6).round() / 1e6)
}

fn analyze_one(
    repo_root: &str,
    e: &RawEntry,
    old_lang: Option<Language>,
    new_lang: Option<Language>,
    blobs: &HashMap<String, Vec<u8>>,
) -> Outcome {
    let old = if e.old_sha == NULL_SHA {
        Vec::new()
    } else {
        blobs.get(&e.old_sha).cloned().unwrap_or_default()
    };
    let new = if e.new_sha == NULL_SHA {
        // Working-tree side: not an object yet, read it off disk.
        std::fs::read(format!("{repo_root}/{}", e.new_path)).unwrap_or_default()
    } else {
        blobs.get(&e.new_sha).cloned().unwrap_or_default()
    };

    if old.len() > MAX_FILE_BYTES || new.len() > MAX_FILE_BYTES {
        return Outcome::Skipped(Skip::TooLarge);
    }
    if old.contains(&0) || new.contains(&0) {
        return Outcome::Skipped(Skip::Binary);
    }

    let old_flags = match old_lang {
        Some(l) => match classify(&old, l) {
            Some(f) => f,
            None => return Outcome::Skipped(Skip::ParseError),
        },
        None => Vec::new(),
    };
    let new_flags = match new_lang {
        Some(l) => match classify(&new, l) {
            Some(f) => f,
            None => return Outcome::Skipped(Skip::ParseError),
        },
        None => Vec::new(),
    };

    let (added, removed) = changed_lines(&old, &new);

    let mut stat = FileStat {
        path: e.new_path.clone(),
        previous_path: if e.is_rename_or_copy() { Some(e.old_path.clone()) } else { None },
        language: new_lang.or(old_lang).map(|l| l.name()).unwrap_or("unknown"),
        status: e.status_word(),
        comment_lines_added: 0,
        comment_lines_removed: 0,
        comment_lines_net: 0,
        code_lines_added: 0,
        code_lines_removed: 0,
    };

    tally(&added, &new_flags, &mut stat.comment_lines_added, &mut stat.code_lines_added);
    tally(&removed, &old_flags, &mut stat.comment_lines_removed, &mut stat.code_lines_removed);
    stat.comment_lines_net =
        stat.comment_lines_added as i64 - stat.comment_lines_removed as i64;

    Outcome::Counted(stat)
}

fn tally(indices: &[usize], flags: &[LineFlags], comments: &mut u64, code: &mut u64) {
    for &i in indices {
        if let Some(f) = flags.get(i) {
            if f.comment {
                *comments += 1;
            }
            if f.code {
                *code += 1;
            }
        }
    }
}

/// Zero-based line indices that the diff adds (in `new`) and removes (in `old`).
///
/// Lines are split with the same helper the classifier uses, so an index here
/// always addresses the same line as an index into the flags.
pub fn changed_lines(old: &[u8], new: &[u8]) -> (Vec<usize>, Vec<usize>) {
    use similar::{capture_diff_slices, Algorithm, DiffOp};

    let old_lines: Vec<&[u8]> = crate::classify::line_ranges(old)
        .into_iter()
        .map(|(s, e)| &old[s..e])
        .collect();
    let new_lines: Vec<&[u8]> = crate::classify::line_ranges(new)
        .into_iter()
        .map(|(s, e)| &new[s..e])
        .collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    for op in capture_diff_slices(Algorithm::Histogram, &old_lines, &new_lines) {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { old_index, old_len, .. } => {
                removed.extend(old_index..old_index + old_len)
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                added.extend(new_index..new_index + new_len)
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                removed.extend(old_index..old_index + old_len);
                added.extend(new_index..new_index + new_len);
            }
        }
    }
    (added, removed)
}

/// Resolve the CLI's ref arguments into a comparison mode.
pub fn resolve_mode(repo: &Repo, refs: &[String], staged: bool, direct: bool) -> Result<Mode> {
    if staged && !refs.is_empty() {
        bail!("--staged compares the index against HEAD and takes no ref arguments");
    }
    if staged {
        return Ok(Mode::Staged);
    }
    match refs.len() {
        0 => Ok(Mode::Worktree),
        1 | 2 => {
            let base_ref = refs[0].clone();
            let head_ref = refs.get(1).cloned().unwrap_or_else(|| "HEAD".into());
            let head_sha = repo.rev_parse(&head_ref)?;
            let base_resolved = repo.rev_parse(&base_ref)?;
            let base_sha = if direct {
                base_resolved
            } else {
                repo.merge_base(&base_resolved, &head_sha)?
            };
            Ok(Mode::Range {
                base_ref,
                head_ref,
                base_sha,
                head_sha,
                merge_base: !direct,
            })
        }
        n => bail!("expected at most 2 refs, got {n}"),
    }
}
