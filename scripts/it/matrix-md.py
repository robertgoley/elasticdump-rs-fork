#!/usr/bin/env python3
"""INF-011: regenerate the generated blocks of TESTING-MATRIX.md.

  matrix-md.py [--summary FILE ...] [--check]

Blocks (between `<!-- BEGIN generated:<name> -->` and `<!-- END generated:<name> -->`):
  digests   every image in scripts/it/images.lock with its digest and, when present,
            the newest IT-SUMMARY result for each profile that ran it
  goldens   index of tests/golden/<engine>-<version>/<case>/ recordings (INF-003)
  expect    the cross-version expectation table, tests/golden/expectations.md (API-071)
  contract  endpoint x engine-version and case x engine-version from the goldens (API-002)

IT-SUMMARY lines come from scripts/it/run.sh, which also appends them to
$IT_DIR/it-summary.log; pass that file (or CI logs) with --summary.
--check exits 1 if the file would change (CI: the matrix is current).
"""
import json
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


def endpoints():
    import tomllib

    f = GOLDEN / "endpoints.toml"
    if not f.is_file():
        return []
    return tomllib.loads(f.read_text()).get("endpoint", [])


def match_endpoint(eps, req):
    keys = {k for k, _ in req.get("query", [])}
    for e in eps:
        if req["method"] in e["method"] and re.search(e["path"], req["path"]) and (
            "query" not in e or e["query"] in keys
        ):
            return e["id"]
    return None


def block_contract():
    """API-002: endpoint x engine-version and case x engine-version, from the committed goldens.

    Endpoint cell: `✓` every recorded exchange 2xx; `✓/<st>` 2xx and some other status; `✗ <st>`
    only non-2xx (asserted as such by the replay); `n/a` not sent by any case on that version.
    Case cell: `✓` exit 0; `fails (exit N)` is the asserted expected failure (replayed exactly);
    `✓ scroll` means `--searchWithPit` fell back to scroll (no PIT API on that version)."""
    if not GOLDEN.is_dir():
        return "_No recordings yet (`IT_RECORD=1`, INF-003)._"
    eps = endpoints()
    evs = sorted(p for p in GOLDEN.iterdir() if p.is_dir() and any(c.is_dir() for c in p.iterdir()))
    seen, cases = {}, {}
    for ev in evs:
        for c in sorted(x for x in ev.iterdir() if x.is_dir()):
            meta = (c / "meta.toml").read_text() if (c / "meta.toml").is_file() else ""
            m = re.search(r"^exit_code = (\d+)", meta, re.M)
            code = int(m.group(1)) if m else 0
            paths = []
            for r in sorted(c.glob("*.req.json")):
                req = json.loads(r.read_text())
                resp = json.loads(r.with_name(r.name.replace(".req.", ".resp.")).read_text())
                paths.append(req["path"])
                eid = match_endpoint(eps, req) or "unlisted"
                seen.setdefault((eid, ev.name), set()).add(int(resp.get("status", 0)))
            if code != 0:
                cell = f"fails (exit {code})"
            elif c.name == "data-pit" and not any("_pit" in p or "point_in_time" in p for p in paths):
                cell = "✓ scroll"
            else:
                cell = "✓"
            cases.setdefault(c.name, {})[ev.name] = cell
    head = "| | " + " | ".join(e.name for e in evs) + " |"
    sep = "|---" * (len(evs) + 1) + "|"
    rows = ["Endpoint x engine-version:", "", head.replace("| |", "| Endpoint |", 1), sep]
    for e in eps + [{"id": "unlisted", "title": "matches no inventory row"}]:
        cells = []
        for ev in evs:
            st = seen.get((e["id"], ev.name))
            if not st:
                cells.append("n/a")
                continue
            ok = {s for s in st if 200 <= s < 300}
            bad = sorted(st - ok)
            cells.append("✓" + (f"/{','.join(map(str, bad))}" if bad else "") if ok else "✗ " + ",".join(map(str, bad)))
        if e["id"] == "unlisted" and all(c == "n/a" for c in cells):
            continue
        rows.append(f"| {e['id']} {e['title'].split(' (')[0]} | " + " | ".join(cells) + " |")
    rows += ["", "Case x engine-version:", "", head.replace("| |", "| Case |", 1), sep]
    for name in sorted(cases):
        rows.append(f"| {name} | " + " | ".join(cases[name].get(ev.name, "–") for ev in evs) + " |")
    return "\n".join(rows)


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
    new = render(new, "contract", block_contract())
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
