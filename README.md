# commentbuster

Counts the net comment lines added in a git diff, for Java, Kotlin and TypeScript.

Models over-comment by default, including on self-explanatory new public methods.
"No comments whatsoever" is too blunt a correction. This gives you the number, so
you can set a budget instead.

Comments are found with tree-sitter, not regex: a `//` inside a string literal is
not a comment, and a `/**` line carrying no prose is not a comment line.

```
$ commentbuster main --pretty --max-ratio 0.15

commentbuster  main...HEAD (merge-base 6abe7c9dc)

  comment lines     +78     -0   net +78
  code lines        +34     -6   ratio 229.4%

  path                                                 net     +     -  lang
  server/src/main/java/com/acme/OrderService.java      +28    28     0  java
  server/src/main/java/com/acme/DiscountPolicy.java    +17    17     0  java
  web/src/orders.ts                                    +15    15     0  typescript
  server/src/main/kotlin/com/acme/PricingRules.kt      +14    14     0  kotlin
  web/src/OrderList.tsx                                 +4     4     0  tsx

  2 files skipped (2 unsupported)

  FAIL  ratio 229.4% exceeds --max-ratio 15.0%

  80 ms
```

## Install

Prebuilt binaries for macOS (Apple Silicon, Intel) and Linux (x86_64, arm64) are
attached to each [release](https://github.com/NoahDavey/commentbuster/releases) and
published to npm.

With the install script, which picks the right binary, verifies its checksum and
puts `commentbuster` in `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/NoahDavey/commentbuster/main/install.sh | sh
```

`COMMENTBUSTER_INSTALL_DIR` changes the directory; `COMMENTBUSTER_VERSION=v0.3.0`
pins a release.

From npm, with no separate install step:

```sh
bunx @havella/commentbuster@0.3.0 --staged --pretty
npx @havella/commentbuster@0.3.0 --staged --pretty
npm install --save-dev --save-exact @havella/commentbuster@0.3.0
```

`@havella/commentbuster` is a small Node launcher. The binary comes from an
optional dependency for your platform, so nothing runs at install time and
`--ignore-scripts` is fine; `--omit=optional` is not. The Linux binaries need
glibc 2.39 or later. The launcher adds a Node start to every run (about 25 ms), so
the installed binary is the faster choice for a hook.

With a Rust toolchain, `cargo install --git
https://github.com/NoahDavey/commentbuster --locked` builds it instead.

The binary itself has no runtime dependencies; the Claude Code hook is a subcommand
of it.

## Usage

| Invocation | Compares |
|---|---|
| `commentbuster` | Working tree vs `HEAD` |
| `commentbuster --staged` | Index vs `HEAD` — the pre-commit case |
| `commentbuster <ref>` | `<ref>...HEAD`, from the merge base |
| `commentbuster <a> <b>` | `<a>...<b>`, from the merge base |
| `commentbuster --direct <a> <b>` | `<a>..<b>`, compared literally |

Two refs are compared **from their merge base** by default. "How many comments did
this PR add" means *since it branched*, not *since main moved on*; comparing
against a moved base is how a gate ends up blaming you for someone else's
comments.

JSON goes to stdout by default. `--pretty` gives the human view above.

### Metrics

- **net** = comment lines added − comment lines removed, always shown as its
  components (`+78 / -0`). Deleting stale comments earns budget.
- **ratio** = comment lines added ÷ code lines added. Unbounded above: a diff that
  is mostly comments reads as 400%. `null` when a diff adds no code at all.

A trailing comment (`val x = 5 // why`) counts as one comment line *and* one code
line. Comment-only lines that carry no prose — `/**`, `*/`, a bare `*`, an empty
`//` — count as neither.

### Gating

```sh
commentbuster --max-net 10 --max-ratio 0.15     # exit 1 if either is breached
```

| Exit code | Meaning |
|---|---|
| 0 | No gate breached, or no gate configured |
| 1 | A configured gate was breached |
| 2 | Usage error, not a git repo, bad ref, or internal failure |

`--min-code-lines N` (default 20) stops the ratio gate applying to tiny diffs.
Because of it, **set both gates**: `--max-ratio` catches comment-heavy large PRs,
and a net ceiling catches the small ones the floor lets through. `--max-net`
applies to every diff; `--max-net-small` applies only below the floor, so it can
be set tight without capping large, proportionately-commented changes.

### CI

The merge-base comparison needs history, so check out with `fetch-depth: 0`.

```yaml
- uses: actions/checkout@v5
  with:
    fetch-depth: 0
- name: Install commentbuster
  run: |
    curl -fsSL https://raw.githubusercontent.com/NoahDavey/commentbuster/main/install.sh | sh
    echo "$HOME/.local/bin" >> "$GITHUB_PATH"
- name: Comment budget
  run: commentbuster ${{ github.event.pull_request.base.sha }} --pretty --max-ratio 0.15 --max-net 40
```

Set `COMMENTBUSTER_VERSION` on the install step to pin a release.

### Claude Code hook (preferred over a git hook for agents)

`commentbuster claude-gate` is a `PreToolUse` hook. Claude Code pipes it the tool
call it is about to run; the hook ignores everything but `git commit`, checks the
staged budget, and on a breach returns a denial the model can act on. Wire it into
a project's `.claude/settings.json` (team-wide) or `.claude/settings.local.json`
(personal, gitignored). From npm, pinned and with nothing to install:

```json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "Bash",
      "hooks": [{
        "type": "command",
        "command": "bunx @havella/commentbuster@0.3.0 claude-gate",
        "timeout": 20,
        "statusMessage": "Checking comment budget"
      }]
    }]
  }
}
```

Or with the installed binary, which adds the least to each Bash call:

```json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "Bash",
      "hooks": [{
        "type": "command",
        "command": "$HOME/.local/bin/commentbuster claude-gate",
        "timeout": 20,
        "statusMessage": "Checking comment budget"
      }]
    }]
  }
}
```

A teammate who has not installed the binary (or bun) yet gets `command not found`
on stderr and their commit goes through, because Claude Code blocks only on exit
code 2. So the hook can land in a team's settings before everyone has run the
installer. The gate's own failures allow too: an unreadable payload or a
directory that is not a git repo never stops a commit, and the npm launcher exits 1
when it has no binary to run. A misconfigured flag does block, with the usage
error, so it gets noticed instead of silently disabling the gate.

What it does with a command:

- The command is parsed as shell, so a commit is found behind `cd x &&`,
  `FOO=1`, `env`, `bash -c '...'`, `git -C x`, inside `if`, loops, subshells and
  `$(...)`, and a message or heredoc that mentions git commands is not mistaken
  for one.
- It looks at the repository the commit would run in: the session's directory,
  then any literal `cd`, then `git -C`. Linked worktrees use their own index.
- The hook runs before the command, so **staging and committing must be separate
  Bash calls**. `git add -A && git commit`, `git commit -a`/`-p`/`-i`/`-o`,
  `git commit <path>` and two commits in one call are denied with a message that
  says so.
- `--amend` is measured from HEAD's parent, so amending repeatedly does not add
  up to more than one budget. Amending a merge commit is allowed.
- Commits during a merge, rebase, cherry-pick or revert are allowed: their
  comments are mostly someone else's.
- What the text does not settle is allowed: `eval`, aliases, shell functions,
  scripts, `$GIT commit`, `cd "$DIR"`, `--git-dir`/`GIT_DIR`, unparseable shell.

It beats a git `pre-commit` hook for the agent case on three counts: it intercepts
the Bash tool call rather than git's hook chain, so `--no-verify` does not get
around it; it fires only for agents, leaving a human's own commits alone; and its
denial reason goes back to the model as feedback it can act on.

The gate's two rules are **complementary, not overlapping**: at or above
`--min-code-lines` added code lines the ratio decides, below it the absolute net
decides. Applying both to every commit would flag large, proportionately-commented
changes. These are the main command's `--max-ratio` and `--max-net-small` with
hook defaults, so CI can apply the same rule.

| Flag | Variable | Default | Meaning |
|---|---|---|---|
| `--max-ratio P` | `COMMENTBUSTER_MAX_RATIO` | `0.15` | Ratio ceiling for larger commits |
| `--max-net-small N` | `COMMENTBUSTER_MAX_NET_SMALL` | `5` | Net ceiling for small commits |
| `--min-code-lines N` | `COMMENTBUSTER_MIN_CODE_LINES` | `20` | Boundary between the two rules |
| | `COMMENTBUSTER_SKIP` | — | For people: set to anything in Claude Code's own environment to bypass the gate. Setting it in the command has no effect, and denials do not mention it |

Flags win over variables, so a project can pin its budget in the settings entry:
`"command": "$HOME/.local/bin/commentbuster claude-gate --max-ratio 0.1"`.

### git pre-commit

Tool-agnostic (catches any agent, and hand-written code), but evadable with
`--no-verify` and it fires on human commits too. Run it after any `cd` to the
repo root; commentbuster resolves the root itself, so it is worktree-safe.

```sh
#!/bin/sh
commentbuster --staged --pretty --max-net 5 || {
  echo "Too many comment lines added. Cut the ones that restate the code."
  exit 1
}
```

## Scope

`.java` `.kt` `.kts` `.ts` `.tsx` `.mts` `.cts`. Everything else is skipped and
counted in the `skipped` block, so the number is never silently wrong.

Known limits, all deliberate for v1 and covered by tests:

- All comment lines weigh the same. A Javadoc block on a new public API is not
  treated as more legitimate than `// increment the counter`.
- No exemptions yet for licence headers, generated files or vendored paths.
- Untracked files are invisible to the default mode, matching `git diff HEAD`.
  Stage them, or use `--staged`.
- A method moved between files reads as added in one file and removed in the
  other. File renames are handled.

## Development

```sh
cargo build --release                             # target/release/commentbuster
cargo test                                        # 99 tests
node --test npm/test/                             # the npm packages
CB=$PWD/target/release/commentbuster \
  ./demo/capture-evidence.sh /tmp/cb-evidence     # rebuild the demo runs
```

`SPEC.md` is the specification, including what implementation changed and why.

### Releasing

Bump `Cargo.toml`, merge, then tag `v<version>`. The release workflow builds the
binaries, creates the GitHub release and publishes the npm packages with trusted
publishing. The same script publishes by hand from a downloaded release, and
skips whatever is already on npm:

```sh
gh release download v0.3.0 --dir release
node npm/publish.mjs release --dry-run
node npm/publish.mjs release
```
