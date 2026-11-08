#!/usr/bin/env python3
"""LOAD-004 scenario matrix (L-801): run every scenario in scenarios/matrix.toml at
one tier and gate each cell for correctness and against a perf baseline.

    scripts/load/matrix.sh <tier> [--place shared] [--engine E] [--only a,b]
                           [--baseline FILE] [--write-baseline] [--threshold PCT] [--docs N]

A cell is one step of one scenario (e.g. matrix-restore/c4-l5000). Gates:

* correctness (always): the step's `edverify diff` passed, the tool exited 0, and
  the scenario passed its write-count, RSS-budget and leak checks;
* performance: docs/s may not fall, and tool max RSS may not rise, by more than
  the threshold against the baseline. The default baseline is
  scripts/load/baselines/<place>-<tier>-<engine>.json, and perf numbers are only
  compared within the same place/tier/engine. A cell with no baseline entry is
  reported as `no-baseline` and does not fail.

A tier that does not fit the place is refused by run.py; matrix.py reports it as
`refused` with what it needs (a dedicated runner) and exits 2.

Writes results/load/<run-id>/matrix.json. Exit codes: 0 pass, 1 correctness or perf
failure, 2 refused, 3 skipped by the resource guard.
"""
import argparse, datetime, json, os, subprocess, sys, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "shared"))
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--only", default=None, help="comma-separated scenario names")
    ap.add_argument("--baseline", default=None)
    ap.add_argument("--write-baseline", action="store_true")
    ap.add_argument("--threshold", type=float, default=None)
    ap.add_argument("--docs", type=int, default=None, help="override doc count (smoke; no baseline compare)")
    ap.add_argument("--run-id", default=None)
    a = ap.parse_args()

    mx = tomllib.load(open(os.path.join(HERE, "scenarios", "matrix.toml"), "rb"))
    scen = a.only.split(",") if a.only else mx["scenarios"]
    thr = a.threshold if a.threshold is not None else mx.get("threshold_pct", 30)
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-{a.tier}-matrix"
    base_path = a.baseline or os.path.join(HERE, "baselines", f"{a.place}-{a.tier}-{a.engine}.json")
    base = json.load(open(base_path)) if os.path.exists(base_path) and not a.docs else {"cells": {}}
    size = next((s for s in mx.get("size", []) if s["tier"] == a.tier), {})

    cells, scen_res, worst = {}, {}, 0
    for sc in scen:
        cmd = [sys.executable, os.path.join(HERE, "run.py"), a.tier, sc, "--place", a.place,
               "--engine", a.engine, "--run-id", run_id]
        if a.docs:
            cmd += ["--docs", str(a.docs)]
        p = subprocess.run(cmd, capture_output=True, text=True)
        sys.stderr.write(p.stderr)
        summary = [line for line in p.stdout.splitlines() if line.startswith(("LOAD-SUMMARY", "  failure:"))]
        print("\n".join(summary), flush=True)
        if p.returncode in (2, 3):
            state = "refused" if p.returncode == 2 else "skipped"
            scen_res[sc] = dict(state=state, detail=summary[0] if summary else "", needs=size.get("note"))
            worst = max(worst, p.returncode)
            continue
        res_path = os.path.join(ROOT, "results", "load", run_id, sc + ".json")
        if not os.path.exists(res_path):
            # run.py stopped before writing a result (e.g. "edgen not built"): a failed
            # scenario, not a crash of the whole matrix
            errs = [line for line in p.stderr.splitlines() if line.strip()]
            reason = errs[-1] if errs else f"run.py exited {p.returncode} without a result file"
            scen_res[sc] = dict(state="fail", detail=reason, exit=p.returncode)
            print(f"matrix: scenario {sc} FAILED (no result file): {reason}", flush=True)
            worst = 1 if worst in (0, 3) else worst
            continue
        res = json.load(open(res_path))
        scen_res[sc] = dict(state="pass" if res["pass"] else "fail", failures=res["failures"],
                            node_peak_rss_mb=res.get("node", {}).get("peak_rss_mb"), scaled=res.get("scaled"),
                            seeds=[{k: s[k] for k in ("index", "stored", "seconds")} for s in res["seeds"]])
        for st in res["steps"]:
            key = f"{sc}/{st.get('cell', st['name'])}"
            ok = bool(st.get("verify", {}).get("ok")) and st["exit"] == 0 and not [
                f for f in res["failures"] if f.startswith(st["name"] + ":")]
            c = dict(correct=ok, docs_per_s=st.get("docs_per_s"), wall_s=st.get("wall_s"),
                     max_rss_kb=st.get("max_rss_kb"), docs=st.get("verify", {}).get("b_count"))
            for k in ("parts", "bytes_out", "reference_docs"):
                if k in st:
                    c[k] = st[k]
            b = base["cells"].get(key)
            if b is None:
                c["perf"] = "no-baseline"
            else:
                bad = []
                if b.get("docs_per_s") and c["docs_per_s"] is not None and \
                        c["docs_per_s"] < b["docs_per_s"] * (1 - thr / 100):
                    bad.append(f"docs/s {c['docs_per_s']} < {b['docs_per_s']} -{thr}%")
                if b.get("max_rss_kb") and c["max_rss_kb"] and c["max_rss_kb"] > b["max_rss_kb"] * (1 + thr / 100):
                    bad.append(f"max RSS {c['max_rss_kb']} KB > {b['max_rss_kb']} KB +{thr}%")
                c["perf"] = "REGRESSION: " + "; ".join(bad) if bad else "ok"
            cells[key] = c
        if not res["pass"]:
            worst = max(worst, 1) if worst != 3 else worst

    if any(not c["correct"] or c["perf"].startswith("REGRESSION") for c in cells.values()):
        worst = 1 if worst in (0, 3) else worst
    out = dict(schema="edrs-load-matrix/v1", run_id=run_id, tier=a.tier, place=a.place, engine=a.engine,
               threshold_pct=thr, baseline=os.path.relpath(base_path, ROOT) if base["cells"] else None,
               docs_override=a.docs, scenarios=scen_res, cells=cells)
    os.makedirs(os.path.join(ROOT, "results", "load", run_id), exist_ok=True)
    path = os.path.join(ROOT, "results", "load", run_id, "matrix.json")
    json.dump(out, open(path, "w"), indent=1)
    for k, c in cells.items():
        print(f"CELL {k:34} {'ok  ' if c['correct'] else 'FAIL'} {c['docs_per_s']!s:>10} d/s "
              f"rss {c['max_rss_kb']!s:>7} KB  perf={c['perf']}")
    if a.write_baseline:
        if a.docs or worst:
            print("matrix: not writing a baseline from a smoke (--docs) or failed run")
            return worst or 1
        bl = dict(schema="edrs-load-baseline/v1", tier=a.tier, place=a.place, engine=a.engine,
                  git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                     capture_output=True, text=True).stdout.strip(),
                  recorded=datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
                  note="perf baseline for this place/tier/engine only; scaled runs are not comparable "
                       "with full-size ones",
                  cells={k: {m: c[m] for m in ("docs_per_s", "wall_s", "max_rss_kb", "docs")} for k, c in cells.items()})
        old = json.load(open(base_path)) if os.path.exists(base_path) else None
        if old and a.only:  # partial run: merge into the existing baseline
            old["cells"].update(bl["cells"])
            bl["cells"] = old["cells"]
        os.makedirs(os.path.dirname(base_path), exist_ok=True)
        json.dump(bl, open(base_path, "w"), indent=1, sort_keys=False)
        open(base_path, "a").write("\n")
        print(f"matrix: baseline written to {os.path.relpath(base_path, ROOT)}")
    print(f"MATRIX-SUMMARY tier={a.tier} place={a.place} cells={len(cells)} "
          f"correct={sum(c['correct'] for c in cells.values())} "
          f"perf_regressions={sum(c['perf'].startswith('REGRESSION') for c in cells.values())} "
          f"refused={[s for s, r in scen_res.items() if r['state'] == 'refused']} "
          f"failed_no_result={[s for s, r in scen_res.items() if r['state'] == 'fail' and 'exit' in r]} result={os.path.relpath(path, ROOT)}")
    return worst


if __name__ == "__main__":
    sys.exit(main())
