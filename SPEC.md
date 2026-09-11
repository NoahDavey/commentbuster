# commentbuster — Specification

**Status:** v1 implemented and tested. 38 tests passing. See §10 for what
implementation changed.
**Date:** 2026-09-08

---

## 1. Intent

| | |
|---|---|
| **Outcome** | A single fast binary, `commentbuster`, that reports the net comment lines added in a git diff for Java, Kotlin and TypeScript, and can fail on a threshold. |
| **User** | The author first, then CI and pre-commit hooks — and indirectly the coding agents, whose comment habits the gate is meant to correct. |
| **Why now** | Models over-comment by default, including on self-explanatory new public methods. "No comments whatsoever" is too blunt a correction. A number is needed before a fair budget can be set. |
| **Success** | Run at a repo root, given a ref or nothing, returns a trustworthy count in well under a second; later drops into CI with `--max-ratio` and fails PRs for the right reason. |
| **Constraint** | Speed. Pre-commit hooks that feel slow get disabled. Single static binary, no runtime dependency. |

### Out of scope for v1

- Judging comment *quality* or usefulness.
- Treating Javadoc/KDoc/TSDoc as more legitimate than `//`. All comment lines weigh the same.
- License- or copyright-header exemptions.
- Ignore files, config files, or per-path rules.
- Languages beyond Java, Kotlin and TypeScript.
- Detecting a method moved between files (file renames *are* handled; cross-file moves are not).

These are deliberate omissions, not oversights. A mechanism for exemptions and a comment-classification model are day-two work (§9).

---

## 2. Metrics

### Primary: net comment lines

```
net = comment_lines_added - comment_lines_removed
```

Always reported as its components, never collapsed: `+18 / -25 = -7`. A negative net is a good outcome, and the tool is designed so that deleting stale comments earns budget. The breakdown is what makes a CI failure actionable.

### Secondary: comment ratio

```
ratio = comment_lines_added / code_lines_added
```

Reported alongside net. `null` when `code_lines_added` is 0. **Unbounded above**: a
diff that adds mostly comment-only lines has more comment lines than code lines,
so 400% is a real and common reading. `--max-ratio` therefore accepts values
above 1. A PR adding 800 lines with 40 comment lines is healthy; one adding 30 lines with 25 comment lines is the pathology being chased — which is why ratio exists and why raw count alone is insufficient.

---

## 3. Counting semantics

### What is a comment line

A line counts as a comment line when it intersects a comment node in the parse tree **and** carries prose — that is, the comment's slice of that line contains at least one character that is neither whitespace nor comment furniture (`/`, `*`).

| Case | Counts | Rationale |
|---|---|---|
| `// increment the counter` | Yes | The target case. |
| `val x = 5 // because foo` | Yes, 1 comment line — **and also 1 code line** | A comment the model chose to add. The line genuinely contains both. |
| Body lines of a block comment carrying text | Yes, one per line | A 5-line Javadoc with 3 prose lines counts 3. |
| Bare `*` between Javadoc paragraphs | No | Carries no prose. Counting it lets formatting style inflate the score ~40% and makes Javadoc look worse than it is. |
| A row of asterisks or slashes (`// ***`, `/**/`) | No | Decoration, by the same rule. |
| `/**` and `*/` delimiter lines (nothing else on them) | No | Same reason. |
| Empty `//` | No | Same reason. |
| Commented-out code | Yes, as prose | Models leave dead code behind constantly and it should be flagged. Telling prose from dead code reliably is a day-two problem. |
| Comment inside a string literal | No | The AST settles this, which is a core reason for parsing rather than regex. |

### What is a code line

A changed line counts as a code line when it carries any non-comment, non-whitespace content. A comment-only line is not a code line. A trailing-comment line is both, so `ratio` cannot exceed 1.

### Rename and move handling

git rename and copy detection is enabled (`-M -C`), so a pure file rename nets to zero. A method moved between files will show as a false positive in v1 — accepted, documented.

Rename detection is git's, so it holds only above git's similarity threshold
(50% by default). A file that is moved *and* substantially rewritten reads as an
add plus a delete. The totals stay correct — the same comment lines are counted
once on each side — but the per-file rows show two entries rather than one. Pinned
by a test, because it is the shape most likely to look like a bug.

---

## 4. CLI surface

