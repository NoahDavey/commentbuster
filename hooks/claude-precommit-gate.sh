#!/usr/bin/env bash
#
# Claude Code PreToolUse hook: check the staged comment budget before a commit.
#
# install.sh puts this next to the binary as ~/.local/bin/commentbuster-claude-gate,
# so a project's .claude/settings.json (team-wide) or .claude/settings.local.json
# (personal) can reference it the same way on every machine:
#
#   "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [
#     { "type": "command", "command": "$HOME/.local/bin/commentbuster-claude-gate" } ] } ] }
#
# If the script is vendored into a repo instead, reference it as
# "$CLAUDE_PROJECT_DIR/.claude/hooks/claude-precommit-gate.sh".
#
# Why here rather than a git pre-commit hook: this intercepts the Bash tool call,
# not git's hook chain, so `--no-verify` does not get around it; and it fires only
# for agents, leaving a human's own commits alone.
#
# The gate is complementary, not overlapping: above MIN_CODE_LINES added code
# lines the ratio decides, below it the absolute net decides. Applying both to
# every commit would flag large, proportionately-commented changes.
#
# Tuning (all optional):
#   COMMENTBUSTER_MAX_RATIO       ratio ceiling for larger commits   (0.15)
#   COMMENTBUSTER_MAX_NET_SMALL   net ceiling for small commits      (5)
#   COMMENTBUSTER_MIN_CODE_LINES  boundary between the two rules     (20)
#   COMMENTBUSTER_BIN             path to the binary (default: next to this
#                                 script, then `commentbuster` on PATH)
#   COMMENTBUSTER_SKIP            set to anything to bypass the gate
set -uo pipefail

PAYLOAD=$(cat)

# Most Bash calls are not commits. A substring test on the raw payload costs
# nothing and avoids starting a JSON parser on every tool call in a session.
case "$PAYLOAD" in
  *"git commit"*) ;;
  *) exit 0 ;;
esac

[ -n "${COMMENTBUSTER_SKIP:-}" ] && exit 0

CB=""
for candidate in \
  "${COMMENTBUSTER_BIN:-}" \
  "$(dirname "$0")/commentbuster" \
  "$(command -v commentbuster 2>/dev/null)" \
  "$HOME/.local/bin/commentbuster" \
  "$HOME/.cargo/bin/commentbuster"
do
  if [ -n "$candidate" ] && [ -x "$candidate" ]; then CB="$candidate"; break; fi
done

# Never block a commit because the tool is missing.
if [ -z "$CB" ]; then
  echo "commentbuster-gate: commentbuster binary not found, allowing the commit." >&2
  echo "commentbuster-gate: install it: https://github.com/NoahDavey/commentbuster#install" >&2
  exit 0
fi

export PAYLOAD CB
export MAX_RATIO="${COMMENTBUSTER_MAX_RATIO:-0.15}"
export MAX_NET_SMALL="${COMMENTBUSTER_MAX_NET_SMALL:-5}"
export MIN_CODE="${COMMENTBUSTER_MIN_CODE_LINES:-20}"

exec python3 - <<'PY'
import json
import os
import re
import subprocess
import sys


def allow():
    sys.exit(0)


def deny(reason):
    print(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }))
    sys.exit(0)


try:
    command = json.loads(os.environ["PAYLOAD"]).get("tool_input", {}).get("command", "")
except (ValueError, KeyError):
    allow()

# The fast path matched the raw payload, which could have been a commit message
# or an echo that merely mentions the words. `git` must start a shell segment,
# so `git add -A && git commit -m ...` matches but `echo "run git commit"` does not.
if not re.search(r"(?:^|[;&|(\n])\s*git\b[^;&|\n]*\bcommit\b", command):
    allow()

proc = subprocess.run([os.environ["CB"], "--staged"], capture_output=True, text=True)
if proc.returncode == 2 or not proc.stdout.strip():
    allow()  # not a git repo, bad state — the gate has nothing to say

try:
    report = json.loads(proc.stdout)
except ValueError:
    allow()

totals = report["totals"]
added = totals["comment_lines_added"]
removed = totals["comment_lines_removed"]
net = totals["comment_lines_net"]
code = totals["code_lines_added"]
ratio = totals["comment_ratio"]

max_ratio = float(os.environ["MAX_RATIO"])
max_net_small = int(os.environ["MAX_NET_SMALL"])
min_code = int(os.environ["MIN_CODE"])

# Complementary rules: the ratio governs commits with enough code to normalise
# against; the absolute net governs the small ones, which is where a
# ratio would be meaningless and where over-commenting hides.
if code >= min_code:
    if ratio is None or ratio <= max_ratio:
        allow()
    headline = (
        f"adds {added} comment lines against {code} code lines "
        f"— {ratio * 100:.0f}%, over the {max_ratio * 100:.0f}% ceiling"
    )
else:
    if net <= max_net_small:
        allow()
    headline = (
        f"adds {net} net comment lines ({added} added, {removed} removed) "
        f"on only {code} added code lines — over the ceiling of {max_net_small} "
        f"for a commit this small"
    )

worst = [f for f in report["files"] if f["comment_lines_net"] > 0][:6]
lines = [f"  {f['path']}  +{f['comment_lines_net']}" for f in worst]

deny(
    "Comment budget exceeded — this commit " + headline + ".\n\n"
    + "\n".join(lines)
    + "\n\nRevise the comments before committing. Cut the ones that restate the code: "
    "a doc block that repeats the signature, a line narrating the statement below it, "
    "commented-out code, a comment that names what a well-named function already says. "
    "Keep the ones that explain why a choice was made, record a constraint or invariant, "
    "or warn about something non-obvious. Then re-stage the files and commit again."
)
PY
