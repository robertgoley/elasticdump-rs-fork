#!/usr/bin/env python3
"""Aggregate per-engine perf runs (results/perf/<run>/combined.json) into one markdown report.

  scripts/perf/aggregate.py --title T [--old-baselines GLOB] RUN_DIR... > report.md

For each engine: per-case searchdump / Node wall medians, Node/searchdump "× faster", status,
and the wall delta against an older baseline cell (e.g. the pre-1.0 box baselines in
scripts/perf/baselines/), which is informational only (the identity differs, so suite.py
itself cannot compare).
"""
import argparse, glob, json, os, statistics, sys

ap = argparse.ArgumentParser()
ap.add_argument("--title", default="Perf suite")
ap.add_argument("--old-baselines", default="")
ap.add_argument("runs", nargs="+")
a = ap.parse_args()
old = {}
for f in glob.glob(a.old_baselines) if a.old_baselines else []:
    eng = os.path.basename(f).rsplit("-L0-", 1)[-1][:-5]
    old[eng] = json.load(open(f)).get("cells", {})


def wall(m):
    return None if not m or m.get("wall_s") is None else m["wall_s"]


def x_faster(rs_s, node_s):
    if rs_s and node_s:
        return node_s / rs_s
    return None


def fmt_s(v):
    return "—" if v is None else f"{v:.3f}"


def fmt_x(v):
    return "—" if v is None else f"{v:.2f}×"


def fmt_delta(rs_s, ob):
    if not (ob and rs_s is not None and ob.get("wall_s")):
        return ""
    dp = (rs_s - ob["wall_s"]) / ob["wall_s"] * 100
    return f"{dp:+.0f}%"


# Collect engines in run order, and a stable case list (first occurrence wins).
engines = []  # list of dicts: name, version, run, failures, rows{case: summary}, missing
case_order = []
seen_cases = set()
tot = dict(cases=0, passed=0, failed=0, no_baseline=0, skipped=0, ratios=[])
anomalies = []
engine_failures = []
interrupted = []

for r in a.runs:
    p = os.path.join(r, "combined.json")
    if not os.path.exists(p):
        interrupted.append(os.path.basename(r))
        continue
    d = json.load(open(p))
    for e in d.get("engines") or []:
        name = e.get("engine", os.path.basename(r))
        rec = {
            "name": name,
            "version": e.get("engine_version", "?"),
            "run": os.path.basename(r),
            "failures": list(e.get("failures") or []),
            "rows": {},
        }
        if rec["failures"]:
            engine_failures.append((name, rec["run"], rec["failures"]))
        for s in e.get("summary") or []:
            case = s["case"]
            rec["rows"][case] = s
            if case not in seen_cases:
                seen_cases.add(case)
                case_order.append(case)
            rs, nd = s.get("rs") or {}, s.get("node") or {}
            rs_s, nd_s = wall(rs), wall(nd)
            ratio = x_faster(rs_s, nd_s)
            st = s.get("status") or ""
            tot["cases"] += 1
            if "NO_BASELINE" in st:
                tot["no_baseline"] += 1
            if st.startswith("PASS"):
                tot["passed"] += 1
            elif st.startswith("SKIP") or st.startswith("NOT_RUN") or st.startswith("REFUSED"):
                tot["skipped"] += 1
            else:
                tot["failed"] += 1
            if ratio is not None:
                tot["ratios"].append(ratio)
                if ratio < 1.0:
                    anomalies.append(
                        f"{name}/{case}: searchdump slower than Node ({fmt_x(ratio)}; "
                        f"{fmt_s(rs_s)}s vs {fmt_s(nd_s)}s)"
                    )
            ob = old.get(name, {}).get(case)
            if ob and rs_s is not None:
                dp = (rs_s - ob["wall_s"]) / ob["wall_s"] * 100
                if dp > 25 and rs_s - ob["wall_s"] > 0.3:
                    anomalies.append(
                        f"{name}/{case}: old baseline {ob['wall_s']:.3f}s -> {rs_s:.3f}s ({dp:+.0f}%)"
                    )
        engines.append(rec)

