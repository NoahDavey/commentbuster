#!/usr/bin/env bash
# Captures every evidence run in one reproducible pass. For each run N it writes
# run-N.cmd (the command as typed), run-N.out (stdout, ANSI intact) and
# run-N.exit (the exit code).
set -euo pipefail

CB="${CB:?set CB to the commentbuster binary}"
WORK="${1:?usage: capture-evidence.sh <workdir>}"
HERE="$(cd "$(dirname "$0")" && pwd)"

DEMO="$WORK/demo-repo"
BIG="$WORK/big-repo"
EV="$WORK/evidence"

rm -rf "$EV"; mkdir -p "$EV"
"$HERE/build-demo-repo.sh" "$DEMO" >/dev/null

capture() {
  local n="$1"; local dir="$2"; local label="$3"; shift 3
  printf '%s\n' "$label" > "$EV/run-$n.cmd"
  ( cd "$dir" && set +e; "$@" > "$EV/run-$n.out" 2>"$EV/run-$n.err"; echo $? > "$EV/run-$n.exit" )
}

# 1 — the over-commented feature PR, gated on ratio
git -C "$DEMO" checkout -q feature/discount-codes
capture 1 "$DEMO" 'commentbuster main --pretty --max-ratio 0.15' \
  "$CB" main --pretty --color always --max-ratio 0.15

# 2 — the cleanup PR: net reduction earns budget
git -C "$DEMO" checkout -q chore/strip-stale-comments
capture 2 "$DEMO" 'commentbuster feature/discount-codes --pretty --max-net 0' \
  "$CB" feature/discount-codes --pretty --color always --max-net 0

# 3 — the default output: JSON, for CI
git -C "$DEMO" checkout -q feature/discount-codes
capture 3 "$DEMO" 'commentbuster main --max-ratio 0.15        # JSON is the default' \
  "$CB" main --max-ratio 0.15

# 4 — merge-base (default) against a literal comparison, same two refs
capture 4a "$DEMO" 'commentbuster main HEAD --pretty --top 3            # merge-base, the default' \
  "$CB" main HEAD --pretty --color always --top 3
capture 4b "$DEMO" 'commentbuster --direct main HEAD --pretty --top 3   # literal, two-dot' \
  "$CB" --direct main HEAD --pretty --color always --top 3

# 5 — the pre-commit hook: an agent has just written an over-commented helper
git -C "$DEMO" checkout -q chore/strip-stale-comments
cat >> "$DEMO/web/src/orders.ts" <<'EOF'

/**
 * Formats a money amount for display.
 *
 * @param amount - the amount
 * @returns the formatted string
 */
export function formatMoney(amount: number): string {
  // Convert the amount to a fixed string
  const fixed = amount.toFixed(2);
  // Return the formatted string
  return `£${fixed}`;
}
EOF
git -C "$DEMO" add web/src/orders.ts
capture 5 "$DEMO" 'commentbuster --staged --pretty --max-net 3   # the pre-commit hook' \
  "$CB" --staged --pretty --color always --max-net 3
git -C "$DEMO" reset -q
git -C "$DEMO" checkout -q -- web/src/orders.ts

# 6 — speed on a large diff
python3 - "$BIG" <<'PY'
import os, subprocess, sys
dest = sys.argv[1]
subprocess.run(["rm", "-rf", dest], check=True)
os.makedirs(f"{dest}/src")

def java(i, commented):
    s = [f"package demo.p{i};", ""]
    if commented:
        s += ["/**", f" * Generated class number {i}.", " *", " * @see Nothing", " */"]
    s += [f"public class C{i} {{"]
    for m in range(20):
        if commented:
            s.append(f"  // method {m} does a thing")
        s += [f"  int m{m}(int a) {{", f"    return a + {m}; {'// add' if commented else ''}", "  }"]
    s += ["}", ""]
    return "\n".join(s)

def run(*a): subprocess.run(a, cwd=dest, check=True, stdout=subprocess.DEVNULL)
run("git", "init", "-q", "-b", "main")
run("git", "config", "user.email", "demo@example.com")
run("git", "config", "user.name", "Demo")
for commented in (False, True):
    for i in range(400):
        open(f"{dest}/src/C{i}.java", "w").write(java(i, commented))
    run("git", "add", "-A")
    run("git", "commit", "-q", "-m", "commented" if commented else "base")
PY
capture 6 "$BIG" 'commentbuster HEAD~1 --pretty --top 4   # 400-file diff' \
  "$CB" HEAD~1 --pretty --color always --top 4

for n in 1 2 3 4a 4b 5 6; do
  printf 'run %-3s exit=%s  %s\n' "$n" "$(cat "$EV/run-$n.exit")" "$(head -1 "$EV/run-$n.cmd")"
done
