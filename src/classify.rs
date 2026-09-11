//! Turns file content into a per-line verdict: is this line a comment line, a
//! code line, or both?
//!
//! The AST decides what is a comment. The diff (see `analyze`) decides what
//! changed. The answer is the intersection.

use crate::lang::Language;
use tree_sitter::{Node, Parser};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineFlags {
    /// Line intersects a comment node and that comment carries prose.
    pub comment: bool,
    /// Line carries non-whitespace content outside of any comment.
    pub code: bool,
}

/// Byte ranges of each line, excluding the trailing newline. A trailing
/// newline at end of file does not produce a final empty line, which keeps the
/// line count identical to the diff's view of the same content.
pub fn line_ranges(content: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, b) in content.iter().enumerate() {
        if *b == b'\n' {
            let mut end = i;
            if end > start && content[end - 1] == b'\r' {
                end -= 1;
            }
            out.push((start, end));
            start = i + 1;
        }
    }
    if start < content.len() {
        let mut end = content.len();
        if end > start && content[end - 1] == b'\r' {
            end -= 1;
        }
        out.push((start, end));
    }
    out
}

/// Does this slice of a comment carry prose?
///
/// A comment line counts only if it holds a character that is not comment
/// furniture. `/**`, `*/`, `**/`, a bare `*` continuation, an empty `//` and a
/// row of asterisks are all decoration: counting them would let formatting
/// style inflate the score by ~40% and make Javadoc look worse than it is.
pub fn has_prose(slice: &[u8]) -> bool {
    slice
        .iter()
        .any(|b| !matches!(b, b'/' | b'*') && !b.is_ascii_whitespace())
}

/// A comment node: byte range plus the row range it spans.
type CommentSpan = (usize, usize, usize, usize);

fn collect_comments(node: Node, kinds: &[&str], out: &mut Vec<CommentSpan>) {
    if kinds.contains(&node.kind()) {
        out.push((
            node.start_byte(),
            node.end_byte(),
            node.start_position().row,
            node.end_position().row,
        ));
        // Nested block comments are a single node in these grammars; no need to
        // descend, and descending would double-count.
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_comments(child, kinds, out);
    }
}

/// Classify every line of `content`. Returns `None` only when the parser could
/// not produce a tree at all. A tree containing errors is expected and usable:
/// a pre-commit hook parses whatever half-finished state it catches, and
/// tree-sitter recovers well enough to still find the comments.
pub fn classify(content: &[u8], lang: Language) -> Option<Vec<LineFlags>> {
    let lines = line_ranges(content);
    let mut flags = vec![LineFlags::default(); lines.len()];
    if lines.is_empty() {
        return Some(flags);
    }

    let mut parser = Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    let tree = parser.parse(content, None)?;

    let mut comments = Vec::new();
    collect_comments(tree.root_node(), lang.comment_kinds(), &mut comments);

    // Byte mask of comment territory, so a line's code content is whatever
    // non-whitespace sits outside it.
    let mut in_comment = vec![false; content.len()];
    for &(s, e, _, _) in &comments {
        for b in &mut in_comment[s..e.min(content.len())] {
            *b = true;
        }
    }

    // A comment node knows which rows it spans; intersect it with each row and
    // test that row's slice for prose.
    for &(s, e, first_row, last_row) in &comments {
        for idx in first_row..=last_row {
            let Some(&(ls, le)) = lines.get(idx) else { break };
            let from = s.max(ls);
            let to = e.min(le);
            if from < to && has_prose(&content[from..to]) {
                flags[idx].comment = true;
            }
        }
    }

    for (idx, &(ls, le)) in lines.iter().enumerate() {
        for i in ls..le {
            if !in_comment[i] && !content[i].is_ascii_whitespace() {
                flags[idx].code = true;
                break;
            }
        }
    }

    Some(flags)
}
