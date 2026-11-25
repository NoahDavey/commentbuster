//! What a git command line means, as far as the gate needs to know: whether a
//! `git commit` commits more than the index, and whether some other
//! subcommand changes the index.

use crate::shell::Word;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub amend: bool,
    /// Prints status or help instead of committing.
    pub no_commit: bool,
    /// Why this commit records something other than the index as it stands.
    pub stages: Option<&'static str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arg {
    None,
    /// `--opt value` or `--opt=value`
    Required,
    /// `--opt` or `--opt=value`, never the next word
    Optional,
}

/// Every long option of `git commit`, so abbreviations resolve the way git
/// resolves them. From `git commit -h` and the hidden options in
/// builtin/commit.c.
const LONG: &[(&str, Arg)] = &[
    ("ahead-behind", Arg::None),
    ("all", Arg::None),
    ("allow-empty", Arg::None),
    ("allow-empty-message", Arg::None),
    ("amend", Arg::None),
    ("author", Arg::Required),
    ("branch", Arg::None),
    ("cleanup", Arg::Required),
    ("date", Arg::Required),
    ("dry-run", Arg::None),
    ("edit", Arg::None),
    ("file", Arg::Required),
    ("fixup", Arg::Required),
    ("gpg-sign", Arg::Optional),
    ("help", Arg::None),
    ("include", Arg::None),
    ("inter-hunk-context", Arg::Required),
    ("interactive", Arg::None),
    ("long", Arg::None),
    ("message", Arg::Required),
    ("null", Arg::None),
    ("only", Arg::None),
    ("patch", Arg::None),
    ("pathspec-file-nul", Arg::None),
    ("pathspec-from-file", Arg::Required),
    ("porcelain", Arg::None),
    ("post-rewrite", Arg::None),
    ("quiet", Arg::None),
    ("reedit-message", Arg::Required),
    ("reset-author", Arg::None),
    ("reuse-message", Arg::Required),
    ("short", Arg::None),
    ("signoff", Arg::None),
    ("squash", Arg::Required),
    ("status", Arg::None),
    ("template", Arg::Required),
    ("trailer", Arg::Required),
    ("unified", Arg::Required),
    ("untracked-files", Arg::Optional),
    ("verbose", Arg::None),
    ("verify", Arg::None),
];

/// The option `name` spells, in full or as an unambiguous prefix.
fn long_option(name: &str) -> Option<(&'static str, Arg)> {
    if let Some(exact) = LONG.iter().find(|(n, _)| *n == name) {
        return Some(*exact);
    }
    let mut matches = LONG.iter().filter(|(n, _)| n.starts_with(name));
    match (matches.next(), matches.next()) {
        (Some(only), None) => Some(*only),
        _ => None,
    }
}

/// Parse the words after `git commit`.
///
/// A word whose value is only known at run time is taken as a pathspec when it
/// cannot be anything else, and otherwise ignored.
pub fn parse_commit(args: &[Word]) -> Commit {
    let mut on: Vec<&'static str> = Vec::new();
    let mut paths = false;
    let mut paths_from_file = false;

    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.text.as_str();
        if word.literal && text == "--" {
            paths |= words.next().is_some();
            break;
        }
        if let Some(long) = text.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, _)) => (name, true),
                None => (long, !word.literal),
            };
            if let Some((option, arg)) = long_option(name) {
                if arg == Arg::Required && !attached {
                    words.next();
                }
                paths_from_file |= option == "pathspec-from-file";
                on.push(option);
            } else if let Some((option, _)) = name.strip_prefix("no-").and_then(long_option) {
                on.retain(|o| *o != option);
            }
        } else if text.len() > 1 && text.starts_with('-') {
            for (i, flag) in text.char_indices().skip(1) {
                match flag {
                    'F' | 'm' | 'c' | 'C' | 't' | 'U' => {
                        if text[i + 1..].is_empty() && word.literal {
                            words.next();
                        }
                        break;
                    }
                    'S' | 'u' => break,
                    'a' => on.push("all"),
                    'p' => on.push("patch"),
                    'i' => on.push("include"),
                    'o' => on.push("only"),
                    'z' => on.push("null"),
                    'h' => on.push("help"),
                    _ => {}
                }
            }
        } else if word.literal || !text.is_empty() {
            paths = true;
        }
    }

    let has = |o: &str| on.contains(&o);
    let stages = if has("all") {
        Some("`git commit -a` / `--all` stages files itself")
    } else if has("patch") || has("interactive") {
        Some("`git commit -p` / `--patch` / `--interactive` stages changes itself")
    } else if has("include") {
        Some("`git commit -i` / `--include` stages files itself")
    } else if has("only") {
        Some("`git commit -o` / `--only` commits paths instead of the index")
    } else if paths || paths_from_file {
        Some("`git commit` with a path commits that path instead of the index")
    } else {
        None
    };
    Commit {
        amend: has("amend"),
        no_commit: ["dry-run", "help", "short", "porcelain", "long", "null"]
            .iter()
            .any(|o| has(o)),
        stages,
    }
}

