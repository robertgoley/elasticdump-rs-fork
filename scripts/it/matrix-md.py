#!/usr/bin/env python3
"""INF-011: regenerate the generated blocks of TESTING-MATRIX.md.

  matrix-md.py [--summary FILE ...] [--check]

Blocks (between `<!-- BEGIN generated:<name> -->` and `<!-- END generated:<name> -->`):
  digests   every image in scripts/it/images.lock with its digest and, when present,
            the newest IT-SUMMARY result for each profile that ran it
  goldens   index of tests/golden/<engine>-<version>/<case>/ recordings (INF-003)
  expect    the cross-version expectation table, tests/golden/expectations.md (API-071)

IT-SUMMARY lines come from scripts/it/run.sh, which also appends them to
$IT_DIR/it-summary.log; pass that file (or CI logs) with --summary.
--check exits 1 if the file would change (CI: the matrix is current).
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MD = ROOT / "TESTING-MATRIX.md"
LOCK = ROOT / "scripts/it/images.lock"
GOLDEN = ROOT / "tests/golden"
SUMMARY = re.compile(r"IT-SUMMARY profile=(\S+) (?:image=(\S*))?(?: os=(\S+))?\s*(.*)$")


def lock():
    out = []
    for line in LOCK.read_text().splitlines():
        if line.strip() and not line.startswith("#"):
            img, d = line.split()[:2]
            out.append((img, d))
    return out


def summaries(files):
    by_image = {}
    for f in files:
        for line in Path(f).read_text(errors="replace").splitlines():
            m = SUMMARY.search(line)
            if not m:
                continue
            prof, es, os_, rest = m.groups()
            img = (os_ or es or "").split("@")[0]
            res = "SKIPPED (resource guard)" if "SKIPPED" in rest else rest.strip()
            res = re.sub(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored.*?(skipped-by-reason=\d+)?$",
                         lambda x: f"{x[1]}: {x[2]} pass / {x[3]} fail / {x[4]} ignored" + (f", {x[5]}" if x[5] else ""), res)
            by_image.setdefault(img, {})[prof] = res  # newest line wins
    return by_image


def previous_results(text):
    """Result cells already in the file, so a run without --summary keeps them."""
    out = {}
    for m in re.finditer(r"^\| `([^`]+)` \| `sha256:[0-9a-f]+` \| (.*) \|$", text, re.M):
        out[m[1]] = m[2]
    return out


def block_digests(files, text):
    res = summaries(files)
    prev = previous_results(text)
    rows = ["| Image | Digest | Latest IT-SUMMARY |", "|---|---|---|"]
    for img, d in lock():
        r = "; ".join(f"`{p}` {v}" for p, v in sorted(res.get(img, {}).items())) or prev.get(img, "–")
        rows.append(f"| `{img}` | `{d}` | {r} |")
    return "\n".join(rows)


def block_goldens():
    if not GOLDEN.is_dir():
        return "_No recordings yet (`IT_RECORD=1`, INF-003)._"
    rows = ["| Engine-version | Cases | Exchanges |", "|---|---|---|"]
    for ev in sorted(p for p in GOLDEN.iterdir() if p.is_dir()):
        cases = sorted(c for c in ev.iterdir() if c.is_dir())
        n = sum(len(list(c.glob("*.req.json"))) for c in cases)
        if cases:
            rows.append(f"| {ev.name} | {len(cases)} | {n} |")
    return "\n".join(rows) if len(rows) > 2 else "_No recordings yet (`IT_RECORD=1`, INF-003)._"


def block_expect():
    f = GOLDEN / "expectations.md"
    return f.read_text().strip() if f.is_file() else "_Expectation table not committed yet (API-071)._"


def render(text, name, body):
    b, e = f"<!-- BEGIN generated:{name} -->", f"<!-- END generated:{name} -->"
    if b not in text:
        return text
    pre, rest = text.split(b, 1)
    _, post = rest.split(e, 1)
    return f"{pre}{b}\n{body}\n{e}{post}"


def main(a):
    files = [a[i + 1] for i, x in enumerate(a) if x == "--summary"]
    old = MD.read_text()
    new = render(old, "digests", block_digests(files, old))
    new = render(new, "goldens", block_goldens())
    new = render(new, "expect", block_expect())
    if "--check" in a:
        if new != old:
            print("matrix-md: TESTING-MATRIX.md is stale; run scripts/it/matrix-md.py", file=sys.stderr)
            return 1
        print("matrix-md: up to date")
        return 0
    MD.write_text(new)
    print("matrix-md: TESTING-MATRIX.md regenerated")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
