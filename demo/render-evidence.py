#!/usr/bin/env python3
"""Render a captured commentbuster run as a terminal screenshot.

Reads run-N.cmd / run-N.out / run-N.exit, converts the ANSI output to HTML, and
screenshots it with headless Chrome. The pixels come from the real run.
"""
import html
import re
import subprocess
import sys
from pathlib import Path

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
SGR = re.compile(r"\x1b\[([0-9;]*)m")
FONT_PX = 13
LINE_PX = 20
CHAR_PX = 7.82

PALETTE = {"31": "#f7768e", "32": "#9ece6a", "36": "#7dcfff", "33": "#e0af68"}


def ansi_to_html(text: str) -> str:
    out, open_spans = [], 0
    pos = 0
    for m in SGR.finditer(text):
        out.append(html.escape(text[pos:m.start()]))
        pos = m.end()
        codes = [c for c in m.group(1).split(";") if c] or ["0"]
        for code in codes:
            if code == "0":
                out.append("</span>" * open_spans)
                open_spans = 0
            elif code == "1":
                out.append('<span class="b">')
                open_spans += 1
            elif code == "2":
                out.append('<span class="d">')
                open_spans += 1
            elif code in PALETTE:
                out.append(f'<span style="color:{PALETTE[code]}">')
                open_spans += 1
    out.append(html.escape(text[pos:]))
    out.append("</span>" * open_spans)
    return "".join(out)


def render(ev: Path, run: str, title: str, prompt: str, out_dir: Path) -> Path:
    cmd = (ev / f"run-{run}.cmd").read_text().rstrip("\n")
    body = (ev / f"run-{run}.out").read_text().rstrip("\n")
    code = (ev / f"run-{run}.exit").read_text().strip()

    plain = SGR.sub("", body).split("\n")
    cols = max([len(l) for l in plain] + [len(prompt) + len(cmd) + 2, len(title) + 8, 34])
    width = int(cols * CHAR_PX) + 56
    height = 40 + 22 + (len(plain) + 3) * LINE_PX + 34

    ok = code == "0"
    page = f"""<!doctype html><meta charset="utf-8"><style>
* {{ box-sizing: border-box; }}
body {{ margin:0; background:#11121a; font-family:"SF Mono",Menlo,monospace; }}
.win {{ width:{width}px; background:#1a1b26; border-radius:9px; overflow:hidden;
        box-shadow:0 8px 28px rgba(0,0,0,.5); }}
.bar {{ height:40px; background:#20212e; display:flex; align-items:center;
        padding:0 14px; gap:7px; border-bottom:1px solid #2a2b3c; }}
.dot {{ width:11px; height:11px; border-radius:50%; }}
.t {{ flex:1; text-align:center; color:#787c99; font-size:11.5px;
      letter-spacing:.04em; margin-right:44px; }}
pre {{ margin:0; padding:14px 24px 20px; color:#c0caf5; font-size:{FONT_PX}px;
       line-height:{LINE_PX}px; white-space:pre; }}
.b {{ font-weight:700; }} .d {{ color:#565a7e; }}
.p {{ color:#565a7e; }} .c {{ color:#c0caf5; font-weight:600; }}
.x {{ color:{'#9ece6a' if ok else '#f7768e'}; }}
</style>
<div class="win">
  <div class="bar">
    <div class="dot" style="background:#ff5f57"></div>
    <div class="dot" style="background:#febc2e"></div>
    <div class="dot" style="background:#28c840"></div>
    <div class="t">{html.escape(title)}</div>
  </div>
  <pre><span class="p">{html.escape(prompt)}</span> <span class="c">{html.escape(cmd)}</span>
{ansi_to_html(body)}
<span class="p">$ echo $?</span>
<span class="x b">{html.escape(code)}</span></pre>
</div>"""

    tmp = out_dir / f"run-{run}.html"
    tmp.write_text(page)
    png = out_dir / f"run-{run}.png"
    subprocess.run(
        [CHROME, "--headless", "--disable-gpu", "--hide-scrollbars",
         f"--screenshot={png}", f"--window-size={width},{height}",
         "--force-device-scale-factor=2", "--default-background-color=11121a",
         tmp.as_uri()],
        check=True, capture_output=True,
    )
    return png


if __name__ == "__main__":
    ev = Path(sys.argv[1])
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    runs = [
        ("1", "the over-commented feature PR", "acme-orders (feature/discount-codes) $"),
        ("2", "the cleanup PR", "acme-orders (chore/strip-stale-comments) $"),
        ("3", "default output, for CI", "acme-orders (feature/discount-codes) $"),
        ("4a", "merge-base, the default", "acme-orders (feature/discount-codes) $"),
        ("4b", "a literal two-dot comparison", "acme-orders (feature/discount-codes) $"),
        ("5", "the pre-commit hook", "acme-orders (chore/strip-stale-comments) $"),
        ("6", "400 changed files", "big-repo (main) $"),
    ]
    for run, title, prompt in runs:
        png = render(ev, run, title, prompt, out)
        size = png.stat().st_size
        print(f"run-{run}.png  {size/1024:.0f} KB")