```
commentbuster [<ref> [<ref>]] [options]
commentbuster claude-gate [options]
```

### Ref resolution

| Invocation | Compares |
|---|---|
| `commentbuster` | Working tree vs `HEAD` (includes unstaged changes; **untracked files are invisible**, matching `git diff HEAD` — stage them or use `--staged`) |
| `commentbuster --staged` | Index vs `HEAD` — the pre-commit case |
| `commentbuster <ref>` | `<ref>...HEAD` |
| `commentbuster <a> <b>` | `<a>...<b>` |

**Merge-base is the default** for two-ref comparisons (`...`, three-dot). "How many comments did this PR add" means *since it branched*, not *since main moved on*. Comparing directly against a moved base is how a gate blames an author for someone else's comments.

`--direct` switches to two-dot semantics for the cases where the literal comparison is what's wanted.

Must be run inside a git work tree; discovers the repo root itself, so it works from any subdirectory.

### Options

| Flag | Default | Effect |
|---|---|---|
| `--staged` | off | Compare index against `HEAD`. Mutually exclusive with ref arguments. |
| `--direct` | off | Two-dot ref comparison instead of merge-base. |
| `--pretty` | off | Human-readable summary and per-file table instead of JSON. |
| `--max-net N` | unset | Exit 1 if net comment lines added exceeds `N`. |
| `--max-ratio P` | unset | Exit 1 if ratio exceeds `P` (a fraction, e.g. `0.05`). |
| `--max-net-small N` | unset | Exit 1 if net comment lines added exceeds `N`, on diffs below `--min-code-lines`. The complement of `--max-ratio`: the two never apply to the same diff. |
| `--min-code-lines N` | `20` | The ratio gate does not apply below this many added code lines, and `--max-net-small` applies only there. Prevents a 2-line diff with 1 comment scoring 50%. |
| `--top N` | `20` | Show only the N worst files in `--pretty`; `0` shows all. JSON is never truncated. |
| `--` | | Path filter separator; paths after it limit the diff, mirroring `git diff -- <paths>`. |

With no gate flags the exit code is always 0 — the tool reports and says nothing about pass or fail.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success; no gate breached, or no gate configured |
| 1 | A configured gate was breached |
| 2 | Usage error, not a git repo, bad ref, or internal failure |

### `claude-gate` subcommand

The Claude Code `PreToolUse` hook, so the hook needs nothing but the binary.
Claude Code writes a JSON description of the pending tool call to stdin; the
subcommand:

1. Extracts `tool_input.command` and allows, printing nothing, unless `git` starts
   a shell segment and `commit` follows it as a whole word in that segment.
   `git add -A && git commit -m x` is a commit; `echo "git commit"` is not.
2. Analyses index vs `HEAD` with `--max-ratio` (default `0.15`) and
   `--max-net-small` (default `5`) around `--min-code-lines` (default `20`). Each
   flag falls back to `COMMENTBUSTER_MAX_RATIO`, `COMMENTBUSTER_MAX_NET_SMALL`,
   `COMMENTBUSTER_MIN_CODE_LINES`.
3. On a breach prints `{"hookSpecificOutput": {"hookEventName": "PreToolUse",
   "permissionDecision": "deny", "permissionDecisionReason": …}}`, naming the
   rule, the worst files and what to cut.

The exit code is 0 on every path except a usage error. An explicit "allow" is
never printed: it would skip Claude Code's permission prompt. Failures of the gate
itself (not a git repo, unreadable payload) allow with a note on stderr, since
Claude Code blocks only on exit 2 and a gate that blocks because of its own
trouble gets disabled. `COMMENTBUSTER_SKIP` set to anything bypasses it.

---

## 5. Output

### JSON (default)

Machine-first, because CI is the main consumer. Written to stdout; diagnostics to stderr.

