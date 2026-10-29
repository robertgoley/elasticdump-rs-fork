#!/usr/bin/env python3
"""Flag regressions between two metrics files (TST-INF-006 / LOAD-020).

  scripts/load/compare.py BASE.json NEW.json [--threshold 10]

Lower is better: wall_s, cpu_s, max_rss_kb, fds_peak; higher is better:
docs_per_s, mb_per_s_in, mb_per_s_out. Exit 1 if any metric is worse than
the threshold (percent), or if NEW has failed writes / a non-zero exit.
"""
import argparse, json, sys

LOWER = ["wall_s", "cpu_s", "max_rss_kb", "fds_peak"]
HIGHER = ["docs_per_s", "mb_per_s_in", "mb_per_s_out"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("base")
    ap.add_argument("new")
    ap.add_argument("--threshold", type=float, default=10.0)
    a = ap.parse_args()
    b, n = json.load(open(a.base)), json.load(open(a.new))
    bad = []
    for k in LOWER + HIGHER:
        x, y = b.get(k), n.get(k)
        if not x or y is None:
            continue
        d = (y - x) / x * 100
        worse = d > a.threshold if k in LOWER else -d > a.threshold
        print(f"{k:14} {x:>14} -> {y:>14}  {d:+7.1f}%{'  REGRESSION' if worse else ''}")
        if worse:
            bad.append(k)
    if n.get("exit_code"):
        bad.append(f"exit_code={n['exit_code']}")
    if n.get("failed_writes"):
        bad.append(f"failed_writes={n['failed_writes']}")
    print("compare: " + ("REGRESSION: " + ", ".join(bad) if bad else "OK"))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
