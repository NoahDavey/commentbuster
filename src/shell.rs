//! Which git commands a shell command line runs, and in which directory.
//!
//! The command is parsed with tree-sitter-bash and walked in the order the
//! shell would run it. Only what the text settles counts: a command name that
//! comes from a variable, an `eval`, an alias, a function or a script is not
//! followed, and a directory that is not literal is unknown.

use std::path::{Component, Path, PathBuf};
use tree_sitter::{Node, Parser};

/// How deep `bash -c` strings are followed.
const MAX_NESTING: usize = 8;

/// Variables that point git at a different repository or index.
const GIT_LOCATION_VARS: [&str; 5] =
    ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY"];

/// One word of a command after quote removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    /// The whole value, or the literal text before the first expansion.
    pub text: String,
    /// No expansion contributes to the value.
    pub literal: bool,
}

impl Word {
    pub fn literal(text: impl Into<String>) -> Word {
        Word { text: text.into(), literal: true }
    }

    fn is(&self, text: &str) -> bool {
        self.literal && self.text == text
    }

    fn value(&self) -> Option<&str> {
        self.literal.then_some(self.text.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct GitCall {
    /// Where it runs; `None` when that is only known at run time.
    pub dir: Option<PathBuf>,
    pub subcommand: String,
    pub args: Vec<Word>,
    /// The loops it is inside, outermost first.
    pub loops: Vec<usize>,
}

/// The git commands `command` runs when started in `cwd`, in order.
pub fn git_calls(command: &str, cwd: &Path) -> Vec<GitCall> {
    let mut walker = Walker {
        shell: Shell {
            dir: Some(cwd.to_path_buf()),
            previous: None,
            pushed: Vec::new(),
            git_relocated: false,
        },
        loops: Vec::new(),
        loop_count: 0,
        calls: Vec::new(),
    };
    walker.script(command, 0);
    walker.calls
}

/// The state a `cd` or an assignment changes and a subshell does not leak.
#[derive(Clone, PartialEq, Eq)]
struct Shell {
    dir: Option<PathBuf>,
    previous: Option<PathBuf>,
    pushed: Vec<Option<PathBuf>>,
    git_relocated: bool,
}

struct Walker {
    shell: Shell,
    loops: Vec<usize>,
    loop_count: usize,
    calls: Vec<GitCall>,
}

struct Script<'a> {
    source: &'a str,
    /// Where the parse first went wrong. Nothing from there on is trusted:
    /// text past a misread quote can look like commands.
    broken_at: usize,
    depth: usize,
}

impl Walker {
    fn script(&mut self, command: &str, depth: usize) {
        let mut parser = Parser::new();
        if parser.set_language(&tree_sitter_bash::LANGUAGE.into()).is_err() {
            return;
        }
        let mut source = command.to_string();
        let mut tree = parser.parse(&source, None);
        while let Some(shorter) =
            tree.as_ref().and_then(|t| without_first_heredoc(t.root_node(), &source))
        {
            source = shorter;
            tree = parser.parse(&source, None);
        }
        let Some(tree) = tree else { return };
        let root = tree.root_node();
        let script =
            Script { source: &source, broken_at: first_error(root).unwrap_or(usize::MAX), depth };
        self.statements(&script, root);
    }

    /// Run `f` in a subshell: what it does to the directory stays inside.
    fn subshell(&mut self, f: impl FnOnce(&mut Self)) {
        let saved = self.shell.clone();
        f(self);
        self.shell = saved;
    }

    /// After code where either outcome is possible, only what both agree on is
    /// known.
    fn merge(&mut self, other: Shell) {
        if self.shell.dir != other.dir {
            self.shell.dir = None;
        }
        if self.shell.previous != other.previous {
            self.shell.previous = None;
        }
        if self.shell.pushed != other.pushed {
            self.shell.pushed.clear();
        }
        self.shell.git_relocated |= other.git_relocated;
    }

    /// The statements of a block, in order.
    fn statements(&mut self, s: &Script, node: Node) {
        for child in named_children(node) {
            let background = child.next_sibling().is_some_and(|n| n.kind() == "&");
            if background {
                self.subshell(|w| w.statement(s, child, &[]));
            } else {
                self.statement(s, child, &[]);
            }
        }
    }

    /// `extra` holds arguments the grammar attached to a redirect after the
    /// command: in `git commit > log -m x` they belong to `git commit`.
    fn statement(&mut self, s: &Script, node: Node, extra: &[Word]) {
        match node.kind() {
            "command" => self.command(s, node, extra),
            "redirected_statement" => {
                let mut trailing = Vec::new();
                for redirect in fields(node, "redirect") {
                    self.redirect(s, redirect, &mut trailing);
                }
                trailing.extend_from_slice(extra);
                if let Some(body) = node.child_by_field_name("body") {
                    self.statement(s, body, &trailing);
                }
            }
            "list" => self.list(s, node, extra),
            "pipeline" => {
                for child in named_children(node) {
                    self.subshell(|w| w.statement(s, child, &[]));
                }
            }
            "subshell" | "command_substitution" | "process_substitution" => {
                self.subshell(|w| w.statements(s, node));
            }
            "if_statement" | "elif_clause" => self.branch(s, node),
            "case_statement" => self.alternatives(s, &named_children(node), false),
            "for_statement" | "c_style_for_statement" | "while_statement" => {
                let before = self.shell.clone();
                self.loop_count += 1;
                self.loops.push(self.loop_count);
                self.statements(s, node);
                self.loops.pop();
                self.merge(before);
            }
            "function_definition" | "comment" => {}
            "variable_assignment" => {
                self.relocate(s, node);
                self.statements(s, node);
            }
            _ => self.statements(s, node),
        }
    }

    fn list(&mut self, s: &Script, node: Node, extra: &[Word]) {
        let children = named_children(node);
        let [left, right] = children[..] else {
            return self.statements(s, node);
        };
        self.statement(s, left, &[]);
        if left.next_sibling().is_some_and(|op| op.kind() == "||") {
            let before = self.shell.clone();
            self.statement(s, right, extra);
            self.merge(before);
        } else {
            self.statement(s, right, extra);
        }
    }

    /// The condition always runs, then either the body or a later clause.
    fn branch(&mut self, s: &Script, node: Node) {
        let mut cursor = node.walk();
        let then = node.children(&mut cursor).find(|c| c.kind() == "then");
        let then = then.map_or(usize::MAX, |t| t.start_byte());
        let (conditions, rest): (Vec<Node>, Vec<Node>) =
            named_children(node).into_iter().partition(|c| c.start_byte() < then);
        for condition in conditions {
            self.statement(s, condition, &[]);
        }
        let (clauses, body): (Vec<Node>, Vec<Node>) =
            rest.into_iter().partition(|c| matches!(c.kind(), "elif_clause" | "else_clause"));

        let before = self.shell.clone();
        for statement in body {
            self.statement(s, statement, &[]);
        }
        let after = std::mem::replace(&mut self.shell, before);
        self.alternatives(s, &clauses, clauses.last().is_some_and(|c| c.kind() == "else_clause"));
        self.merge(after);
    }

    /// Run each node from the current state and keep what they all agree on.
    /// Unless `exhaustive`, none of them running is possible too.
    fn alternatives(&mut self, s: &Script, nodes: &[Node], exhaustive: bool) {
        let before = self.shell.clone();
        let mut outcomes = if exhaustive { Vec::new() } else { vec![before.clone()] };
        for node in nodes {
            self.shell = before.clone();
            self.statement(s, *node, &[]);
            outcomes.push(self.shell.clone());
        }
        self.shell = outcomes.pop().unwrap_or(before);
        for outcome in outcomes {
            self.merge(outcome);
        }
    }

    fn redirect(&mut self, s: &Script, redirect: Node, trailing: &mut Vec<Word>) {
        self.statements(s, redirect);
        if redirect.kind() == "file_redirect" {
            let words = fields(redirect, "destination");
            trailing.extend(words.into_iter().skip(1).map(|n| word(n, s.source)));
        }
    }

    fn command(&mut self, s: &Script, node: Node, extra: &[Word]) {
        if node.end_byte() > s.broken_at {
            return;
        }
        let outer = self.shell.git_relocated;
        let mut words = Vec::new();
        let mut subshells = Vec::new();
        let mut cursor = node.walk();
        let mut more = cursor.goto_first_child();
        while more {
            let child = cursor.node();
            match (cursor.field_name(), child.kind()) {
                (Some("redirect"), _) => self.redirect(s, child, &mut words),
                (Some("name" | "argument"), _) => {
                    self.statement(s, child, &[]);
                    words.push(word(child, s.source));
                }
                // `time (git commit)` parses as a command and a subshell.
                (_, "subshell") => subshells.push(child),
                (_, "variable_assignment") => self.statement(s, child, &[]),
                _ => {}
            }
            more = cursor.goto_next_sibling();
        }
        words.extend_from_slice(extra);
        self.run(s, &words);
        self.shell.git_relocated = outer;
        for child in subshells {
            self.statement(s, child, &[]);
        }
    }

    /// Note an assignment that points git somewhere else.
    fn relocate(&mut self, s: &Script, assignment: Node) {
        if let Some(name) = assignment.child_by_field_name("name") {
            self.shell.git_relocated |= GIT_LOCATION_VARS.contains(&&s.source[name.byte_range()]);
        }
    }

    /// One simple command, as words.
    fn run(&mut self, s: &Script, words: &[Word]) {
        let Some((name, args)) = words.split_first() else { return };
        // `time { git commit; }` parses as a command called `time` with `{` as
        // its first word.
        if name.text == "{" {
            return self.run(s, args);
        }
        let Some(name) = name.value() else { return };
        match name.rsplit('/').next().unwrap_or(name) {
            "git" => self.git(args),
            "cd" => self.cd(args),
            "pushd" => {
                let from = self.shell.dir.clone();
                match args {
                    [dir] => self.enter(dir.value()),
                    _ => self.enter(None),
                }
                self.shell.pushed.push(from);
            }
            "popd" => {
                self.shell.previous = self.shell.dir.take();
                self.shell.dir = self.shell.pushed.pop().flatten();
            }
            "bash" | "sh" | "zsh" | "dash" | "ksh" => {
                if let Some(script) = shell_string(args) {
                    if s.depth < MAX_NESTING {
                        self.subshell(|w| w.script(script, s.depth + 1));
                    }
                }
            }
            "env" => self.subshell(|w| w.env(s, args)),
            "command" => {
                let rest = skip_options(args, "");
                let describes = args[..args.len() - rest.len()]
                    .iter()
                    .any(|w| w.text.contains(['v', 'V']));
                if !describes {
                    self.run(s, rest);
                }
            }
            "builtin" | "nohup" => self.run(s, args),
            "exec" => self.run(s, skip_options(args, "a")),
            "time" => self.run(s, skip_options(args, "fo")),
            "nice" => self.run(s, skip_options(args, "n")),
            "stdbuf" => self.run(s, skip_options(args, "ioe")),
            "xargs" => self.run(s, skip_options(args, "adEILnPRsS")),
            "timeout" | "gtimeout" => self.run(s, skip_options(args, "ks").get(1..).unwrap_or(&[])),
            _ => {}
        }
    }

    fn env(&mut self, s: &Script, args: &[Word]) {
        let mut rest = args;
        while let Some((word, after)) = rest.split_first() {
            let text = word.text.as_str();
            rest = after;
            match text {
                "-u" | "--unset" => rest = rest.get(1..).unwrap_or(&[]),
                "-C" | "--chdir" => {
                    let Some((dir, after)) = rest.split_first() else { return };
                    self.enter(dir.value());
                    rest = after;
                }
                // The command is inside a string env splits itself.
                _ if text.starts_with("-S") || text.starts_with("--split-string") => return,
                _ if text.starts_with("--chdir=") => {
                    self.enter(word.value().map(|t| &t["--chdir=".len()..]));
                }
                _ if text.starts_with('-') => {}
                _ => match text.split_once('=') {
                    Some((name, _)) => {
                        self.shell.git_relocated |= GIT_LOCATION_VARS.contains(&name);
                    }
                    None => {
                        let at = args.len() - after.len() - 1;
                        return self.run(s, &args[at..]);
                    }
                },
            }
        }
    }

    fn cd(&mut self, args: &[Word]) {
        match skip_options(args, "") {
            [] => {
                let home = home();
                self.enter(home.as_deref());
            }
            [word] if word.is("-") => {
                let here = self.shell.dir.take();
                self.shell.dir = std::mem::replace(&mut self.shell.previous, here);
            }
            [word] => self.enter(word.value()),
            _ => self.enter(None),
        }
    }

    /// Change directory the way `cd` does, where `..` undoes the last
    /// component even if that was a symlink.
    fn enter(&mut self, path: Option<&str>) {
        let next = path.and_then(|p| join(self.shell.dir.as_deref(), p)).map(|p| normalized(&p));
        self.shell.previous = std::mem::replace(&mut self.shell.dir, next);
    }

    fn git(&mut self, args: &[Word]) {
        let mut dir = self.shell.dir.clone();
        let mut relocated = self.shell.git_relocated;
        let mut rest = args;
        while let Some((word, after)) = rest.split_first() {
            let Some(text) = word.value() else {
                // `--opt="$x"` is still one option; anything else could be the
                // subcommand.
                if word.text.starts_with("--") && word.text.contains('=') {
                    relocated |= is_location_option(&word.text);
                    rest = after;
                    continue;
                }
                return;
            };
            rest = after;
            match text {
                "-C" => {
                    let Some((path, after)) = rest.split_first() else { return };
                    dir = path.value().and_then(|p| join(dir.as_deref(), p));
                    rest = after;
                }
                "-c" | "--config-env" | "--namespace" | "--attr-source" | "--super-prefix" => {
                    rest = rest.get(1..).unwrap_or(&[]);
                }
                "--git-dir" | "--work-tree" => {
                    relocated = true;
                    rest = rest.get(1..).unwrap_or(&[]);
                }
                "--help" | "-h" | "--version" | "-v" => return,
                _ if text.starts_with('-') => relocated |= is_location_option(text),
                _ => {
                    self.calls.push(GitCall {
                        dir: if relocated { None } else { dir },
                        subcommand: text.to_string(),
                        args: rest.to_vec(),
                        loops: self.loops.clone(),
                    });
                    return;
                }
            }
        }
    }
}

fn is_location_option(text: &str) -> bool {
    text.starts_with("--git-dir") || text.starts_with("--work-tree") || text == "--bare"
}

/// The script a `bash -c` invocation runs, if it is literal.
fn shell_string(args: &[Word]) -> Option<&str> {
    let mut takes_string = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.value()?;
        match text {
            "--" | "-" => {
                let script = words.next()?;
                return takes_string.then(|| script.value()).flatten();
            }
            "--rcfile" | "--init-file" => {
                words.next();
            }
            _ if text.starts_with("--") => {}
            _ if text.starts_with(['-', '+']) && text.len() > 1 => {
                takes_string |= text.starts_with('-') && text.contains('c');
                if text.contains(['o', 'O']) {
                    words.next();
                }
            }
            _ => return takes_string.then_some(text),
        }
    }
    None
}

/// Drop leading options; a lone `-` is an operand. `with_value` lists the short ones whose value can be
/// the next word.
fn skip_options<'a>(words: &'a [Word], with_value: &str) -> &'a [Word] {
    let mut rest = words;
    while let Some((word, after)) = rest.split_first() {
        let text = word.text.as_str();
        if text == "--" {
            return after;
        }
        if text.len() < 2 || !text.starts_with('-') {
            return rest;
        }
        rest = after;
        let last = text.chars().next_back().unwrap_or_default();
        if !text.starts_with("--") && with_value.contains(last) && word.literal {
            rest = rest.get(1..).unwrap_or(&[]);
        }
    }
    rest
}

