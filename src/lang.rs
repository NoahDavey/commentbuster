//! Language detection and the comment node kinds for each grammar.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Java,
    Kotlin,
    TypeScript,
    Tsx,
}

impl Language {
    /// Detect from a repo-relative path. `None` means the file is out of scope.
    pub fn from_path(path: &str) -> Option<Language> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e)?;
        match ext {
            "java" => Some(Language::Java),
            "kt" | "kts" => Some(Language::Kotlin),
            "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Java => "java",
            Language::Kotlin => "kotlin",
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
        }
    }

    pub fn ts_language(self) -> tree_sitter::Language {
        match self {
            Language::Java => tree_sitter_java::LANGUAGE.into(),
            Language::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    /// Node kinds that are comments in this grammar. Pinned deliberately rather
    /// than matched on `kind().contains("comment")`: a grammar upgrade that
    /// renames a node should fail the test suite, not silently return zero.
    pub fn comment_kinds(self) -> &'static [&'static str] {
        match self {
            Language::Java | Language::Kotlin => &["line_comment", "block_comment"],
            Language::TypeScript | Language::Tsx => &["comment"],
        }
    }
}
