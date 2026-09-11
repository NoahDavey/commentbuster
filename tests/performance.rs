//! SPEC.md §8: a timing regression test on a synthetic several-hundred-file
//! diff. Speed is the binding constraint — a pre-commit hook that feels slow
//! gets disabled — so this guards the order of magnitude, not a precise number.

mod common;
use common::Repo;

const FILES: usize = 400;

fn java_file(i: usize, commented: bool) -> String {
    let mut s = format!("package demo.p{i};\n\n");
    if commented {
        s.push_str("/**\n * Generated class number ");
        s.push_str(&i.to_string());
        s.push_str(".\n *\n * @see Nothing\n */\n");
    }
    s.push_str(&format!("public class C{i} {{\n"));
    for m in 0..20 {
        if commented {
            s.push_str(&format!("  // method {m} does a thing\n"));
        }
        s.push_str(&format!(
            "  int m{m}(int a) {{\n    return a + {m}; {}\n  }}\n",
            if commented { "// add" } else { "" }
        ));
    }
    s.push_str("}\n");
    s
}

#[test]
fn several_hundred_file_diff_stays_fast() {
    let r = Repo::new("perf");
    for i in 0..FILES {
        r.write(&format!("src/C{i}.java"), &java_file(i, false));
    }
    let base = r.commit("base");
    for i in 0..FILES {
        r.write(&format!("src/C{i}.java"), &java_file(i, true));
    }
    r.commit("comment everything");

    let wall = std::time::Instant::now();
    let (code, v) = r.run(&[&base]);
    let wall_ms = wall.elapsed().as_millis();
    assert_eq!(code, 0);

    let files = v["files"].as_array().unwrap().len();
    let reported = v["duration_ms"].as_u64().unwrap();
    let added = v["totals"]["comment_lines_added"].as_u64().unwrap();
    println!(
        "{files} files, {added} comment lines added, {reported} ms internal, {wall_ms} ms wall"
    );

    assert_eq!(files, FILES);
    // Per file: 2 Javadoc prose lines (`/**`, the bare `*` and `*/` are
    // furniture) + 20 method comments + 20 trailing comments.
    assert_eq!(added, (FILES * 42) as u64);
    assert!(
        reported < 4000,
        "{reported} ms for {FILES} files is a regression"
    );
}