fn home() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
}

fn join(dir: Option<&Path>, path: &str) -> Option<PathBuf> {
    if Path::new(path).is_absolute() {
        return Some(PathBuf::from(path));
    }
    Some(dir?.join(path))
}

/// Resolve `.` and `..` by name alone.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(part),
        }
    }
    out
}

fn named_children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn fields<'t>(node: Node<'t>, name: &str) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.children_by_field_name(name, &mut cursor).collect()
}

fn first_error(node: Node) -> Option<usize> {
    if node.is_error() || node.is_missing() {
        return Some(node.start_byte());
    }
    if !node.has_error() {
        return None;
    }
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).find_map(first_error);
    first
}

fn first_heredoc<'t>(node: Node<'t>) -> Option<Node<'t>> {
    if node.kind() == "heredoc_start" {
        return Some(node);
    }
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).find_map(first_heredoc);
    first
}

/// `source` with its first heredoc removed, operator and body.
///
/// The body is input, not commands, and the grammar reads only some of what
/// can follow the operator on its line: `<<'EOF'; cmd`, `<<'EOF' 2>&1 | cmd`
/// and two heredocs on one line are parse errors. A command substitution in an
/// unquoted body goes with it.
fn without_first_heredoc(root: Node, source: &str) -> Option<String> {
    let start = first_heredoc(root)?;
    let before = &source[..start.start_byte()];
    let operator = before.trim_end_matches([' ', '\t']);
    let strip_tabs = operator.ends_with("<<-");
    let head = operator.strip_suffix("<<-").or_else(|| operator.strip_suffix("<<"))?;
    let delimiter: String = source[start.byte_range()]
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();

    let rest = &source[start.end_byte()..];
    let (tail, body) = rest.split_at(line_end(rest));
    let mut after = "";
    let mut remaining = body.strip_prefix('\n').unwrap_or(body);
    while !remaining.is_empty() {
        let line = if strip_tabs { remaining.trim_start_matches('\t') } else { remaining };
        let next = remaining.find('\n').map_or("", |i| &remaining[i + 1..]);
        match line.strip_prefix(delimiter.as_str()) {
            Some("") => break,
            Some(end) if end.starts_with('\n') => {
                after = next;
                break;
            }
            // Shells also end the body where the enclosing `$(` closes.
            Some(end) if end.starts_with(')') => {
                after = end;
                break;
            }
            _ => remaining = next,
        }
    }
    Some(format!("{head} {tail}\n{after}"))
}