```json
{
  "version": 1,
  "range": {
    "mode": "merge-base",
    "base_ref": "main",
    "head_ref": "HEAD",
    "base_sha": "a1b2c3d4e5f6...",
    "head_sha": "f6e5d4c3b2a1..."
  },
  "totals": {
    "comment_lines_added": 18,
    "comment_lines_removed": 25,
    "comment_lines_net": -7,
    "code_lines_added": 240,
    "code_lines_removed": 30,
    "comment_ratio": 0.075
  },
  "files": [
    {
      "path": "src/main/kotlin/Foo.kt",
      "previous_path": null,
      "language": "kotlin",
      "status": "modified",
      "comment_lines_added": 12,
      "comment_lines_removed": 2,
      "comment_lines_net": 10,
      "code_lines_added": 40,
      "code_lines_removed": 6
    }
  ],
  "skipped": {
    "count": 12,
    "unsupported": 11,
    "parse_error": 1,
    "binary": 0,
    "too_large": 0
  },
  "gate": {
    "configured": true,
    "passed": false,
    "failures": [
      { "rule": "max_ratio", "limit": 0.05, "actual": 0.075 }
    ],
    "skipped_rules": [
      { "rule": "max_ratio", "reason": "12 added code lines is below --min-code-lines 20" }
    ]
  },
  "duration_ms": 34
}
```

`files` is sorted by `comment_lines_net` descending — worst first. Files with no comment-line change are still listed if they had code changes, so the ratio denominator is auditable.

Skipped files are always reported. The count must never be silently wrong.

### `--pretty`

Human summary followed by the worst-first per-file table. Per-file detail is on by default in this mode: it's noise when merely curious and essential when CI has failed, and the failing case wins.

```
commentbuster  main...HEAD (merge-base a1b2c3d)

  comment lines   +18  -25   net -7
  code lines      +240 -30   ratio 7.5%

  path                              net    +    -
  src/main/kotlin/Foo.kt            +10   12    2
  src/main/java/Bar.java             +3    3    0
  web/src/baz.ts                    -20    1   21

  12 files skipped (11 unsupported, 1 parse error)

  FAIL  ratio 7.5% exceeds --max-ratio 5.0%
```

---

## 6. File scope

**In scope:** `.java`, `.kt`, `.kts`, `.ts`, `.tsx`, `.mts`, `.cts`

Everything else is skipped and counted in the `skipped` block. Binary files, files over a size ceiling, and files that fail to parse are skipped and reported by reason. A parse failure never aborts the run.

---

## 7. Implementation

**Rust + tree-sitter, single static binary.**

Speed is the constraint and pre-commit is the demanding case: a hook adding ~400ms of Node startup before doing any work is a hook people disable. Rust with tree-sitter grammars compiled in gives one binary (~10MB), no runtime, no `node_modules`, and parsing a few hundred changed files in parallel in tens of milliseconds. tree-sitter is also error-tolerant, which matters because a pre-commit hook parses whatever half-finished state it catches.

Rejected: TypeScript + tree-sitter WASM — trivial distribution, identical accuracy, but startup cost dominates and is paid on every hook invocation. Go with tree-sitter bindings is the middle option if maintainability by a non-Rust team ever outweighs the speed.

### Algorithm

The core idea: **the AST says which lines are comments; the diff says which lines changed; the answer is the intersection.**

1. Resolve the range. `git merge-base` for three-dot mode; resolve both refs to SHAs.
2. Enumerate changed files with `git diff --raw -z -M -C <base> <head>`, capturing status and rename source.
3. Partition into in-scope and skipped by extension and mode.
4. Fetch every needed blob in one `git cat-file --batch` (stdin written from a
   thread, so a large request list cannot deadlock against a large response).
5. For each in-scope file, in parallel (rayon):
   a. Take old and new contents — blobs from the batch, or the working file from
      disk in worktree mode.
   b. Parse both with the language grammar; walk comment nodes to build a
      per-line comment/code verdict, per §3.
   c. Diff the two line sequences **in process** with `similar`'s histogram
      algorithm, using the same line splitter the classifier uses.
   d. Intersect: added lines against the new-content verdicts, removed lines
      against the old-content verdicts.
6. Sum, compute the ratio, evaluate gates, emit.

Line diffing happens in process rather than by parsing `git diff -U0`. That is
one fewer git invocation, it parallelises, and — the reason that matters — an
index into the diff addresses exactly the same line as an index into the
classifier's verdicts, by construction. Parsing git's patch output would have
required keeping two line-splitting rules in agreement.

Grammar comment nodes, confirmed empirically against the installed grammars:
`line_comment` and `block_comment` (Java), `line_comment` and `block_comment`
(Kotlin — the `-ng` grammar does not use `multiline_comment`), `comment`
(TypeScript and TSX). Node kinds are pinned in `lang.rs` rather than matched on
`kind().contains("comment")`, and a test asserts each one still exists in its
grammar, so an upgrade that renames a node fails the suite instead of silently
reporting zero.

