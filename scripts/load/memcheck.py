#!/usr/bin/env python3
"""LOAD-005 (L-802): memory is bounded and flat, and the README memory formula holds.

  memcheck.py matrix SMALL LARGE [--flat-pct 10] [--formula-pct 25] [--out F]
      SMALL/LARGE: two matrix.json files (or results/load/<run-id> dirs) from
      scripts/load/matrix.sh with the same flags at two sizes (e.g. L0 100k and
      L1L 1M). Per cell: flat = peak RSS at LARGE is at most flat-pct above SMALL;
      formula = each peak RSS is at most formula-pct above the README estimate.
      Flags come from the scenario TOML step; avg doc size from edgen (same
      profile and seed as the scenario's seed index). --formula old checks the
      pre-L-802 README formula (x1.5 + 10 MB) instead of the current one.

  memcheck.py files [--only a,b] [--run-id ID] [--flat-pct] [--formula-pct]
      Cluster-free file-to-file cases from scenarios/memcheck.toml (doc sizes
      ~300 B, 1 KB, 40 KB, 1 MB), each at a small and a large document count.
      Data is generated under LOAD_WORK (default results/load/<run-id>/work)
      and deleted afterwards.

  memcheck.py plateau [--samples OUT | --from FILE] [--flat-pct 10] [--settle-pct 50] -- CMD...
      One long run: samples the command's VmRSS every 0.1 s (or reads "t rss_kb"
      lines from FILE) and checks that the peak after the first settle-pct of the
      run is at most flat-pct above the peak before it: bounded growth, no leak.
      Use it for 10M-doc runs, where peak-vs-peak across sizes mixes in the rare
      high-water marks a longer run is more likely to hit.

Peak RSS is the child's getrusage max RSS (one process per run). Exit 0 when
every check holds, 1 when one fails, 2 on a usage error. Results go to
results/load/<run-id>/memcheck-*.json.
"""
import argparse, datetime, json, os, re, resource, shutil, subprocess, sys, time, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
MB = 1024 * 1024
DEFAULTS = {"limit": 100, "concurrency": 1, "slices": 0, "bufferSize": 16}


def tool(name):
    for prof in ("release", "it", "debug"):
        p = os.path.join(ROOT, "target", prof, name)
        if os.path.exists(p):
            return p
    sys.exit(f"memcheck: {name} not built (cargo build --release -p {name})")


def flags(args):
    f = dict(DEFAULTS)
    for a in args:
        m = re.match(r"--(limit|concurrency|slices|bufferSize)=(\d+)$", a)
        if m:
            f[m.group(1)] = int(m.group(2))
    return f


# README "Memory" formula: (bufferSize + slices + concurrency) x limit x avg_doc x FACTOR + BASE MB.
# L-802 measured 1.5 / 10 MB as too low (concurrent restore and sliced copy up to +127%,
# idle RSS ~14 MB) and changed it to 3 / 15 MB (lead review); --formula old checks the old one.
FORMULAS = {"readme": (3.0, 15.0), "old": (1.5, 10.0)}
FORMULA = FORMULAS["readme"]


def estimate_mb(f, avg_doc):
    factor, base = FORMULA
    return (f["bufferSize"] + f["slices"] + f["concurrency"]) * f["limit"] * avg_doc * factor / MB + base


def avg_doc(profile, seed, extra=(), count=20000):
    """Mean elasticdump-format line size of an edgen profile (bytes)."""
    out = subprocess.run([tool("edgen"), "gen", "--profile", profile, "--seed", str(seed),
                          "--count", str(count), *extra], capture_output=True, check=True).stdout
    lines = out.count(b"\n") or 1
    return len(out) / lines


def check(name, small_mb, large_mb, est, fp, ep):
    flat = (large_mb / small_mb - 1) * 100 if small_mb else 0.0
    over = (max(small_mb, large_mb) / est - 1) * 100
    return {"cell": name, "small_rss_mb": round(small_mb, 1), "large_rss_mb": round(large_mb, 1),
            "flat_pct": round(flat, 1), "flat_ok": flat <= fp,
            "estimate_mb": round(est, 1), "over_estimate_pct": round(over, 1), "formula_ok": over <= ep}