/// The end of the first line, where a backslash joins it to the next.
fn line_end(text: &str) -> usize {
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        match c {
            '\n' if !escaped => return i,
            '\\' => escaped = !escaped,
            _ => escaped = false,
        }
    }
    text.len()
}

/// The value of a word node after quote removal.
fn word(node: Node, source: &str) -> Word {
    let raw = &source[node.byte_range()];
    match node.kind() {
        "command_name" => match node.named_child(0) {
            Some(child) => word(child, source),
            None => Word { text: String::new(), literal: false },
        },
        "word" | "number" => unquoted(raw),
        "raw_string" => Word::literal(raw.trim_matches('\'')),
        "ansi_c_string" => ansi_c(raw),
        "string" => double_quoted(node, source),
        "translated_string" => match node.named_child(0) {
            Some(child) => word(child, source),
            None => Word { text: String::new(), literal: false },
        },
        "concatenation" => {
            let mut out = Word::literal("");
            let mut end = node.start_byte();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                let piece = word(child, source);
                out.text.push_str(&piece.text);
                out.literal = piece.literal && child.start_byte() == end;
                end = child.end_byte();
                if !out.literal {
                    return out;
                }
            }
            out.literal = end == node.end_byte();
            out
        }
        _ => Word { text: String::new(), literal: false },
    }
}

