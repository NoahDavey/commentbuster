//! SPEC.md §8: diff-shape fixtures. Each asserts exact counts.

mod common;
use common::{totals, Repo};

const PLAIN: &str = "package demo;

public class A {
  int add(int a, int b) {
    return a + b;
  }
}
";

/// PLAIN with a single comment inserted: similar enough that git still calls a
/// move of it a rename.
const PLAIN_PLUS_ONE: &str = "package demo;

// a note
public class A {
  int add(int a, int b) {
    return a + b;
  }
}
";

const COMMENTED: &str = "package demo;

/**
 * Adds two numbers together.
 *
 * @param a the first number
 * @param b the second number
 * @return the sum
 */
public class A {
  // Add the two numbers
  int add(int a, int b) {
    return a + b; // return the sum
  }
}
";

#[test]
fn pure_addition_of_comments() {
    let r = Repo::new("pure_addition");
    r.write("src/A.java", PLAIN);
    let base = r.commit("base");
    r.write("src/A.java", COMMENTED);
    r.commit("comment it up");

    let (code, v) = r.run(&[&base]);
    assert_eq!(code, 0);
    // 6 comment lines added: 4 prose lines of the Javadoc (`/**`, a bare `*`
    // and `*/` are furniture), the `// Add the two numbers` line, and the
    // trailing comment. 1 code line added: the trailing-comment line, which
    // counts as both. 1 code line removed: the untouched-looking return.
    assert_eq!(totals(&v), (6, 0, 6, 1, 1));
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(v["files"][0]["language"], "java");
    assert_eq!(v["files"][0]["status"], "modified");
    assert_eq!(v["files"][0]["comment_lines_net"], 6);
}

#[test]
fn removing_comments_earns_budget() {
    let r = Repo::new("pure_deletion");
    r.write("src/A.java", COMMENTED);
    let base = r.commit("base");
    r.write("src/A.java", PLAIN);
    r.commit("strip the noise");

    let (code, v) = r.run(&[&base]);
    assert_eq!(code, 0);
    let (added, removed, net, _, _) = totals(&v);
    assert_eq!((added, removed, net), (0, 6, -6));
}

#[test]
fn rename_with_no_content_change_nets_zero() {
    let r = Repo::new("rename_clean");
    r.write("src/A.java", COMMENTED);
    let base = r.commit("base");
    r.git(&["mv", "src/A.java", "src/B.java"]);
    r.commit("move it");

    let (code, v) = r.run(&[&base]);
    assert_eq!(code, 0);
    assert_eq!(totals(&v), (0, 0, 0, 0, 0));
    assert_eq!(v["files"][0]["status"], "renamed");
    assert_eq!(v["files"][0]["path"], "src/B.java");
    assert_eq!(v["files"][0]["previous_path"], "src/A.java");
}

#[test]
fn rename_with_edits_counts_only_the_edits() {
    let r = Repo::new("rename_edited");
    r.write("src/A.java", PLAIN);
    let base = r.commit("base");
    r.git(&["mv", "src/A.java", "src/B.java"]);
    r.write("src/B.java", PLAIN_PLUS_ONE);
    r.commit("move and add one comment");

    let (_, v) = r.run(&[&base]);
    assert_eq!(totals(&v), (1, 0, 1, 0, 0));
    assert_eq!(v["files"][0]["status"], "renamed");
    assert_eq!(v["files"][0]["previous_path"], "src/A.java");
}

#[test]
fn rewrite_below_gits_rename_threshold_becomes_add_plus_delete() {
    // Moving A.java to B.java *and* tripling its size is only ~47% similar, so
    // git declines to call it a rename. The totals are still right: the same 6
    // comment lines are added, and every code line is counted once on each
    // side. Pinned because it is the shape most likely to look like a bug.
    let r = Repo::new("rename_rewritten");
    r.write("src/A.java", PLAIN);
    let base = r.commit("base");
    r.git(&["mv", "src/A.java", "src/B.java"]);
    r.write("src/B.java", COMMENTED);
    r.commit("move and comment heavily");

    let (_, v) = r.run(&[&base]);
    assert_eq!(totals(&v), (6, 0, 6, 6, 6));
    let statuses: Vec<&str> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, vec!["added", "deleted"]);
}