### Distribution

- GitHub release artifacts: `darwin-arm64`, `darwin-x64`, `linux-x64-musl`, `linux-arm64-musl`.
- A `curl | sh` install script.
- An npm wrapper package so `npx commentbuster` works in a JS pre-commit hook with no separate install step.

---

## 8. Testing

- **Fixture corpus per language** covering every row of the §3 table: trailing comments, bare `*` lines, delimiter-only lines, commented-out code, comment markers inside string literals, nested block comments (Kotlin), Javadoc, KDoc, TSDoc, JSX comments.
- **Diff-shape fixtures:** pure addition, pure deletion, rename with no content change (must net to zero), rename with edits, new file, deleted file, mode change, comment-only diff (`code_lines_added` 0 → `ratio` null).
- **Range semantics:** merge-base vs `--direct` on a branch whose base has advanced — asserting the moved base contributes nothing.
- **Gate matrix:** each threshold, each exit code, `--min-code-lines` suppression, `--max-ratio` and `--max-net-small` never both applying to one diff.
- **Hook protocol:** commit detection against mentions and chained commands, deny JSON shape, silent allow, flag over environment precedence, skip variable, out-of-repo and garbage-payload tolerance.
- **Golden JSON** snapshots for the output schema.
- **Timing regression test** on a synthetic several-hundred-file diff.

Measured on this machine (M-series, release build): a 400-file diff carrying
16,800 added comment lines is analysed in **170–200 ms**; the binary is **7.7 MB**
and links only `libSystem`.

---

## 9. Day-two work

Recorded so v1 stays small, not as commitments:

- **Comment classification** — weigh a doc comment on a new public API differently from `// increment the counter`. Wanted eventually; deliberately absent from v1 because over-commenting is observed even on self-explanatory new public methods, so uniform weighting is the honest starting heuristic.
- **Exemption mechanism** — license and copyright headers, generated files, vendored paths, test directories, and an ignore-config format. Today a repo with a mandated 15-line header starts every new file that far in the hole.
- **More languages** — Python, Go, Swift, C#.
- **Cross-file move detection.**
- **Baseline and trend** — store a per-repo baseline so a gate can be set from observed history rather than a guessed constant.


---

## 10. What implementation changed

Recorded so the spec matches the code.

| Change | Why |
|---|---|
| Prose test is "contains a character that is not `/`, `*` or whitespace" | The delimiter-stripping rule the spec described mis-handled `/**/`: stripping the leading `/*` then greedily stripping `*` ate the closing delimiter's asterisk and left a stray `/`, which read as prose. The content test is simpler and correct for every furniture case. |
| Kotlin's block comment node is `block_comment` | The spec said `multiline_comment`. Confirmed against the installed grammar. |
| Line diffing in process, not by parsing `git diff -U0` | One fewer git invocation, parallelisable, and the diff indices align with the classifier's line splitting by construction. |
| `--max-ratio` accepts values above 1 | Found while writing the tests: a diff adding 6 comment lines against 1 code line has a ratio of 6.0. Capping the flag at 1 would have made the pathological cases unexpressible. |
| `--top N` added, default 20 | Found by running the tool on a 400-file diff: `--pretty` printed 400 rows. JSON is never truncated. |
| `gate.skipped_rules` added to the JSON | So a green result never quietly means "the gate never ran". |
| `--max-net-small N` added | The ratio floor leaves small diffs ungated, and a flat `--max-net` tight enough for them caps large, proportionately-commented diffs. A net ceiling that applies only below the floor is the complement the ratio needs. |
| `claude-gate` subcommand replaces the shell hook | The shell hook reimplemented the thresholds in Python and needed `python3`, against the single-binary constraint in §1. The binary now speaks the PreToolUse protocol itself: one gate implementation, tested. |

### One interaction worth knowing before setting a CI budget

`--min-code-lines` exists to stop the ratio gate flagging tiny diffs — but the
pathology being hunted *is* often a small diff with many comments. A PR adding 15
code lines and 78 comment lines is suppressed by the default floor of 20.

So the ratio gate alone is not enough. **Set both gates in CI**: `--max-ratio` to
catch comment-heavy large PRs, and `--max-net` to catch the small ones the floor
lets through, or `--max-net-small` to do the same without capping large PRs.
