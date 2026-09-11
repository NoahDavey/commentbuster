//! Pins the per-line verdict for every row of the counting-semantics table in
//! SPEC.md §3, in each supported language.
//!
//! Legend: `.` neither, `C` comment only, `K` code only, `B` both.

use commentbuster::classify::{classify, has_prose, LineFlags};
use commentbuster::lang::Language;

fn render(content: &[u8], lang: Language) -> String {
    let flags = classify(content, lang).expect("parse");
    flags
        .iter()
        .map(|f| match f {
            LineFlags { comment: true, code: true } => 'B',
            LineFlags { comment: true, code: false } => 'C',
            LineFlags { comment: false, code: true } => 'K',
            _ => '.',
        })
        .collect()
}

#[test]
fn java_lines() {
    // 1 `/*` and 3 `*/` are furniture; 7 ` *` is a bare continuation; 12 is a
    // trailing comment (both); 13 hides `//` in a string (code only); 14 is
    // commented-out code (a comment); 15 is an inline block plus code.
    assert_eq!(
        render(include_bytes!("fixtures/sample.java"), Language::Java),
        ".C.K.C.C.KCBKCBKK"
    );
}

#[test]
fn kotlin_lines() {
    // 10-11 are a two-line block comment with prose on both lines; 13 is a
    // nested block comment, which the grammar returns as one node.
    assert_eq!(
        render(include_bytes!("fixtures/sample.kt"), Language::Kotlin),
        ".C..K.CBKCCKCKK"
    );
}

#[test]
fn typescript_lines() {
    // 10 hides `/* */` in a template literal.
    assert_eq!(
        render(include_bytes!("fixtures/sample.ts"), Language::TypeScript),
        ".C.C.KCBKKCK"
    );
}

#[test]
fn tsx_lines() {
    // 3 is `{/* jsx comment */}` — the braces are JSX syntax, so the line is
    // both a comment line and a code line, exactly like a trailing comment.
    assert_eq!(
        render(include_bytes!("fixtures/sample.tsx"), Language::Tsx),
        "KKBKKK"
    );
}

#[test]
fn prose_test_rejects_comment_furniture() {
    for furniture in [
        "/**", "/*", "*/", "*", "//", "///", "**/", "/* */", "/**/", "  *  ", "// ***",
        "/* ***** */", "*  *  *",
    ] {
        assert!(!has_prose(furniture.as_bytes()), "{furniture:?} should not count");
    }
    for prose in ["// x", "/** Doc */", "* @param a", "// int dead = 1;", "/* a", "b */"] {
        assert!(has_prose(prose.as_bytes()), "{prose:?} should count");
    }
}

#[test]
fn comment_node_kinds_exist_in_every_grammar() {
    // A grammar upgrade that renames a comment node must fail here rather than
    // silently start reporting zero comments.
    for lang in [Language::Java, Language::Kotlin, Language::TypeScript, Language::Tsx] {
        let ts = lang.ts_language();
        for kind in lang.comment_kinds() {
            assert_ne!(
                ts.id_for_node_kind(kind, true),
                0,
                "grammar for {} no longer has node kind `{kind}`",
                lang.name()
            );
        }
    }
}

#[test]
fn extension_scope() {
    for (path, expected) in [
        ("src/A.java", Some("java")),
        ("src/A.kt", Some("kotlin")),
        ("build.gradle.kts", Some("kotlin")),
        ("web/a.ts", Some("typescript")),
        ("web/a.mts", Some("typescript")),
        ("web/a.cts", Some("typescript")),
        ("web/a.d.ts", Some("typescript")),
        ("web/a.tsx", Some("tsx")),
        ("web/a.js", None),
        ("README.md", None),
        ("no-extension", None),
        ("dir.ts/file", None),
    ] {
        assert_eq!(
            Language::from_path(path).map(|l| l.name()),
            expected,
            "for {path}"
        );
    }
}

#[test]
fn unparseable_half_written_file_still_counts_comments() {
    // A pre-commit hook parses whatever state it catches. tree-sitter recovers,
    // so this must not be reported as a parse error.
    let src = b"public class A {\n  // a comment\n  int x = ;\n";
    let flags = classify(src, Language::Java).expect("tree-sitter recovers");
    assert!(flags[1].comment);
}

#[test]
fn crlf_and_missing_trailing_newline() {
    let src = b"// one\r\nint x = 1;";
    let flags = classify(src, Language::Java).expect("parse");
    assert_eq!(flags.len(), 2);
    assert!(flags[0].comment && !flags[0].code);
    assert!(flags[1].code && !flags[1].comment);
}