#[test]
fn new_file_counts_every_comment_line() {
    let r = Repo::new("new_file");
    r.write("README.md", "seed\n");
    let base = r.commit("base");
    r.write("src/A.java", COMMENTED);
    r.commit("add file");

    let (_, v) = r.run(&[&base]);
    // Every line of the new file is added: 6 comment lines, and 6 code lines
    // (`package`, the class, the signature, the return, and two closing
    // braces) — the trailing-comment line counting as both.
    let (added, removed, net, code_added, code_removed) = totals(&v);
    assert_eq!((added, removed, net, code_removed), (6, 0, 6, 0));
    assert_eq!(code_added, 6);
    assert_eq!(v["files"][0]["status"], "added");
}

#[test]
fn deleted_file_counts_as_removal() {
    let r = Repo::new("deleted_file");
    r.write("src/A.java", COMMENTED);
    r.write("README.md", "seed\n");
    let base = r.commit("base");
    std::fs::remove_file(r.dir.join("src/A.java")).unwrap();
    r.commit("delete it");

    let (_, v) = r.run(&[&base]);
    let (added, removed, net, _, _) = totals(&v);
    assert_eq!((added, removed, net), (0, 6, -6));
    assert_eq!(v["files"][0]["status"], "deleted");
}

#[test]
fn comment_only_diff_has_no_ratio() {
    let r = Repo::new("comment_only");
    r.write("src/A.java", PLAIN);
    let base = r.commit("base");
    r.write(
        "src/A.java",
        "package demo;\n\n// a new comment\npublic class A {\n  int add(int a, int b) {\n    return a + b;\n  }\n}\n",
    );
    r.commit("one comment");

    let (_, v) = r.run(&[&base]);
    let (added, _, _, code_added, _) = totals(&v);
    assert_eq!((added, code_added), (1, 0));
    assert!(v["totals"]["comment_ratio"].is_null(), "ratio must be null, not 0 or inf");
}

#[test]
fn unsupported_binary_and_symlink_files_are_skipped_and_reported() {
    let r = Repo::new("skipped");
    r.write("README.md", "seed\n");
    let base = r.commit("base");
    r.write("docs/notes.md", "# notes\n<!-- an html comment -->\n");
    r.write("web/app.js", "// a js comment\nconst x = 1;\n");
    r.write("config.yaml", "# a yaml comment\nkey: value\n");
    r.write_bytes("web/blob.ts", b"const x = 1;\n\0\0binary\n");
    std::os::unix::fs::symlink("README.md", r.dir.join("link.ts")).unwrap();
    r.write("src/A.java", COMMENTED);
    r.commit("mixed bag");

    let (_, v) = r.run(&[&base]);
    assert_eq!(v["skipped"]["unsupported"], 4, "md, js, yaml, symlink");
    assert_eq!(v["skipped"]["binary"], 1);
    assert_eq!(v["skipped"]["count"], 5);
    // Only the Java file was counted.
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(totals(&v).0, 6);
}

#[test]
fn kotlin_and_typescript_in_one_diff() {
    let r = Repo::new("multi_language");
    r.write("README.md", "seed\n");
    let base = r.commit("base");
    r.write(
        "src/Main.kt",
        "// top comment\nfun main() {\n    val x = 1 // trailing\n}\n",
    );
    r.write(
        "web/util.ts",
        "/**\n * Doc.\n */\nexport const x = 1;\n",
    );
    r.write("build.gradle.kts", "// build comment\nplugins { }\n");
    r.write("web/App.tsx", "const A = () => <div>{/* note */}</div>;\n");
    r.commit("polyglot");

    let (_, v) = r.run(&[&base]);
    let langs: Vec<&str> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["language"].as_str().unwrap())
        .collect();
    assert!(langs.contains(&"kotlin"));
    assert!(langs.contains(&"typescript"));
    assert!(langs.contains(&"tsx"));
    // Main.kt 2 + util.ts 1 + build.gradle.kts 1 + App.tsx 1
    assert_eq!(totals(&v).0, 5);
}