out = [f"# {a.title}", ""]
out.append(
    'Legend: "× faster" is Node wall / searchdump wall (higher means searchdump is faster). '
    "Wall-clock medians are in seconds. "
    "`old baseline` is the searchdump wall change vs the earlier box baseline for the same "
    "engine/case (informational only; identities differ, so suite.py cannot compare them)."
)
out.append("")

head = (
    f"**Totals:** {tot['cases']} case results, {tot['passed']} PASS, {tot['failed']} FAIL, "
    f"{tot['no_baseline']} NO_BASELINE, {tot['skipped']} skipped/not-run"
)
if tot["ratios"]:
    head += (
        f"; × faster median {statistics.median(tot['ratios']):.2f}× "
        f"over {len(tot['ratios'])} Node-compared cases"
    )
out += [head, ""]

if engine_failures:
    out += ["## Engine failures", ""]
    for name, run, fails in engine_failures:
        msg = "; ".join(str(f).replace("\n", " ") for f in fails)
        out.append(f"- `{name}` (run `{run}`): {msg}")
    out.append("")
if interrupted:
    out += ["Runs with no `combined.json` (interrupted): " + ", ".join(f"`{x}`" for x in interrupted), ""]

if case_order and engines:
    out += ["## Cross-engine matrix", "", "Each cell is × faster (Node / searchdump) for that case.", ""]
    header = "| Case | " + " | ".join(f"`{e['name']}`" for e in engines) + " |"
    sep = "|---|" + "|".join("---:" for _ in engines) + "|"
    out += [header, sep]
    for case in case_order:
        cells = []
        for e in engines:
            s = e["rows"].get(case)
            if not s:
                cells.append("—")
                continue
            st = s.get("status") or ""
            if st.startswith("FAIL: setup") or st.startswith("REFUSED") or st.startswith("NOT_RUN") or st.startswith("FAIL: orchestration"):
                cells.append("setup fail" if "FAIL: setup" in st or "orchestration" in st else "—")
                continue
            if st.startswith("SKIP"):
                cells.append("SKIP")
                continue
            xf = x_faster(wall(s.get("rs")), wall(s.get("node")))
            cells.append(fmt_x(xf) if xf is not None else ("—" if not st.startswith("PASS") else "—"))
        out.append("| " + case + " | " + " | ".join(cells) + " |")
    out.append("")

if anomalies:
    out += ["**Anomalies:**", ""] + [f"- {x}" for x in anomalies] + [""]

for e in engines:
    out += [f"## {e['name']}", "", f"engine `{e['version']}`, run `{e['run']}`", ""]
    if e["failures"]:
        out += ["Engine failures: " + "; ".join(str(f).replace("\n", " ") for f in e["failures"]), ""]
    rows = [e["rows"][c] for c in case_order if c in e["rows"]]
    setup = [s for s in rows if str(s.get("status", "")).startswith("FAIL: setup")]
    if rows and len(setup) == len(rows):
        out += [f"**Setup failure:** {len(setup)} cases not run (`{setup[0]['status']}`).", ""]
        continue
    out += [
        "| Case | searchdump s | Node s | × faster | old baseline | Status |",
        "|---|---:|---:|---:|---:|---|",
    ]
    for s in rows:
        rs_s, nd_s = wall(s.get("rs")), wall(s.get("node"))
        xf = x_faster(rs_s, nd_s)
        delta = fmt_delta(rs_s, old.get(e["name"], {}).get(s["case"]))
        out.append(
            f"| {s['case']} | {fmt_s(rs_s)} | {fmt_s(nd_s)} | {fmt_x(xf)} | {delta} | {s.get('status', '')} |"
        )
    out.append("")

print("\n".join(out).rstrip() + "\n")