def report(rows, fp, ep):
    print(f"{'cell':34} {'small':>8} {'large':>8} {'flat%':>7} {'est MB':>8} {'vs est':>7}")
    for r in rows:
        print(f"{r['cell']:34} {r['small_rss_mb']:8.1f} {r['large_rss_mb']:8.1f} {r['flat_pct']:+7.1f}"
              f"{'' if r['flat_ok'] else '!'} {r['estimate_mb']:8.1f} {r['over_estimate_pct']:+7.1f}"
              f"{'' if r['formula_ok'] else '!'}")
    nf = sum(not r["flat_ok"] for r in rows)
    ne = sum(not r["formula_ok"] for r in rows)
    print(f"MEMCHECK cells={len(rows)} not_flat={nf} (> {fp}%) formula_exceeded={ne} (> +{ep}%)")
    return 0 if nf == 0 and ne == 0 else 1


def load_matrix(p):
    if os.path.isdir(p):
        p = os.path.join(p, "matrix.json")
    return json.load(open(p))


def cmd_matrix(a):
    s, l = load_matrix(a.small), load_matrix(a.large)
    if (s["place"], s["engine"]) != (l["place"], l["engine"]):
        print(f"memcheck: warning: comparing place/engine {s['place']}/{s['engine']} with {l['place']}/{l['engine']}")
    rows, avg = [], {}
    for key in sorted(set(s["cells"]) & set(l["cells"])):
        sc_name, cell = key.split("/", 1)
        sc = tomllib.load(open(os.path.join(HERE, "scenarios", sc_name + ".toml"), "rb"))
        step = next(st for st in sc["step"] if st.get("name", st["kind"]) == cell)
        seed = sc["seed"][0]
        k = (seed["profile"], seed["seed"])
        if k not in avg:
            avg[k] = avg_doc(*k)
        est = estimate_mb(flags(step.get("args", [])), avg[k])
        r = check(key, s["cells"][key]["max_rss_kb"] / 1024, l["cells"][key]["max_rss_kb"] / 1024,
                  est, a.flat_pct, a.formula_pct)
        r.update(small_docs=s["cells"][key].get("docs"), large_docs=l["cells"][key].get("docs"),
                 avg_doc_bytes=round(avg[k]), flags=flags(step.get("args", [])))
        rows.append(r)
    rc = report(rows, a.flat_pct, a.formula_pct)
    # Next to the large run's matrix.json, or under results/load/ when the large side is a
    # checked-in baseline (never into scripts/load/baselines/).
    large_dir = os.path.dirname(os.path.abspath(a.large)) if a.large.endswith(".json") else a.large
    if os.path.abspath(large_dir).startswith(os.path.join(ROOT, "scripts")):
        large_dir = os.path.join(ROOT, "results", "load")
        os.makedirs(large_dir, exist_ok=True)
    out = a.out or os.path.join(large_dir, "memcheck-matrix.json")
    json.dump({"schema": "edrs-memcheck/v1", "mode": "matrix", "small": a.small, "large": a.large,
               "formula": a.formula, "factor": FORMULA[0], "base_mb": FORMULA[1],
               "flat_pct": a.flat_pct, "formula_pct": a.formula_pct, "rows": rows}, open(out, "w"), indent=1)
    print(f"memcheck: wrote {out}")
    return rc


def run_peak_mb(cmd):
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    _, status, ru = os.wait4(p.pid, 0)
    err = p.stderr.read().decode(errors="replace")
    if os.waitstatus_to_exitcode(status) != 0:
        raise RuntimeError(f"{cmd[0]} failed: {err[-400:]}")
    return ru.ru_maxrss / 1024