fn unquoted(raw: &str) -> Word {
    let mut text = String::new();
    let mut literal = true;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(escaped) => text.push(escaped),
            },
            '*' | '?' | '[' | '{' => {
                literal = false;
                text.push(c);
            }
            _ => text.push(c),
        }
    }
    if let Some(rest) = raw.strip_prefix('~') {
        match home() {
            Some(home) if rest.is_empty() || rest.starts_with('/') => {
                text.replace_range(..1, &home);
            }
            _ => literal = false,
        }
    }
    Word { text, literal }
}

fn double_quoted(node: Node, source: &str) -> Word {
    let mut cursor = node.walk();
    let expansion = node.named_children(&mut cursor).find(|c| c.kind() != "string_content");
    let end = match expansion {
        Some(e) => e.start_byte(),
        None => node.end_byte().saturating_sub(1),
    };
    let raw = source.get(node.start_byte() + 1..end).unwrap_or_default();
    let mut text = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some('\n')) => {
                chars.next();
            }
            ('\\', Some('$' | '`' | '"' | '\\')) => text.extend(chars.next()),
            _ => text.push(c),
        }
    }
    Word { text, literal: expansion.is_none() }
}

fn ansi_c(raw: &str) -> Word {
    let inner = raw.strip_prefix("$'").and_then(|r| r.strip_suffix('\'')).unwrap_or(raw);
    let mut text = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => text.push('\n'),
            Some('t') => text.push('\t'),
            Some('r') => text.push('\r'),
            Some(c @ ('\\' | '\'' | '"' | '?')) => text.push(c),
            _ => return Word { text, literal: false },
        }
    }
    Word { text, literal: true }
}