/// Whether `git <subcommand> <args>` can change the index, or move HEAD under
/// it.
pub fn changes_index(subcommand: &str, args: &[Word]) -> bool {
    let first = args.first().map_or("", |w| w.text.as_str());
    match subcommand {
        "add" | "stage" | "rm" | "mv" => !has_flag(args, 'n', "--dry-run"),
        "reset" | "merge" | "pull" | "rebase" | "cherry-pick" | "revert" | "am" | "read-tree"
        | "update-index" | "mergetool" => true,
        "restore" => has_flag(args, 'S', "--staged"),
        "apply" => {
            args.iter().any(|w| matches!(&*w.text, "--index" | "--cached" | "--3way" | "-3"))
        }
        "stash" => !matches!(first, "list" | "show" | "drop" | "clear" | "create" | "store"),
        "submodule" => first == "add",
        "checkout" => !starts_branch(args, &["-b", "-B"]) && first != "--",
        "switch" => !starts_branch(args, &["-c", "-C", "--create", "--force-create"]),
        _ => false,
    }
}

/// `-x` alone or in a cluster of short flags, or the long form, before `--`.
fn has_flag(args: &[Word], short: char, long: &str) -> bool {
    args.iter().take_while(|w| w.text != "--").any(|w| {
        w.text == long
            || (w.text.starts_with('-') && !w.text.starts_with("--") && w.text.contains(short))
    })
}

/// Creating a branch at HEAD keeps the index and the commit under it. Naming a
/// start point does not.
fn starts_branch(args: &[Word], create: &[&str]) -> bool {
    args.iter().any(|w| create.contains(&w.text.as_str()))
        && args.iter().filter(|w| !w.text.starts_with('-')).count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Commit {
        let words: Vec<Word> = args.iter().map(|a| Word::literal(*a)).collect();
        parse_commit(&words)
    }

    #[test]
    fn abbreviations_resolve_when_they_are_unambiguous() {
        assert!(parse(&["--am"]).amend);
        assert!(parse(&["--ame"]).amend);
        assert!(!parse(&["--a"]).amend, "git rejects an ambiguous prefix");
        assert!(parse(&["--al"]).stages.is_none(), "all, allow-empty: ambiguous");
        assert!(parse(&["--all"]).stages.is_some());
        assert!(parse(&["--dry"]).no_commit);
    }

    #[test]
    fn the_last_of_an_option_and_its_negation_wins() {
        assert!(!parse(&["--amend", "--no-amend"]).amend);
        assert!(parse(&["--no-amend", "--amend"]).amend);
        assert!(!parse(&["--dry-run", "--no-dry-run"]).no_commit);
    }

    #[test]
    fn a_short_cluster_stops_at_the_option_that_takes_a_value() {
        assert!(parse(&["-am", "x"]).stages.is_some());
        assert!(parse(&["-ma"]).stages.is_none());
        assert!(parse(&["-qm", "-a"]).stages.is_none());
        assert!(parse(&["-Sa"]).stages.is_none());
        assert!(parse(&["-uall"]).stages.is_none());
    }

    #[test]
    fn every_long_option_git_lists_is_known() {
        let out = std::process::Command::new("git").args(["commit", "-h"]).output().unwrap();
        // Older gits print usage to stderr.
        let help = [out.stdout, out.stderr].concat();
        let help = String::from_utf8_lossy(&help).to_string();
        let mut seen = 0;
        for line in help.lines().filter(|l| l.starts_with("    -")) {
            let Some(at) = line.find("--") else { continue };
            let option: String =
                line[at + 2..].chars().take_while(|c| !c.is_whitespace() && *c != '=').collect();
            let name = option.trim_start_matches("[no-]").trim_start_matches("no-");
            let name = name.split('[').next().unwrap();
            assert!(long_option(name).is_some_and(|(n, _)| n == name), "unknown option {name}");
            seen += 1;
        }
        assert!(seen > 25, "parsed only {seen} options from: {help}");
    }
}