def cmd_files(a):
    cases = tomllib.load(open(os.path.join(HERE, "scenarios", "memcheck.toml"), "rb"))["case"]
    if a.only:
        want = set(a.only.split(","))
        cases = [c for c in cases if c["name"] in want]
        if not cases:
            print("memcheck: --only matched no case", file=sys.stderr)
            return 2
    rid = a.run_id or "memcheck-" + datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    res = os.path.join(ROOT, "results", "load", rid)
    work = os.path.join(os.environ.get("LOAD_WORK") or os.path.join(res, "work"), "memcheck")
    os.makedirs(res, exist_ok=True)
    os.makedirs(work, exist_ok=True)
    edgen, ed = tool("edgen"), tool("searchdump")
    rows = []
    try:
        for c in cases:
            extra = c.get("edgen_args", [])
            big = os.path.join(work, f"{c['name']}-large.json")
            small = os.path.join(work, f"{c['name']}-small.json")
            subprocess.run([edgen, "gen", "--profile", c["profile"], "--seed", str(c.get("seed", 1)),
                            "--count", str(c["large"]), "--out", big, *extra], check=True,
                           stderr=subprocess.DEVNULL)
            with open(big, "rb") as fi, open(small, "wb") as fo:
                for i, line in enumerate(fi):
                    if i >= c["small"]:
                        break
                    fo.write(line)
            avg = os.path.getsize(big) / c["large"]
            for args in c["args"]:
                peaks = {}
                for which, f in (("small", small), ("large", big)):
                    out = os.path.join(work, "out.json")
                    t = time.time()
                    peaks[which] = run_peak_mb([ed, f"--input={f}", f"--output={out}", "--quiet", *args])
                    os.remove(out)
                r = check(f"{c['name']} {' '.join(args)}", peaks["small"], peaks["large"],
                          estimate_mb(flags(args), avg), a.flat_pct, a.formula_pct)
                r.update(small_docs=c["small"], large_docs=c["large"], avg_doc_bytes=round(avg),
                         flags=flags(args))
                rows.append(r)
            os.remove(big)
            os.remove(small)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    rc = report(rows, a.flat_pct, a.formula_pct)
    out = os.path.join(res, "memcheck-files.json")
    json.dump({"schema": "edrs-memcheck/v1", "mode": "files", "run_id": rid,
               "formula": a.formula, "factor": FORMULA[0], "base_mb": FORMULA[1],
               "malloc_arena_max": os.environ.get("MALLOC_ARENA_MAX"),
               "flat_pct": a.flat_pct, "formula_pct": a.formula_pct, "rows": rows}, open(out, "w"), indent=1)
    print(f"memcheck: wrote {out}")
    return rc


def sample_run(cmd, out):
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    s, t0 = [], time.time()
    while p.poll() is None:
        try:
            with open(f"/proc/{p.pid}/status") as fh:
                for line in fh:
                    if line.startswith("VmRSS:"):
                        s.append((time.time() - t0, int(line.split()[1])))
        except (FileNotFoundError, ProcessLookupError):
            pass
        time.sleep(0.1)
    err = p.stderr.read().decode(errors="replace")
    if out:
        with open(out, "w") as fh:
            fh.write("".join(f"{t:.2f} {r}\n" for t, r in s))
    if p.returncode != 0:
        raise RuntimeError(f"{cmd[0]} failed (exit {p.returncode}): {err[-400:]}")
    return s


def cmd_plateau(a):
    if a.source:
        s = [(float(t), int(r)) for t, r in (ln.split() for ln in open(a.source) if ln.strip())]
    else:
        cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd
        if not cmd:
            print("memcheck: plateau needs --from FILE or -- CMD", file=sys.stderr)
            return 2
        s = sample_run(cmd, a.samples)
    if len(s) < 10:
        print("memcheck: too few samples", file=sys.stderr)
        return 2
    end = s[-1][0]
    cut = end * a.settle_pct / 100
    early = max(r for t, r in s if t <= cut) / 1024
    late = max((r for t, r in s if t > cut), default=0) / 1024
    growth = (late / early - 1) * 100
    print(f"{'part':>8} {'peak MB':>8}")
    for i in range(10):
        seg = [r for t, r in s if i * end / 10 <= t < (i + 1) * end / 10 or (i == 9 and t == end)]
        print(f"{(i + 1) * 10:>7}% " + (f"{max(seg) / 1024:8.1f}" if seg else f"{'-':>8}  (no samples)"))
    ok = growth <= a.flat_pct
    print(f"MEMCHECK plateau samples={len(s)} wall={end:.1f}s peak_first_{a.settle_pct:g}%={early:.1f}MB "
          f"peak_rest={late:.1f}MB growth={growth:+.1f}%{'' if ok else '!'} (<= {a.flat_pct}%)")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="mode", required=True)
    m = sub.add_parser("matrix")
    m.add_argument("small")
    m.add_argument("large")
    m.add_argument("--out")
    f = sub.add_parser("files")
    f.add_argument("--only")
    f.add_argument("--run-id")
    for p in (m, f):
        p.add_argument("--formula", choices=sorted(FORMULAS), default="readme")
        p.add_argument("--flat-pct", type=float, default=10.0)
        p.add_argument("--formula-pct", type=float, default=25.0)
    q = sub.add_parser("plateau")
    q.add_argument("--samples", help="write the samples here")
    q.add_argument("--from", dest="source", help="read samples instead of running a command")
    q.add_argument("--flat-pct", type=float, default=10.0)
    q.add_argument("--settle-pct", type=float, default=50.0)
    q.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    if a.mode == "plateau":
        return cmd_plateau(a)
    global FORMULA
    FORMULA = FORMULAS[a.formula]
    return cmd_matrix(a) if a.mode == "matrix" else cmd_files(a)


if __name__ == "__main__":
    sys.exit(main())
