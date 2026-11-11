#!/usr/bin/env python3
"""Aggregate per-engine perf runs (results/perf/<run>/combined.json) into one markdown report.

  scripts/perf/aggregate.py --title T [--old-baselines GLOB] RUN_DIR... > report.md

For each engine: per-case rs / Node wall medians, rs/Node ratio, status, and the wall delta
against an older baseline cell (e.g. the pre-1.0 box baselines in scripts/perf/baselines/),
which is informational only (the identity differs, so suite.py itself cannot compare)."""
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
out = [f"# {a.title}", "", "Wall-clock medians in seconds; rs/Node < 1 means searchdump is faster. "
       "`old Δ` = searchdump wall vs the earlier box baseline for the same engine/case (informational).", ""]
tot = dict(cases=0, passed=0, failed=0, ratios=[])
anomalies = []
for r in a.runs:
    p = os.path.join(r, "combined.json")
    if not os.path.exists(p):
        out += [f"## {os.path.basename(r)}", "", "no combined.json (run interrupted)", ""]
        continue
    d = json.load(open(p))
    for e in d["engines"]:
        name = e["engine"]
        out += [f"## {name}", "", f"engine `{e.get('engine_version', '?')}`, run `{os.path.basename(r)}`", ""]
        if e.get("failures"):
            out += ["Engine failures: " + "; ".join(e["failures"]), ""]
        out += ["| Case | rs s | Node s | rs/Node | old Δ | Status |", "|---|---:|---:|---:|---:|---|"]
        for s in e["summary"]:
            rs, nd = s.get("rs") or {}, s.get("node") or {}
            ratio = s.get("rs_over_node")
            ob = old.get(name, {}).get(s["case"])
            delta = ""
            if ob and rs.get("wall_s"):
                dp = (rs["wall_s"] - ob["wall_s"]) / ob["wall_s"] * 100
                delta = f"{dp:+.0f}%"
                if dp > 25 and rs["wall_s"] - ob["wall_s"] > 0.3:
                    anomalies.append(f"{name}/{s['case']}: {ob['wall_s']:.3f}s -> {rs['wall_s']:.3f}s ({delta})")
            st = s.get("status", "")
            tot["cases"] += 1
            if st.startswith("PASS"): tot["passed"] += 1
            elif not st.startswith("SKIP"): tot["failed"] += 1
            if ratio:
                tot["ratios"].append(ratio)
                if ratio > 1.0:
                    anomalies.append(f"{name}/{s['case']}: searchdump slower than Node (rs/Node {ratio:.2f})")
            f = lambda m: f"{m['wall_s']:.3f}" if m.get("wall_s") is not None else "—"
            out.append(f"| {s['case']} | {f(rs)} | {f(nd)} | {ratio:.2f} | {delta} | {st} |" if ratio else
                       f"| {s['case']} | {f(rs)} | {f(nd)} | — | {delta} | {st} |")
        out.append("")
head = [f"**Totals:** {tot['cases']} case results, {tot['passed']} PASS, {tot['failed']} FAIL"
        + (f"; rs/Node median {statistics.median(tot['ratios']):.2f} over {len(tot['ratios'])} Node-compared cases"
           if tot["ratios"] else ""), ""]
if anomalies:
    head += ["**Anomalies:**", ""] + [f"- {x}" for x in anomalies] + [""]
print("\n".join(out[:4] + head + out[4:]))
