#!/usr/bin/env python3
"""LOAD-010 (L-807): concurrency scaling curves.

    scripts/load/curves.py <tier> [--docs N] [--engine es717-plain] [--place box] [--reps 3]
                           [--points 1,2,4,8,12,16] [--shards 1,4] [--baseline FILE]
                           [--threshold 10] [--write-baseline FILE] [--seed 1] [--run-id ID] [--keep]

Provisions one engine (scripts/it/up.sh, no CPU cap: IT_CONTAINER_CPUS = nproc), seeds `load-logs`
(edgen logs, seed 1, 1 primary shard) and, for each extra --shards value, a copy made with the
_split API and force-merged to one segment per shard. Then, per rep (points shuffled per rep):

  dump     --slices S --workers S for S in --points, scroll (default) and PIT (--searchAfter --pit),
           on each index; --limit 1000.
  restore  --concurrency C for C in --points, from a one-file dump into a fresh 1-shard index with
           the same mapping; --limit 1000 --noRefresh.

Per run: wall s, docs/s, tool CPU-s (wait4) and server CPU-s (the engine's process tree, from
/proc). Per point: median, min, max and coefficient of variation over the reps.
Checks: every run exits 0; every dump has the doc count and (first rep) an edverify digest equal
to the index; every restore holds the doc count after a refresh and (first rep) is digest-equal.
--baseline compares each point's median docs/s with a previous curves.json and fails on a drop of
more than --threshold percent (TST-LOAD-010: "regression > 10% on any point fails nightly").
Results: results/load/<run-id>/curves.json and curves.csv. Exit 0 when every check passed (and
the baseline comparison, if any), 1 otherwise, 2 refused, 3 skipped (resource guard).
On the shared 8-vCPU box the curves are trends; the authoritative 10M curves are Phase B (B10).
"""
import argparse, csv, datetime, fcntl, json, os, random, statistics, subprocess, sys, time, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run as R  # noqa: E402

ROOT = R.ROOT
LIMIT = 1000
TCK = os.sysconf("SC_CLK_TCK")


def log(*a):
    print("curves:", *a, file=sys.stderr, flush=True)


def tree_cpu_s(pid):
    """utime+stime (plus reaped children's) of a process and its live descendants, in seconds."""
    total, todo, seen = 0, [pid], set()
    while todo:
        p = todo.pop()
        if p in seen:
            continue
        seen.add(p)
        try:
            st = open(f"/proc/{p}/stat").read()
        except OSError:
            continue
        f = st[st.rindex(")") + 2:].split()
        total += sum(int(x) for x in f[11:15])  # utime stime cutime cstime
        for t in os.listdir(f"/proc/{p}/task"):
            try:
                todo += [int(c) for c in open(f"/proc/{p}/task/{t}/children").read().split()]
            except OSError:
                pass
    return total / TCK


def timed(cmd, srv_pid):
    s0, t0 = tree_cpu_s(srv_pid), time.monotonic()
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    err = p.stderr.read().decode(errors="replace")
    _, status, ru = os.wait4(p.pid, 0)
    wall = time.monotonic() - t0
    return dict(exit=os.waitstatus_to_exitcode(status), wall_s=round(wall, 3),
                cpu_s=round(ru.ru_utime + ru.ru_stime, 3), server_cpu_s=round(tree_cpu_s(srv_pid) - s0, 2),
                err=err.strip().splitlines()[-3:])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("--docs", type=int)
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "box"))
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--points", default="1,2,4,8,12,16")
    ap.add_argument("--shards", default="1,4")
    ap.add_argument("--baseline")
    ap.add_argument("--threshold", type=float, default=10.0)
    ap.add_argument("--write-baseline")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--run-id")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    points = [int(x) for x in a.points.split(",")]
    shards = [int(x) for x in a.shards.split(",")]
    rng = random.Random(a.seed)

    tiers = tomllib.load(open(os.path.join(HERE, "scenarios", "tiers.toml"), "rb"))
    scen = {"name": "curves", "seed": [{"index": "load-logs", "profile": "logs", "seed": 1, "docs": "tier"}],
            "step": [{"kind": "dump"}] * (len(shards) + 1) + [{"kind": "restore"}] * 2}
    ok, why, plan = R.fit(a.tier, a.place, tiers, scen, a.docs)
    if not ok:
        print(f"CURVES-SUMMARY tier={a.tier} place={a.place} REFUSED: {why}")
        return 2
    lock = open("/tmp/elasticdump-rs-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        fcntl.flock(lock, fcntl.LOCK_EX)
    need_mem = plan["heap_mb"] * 2 + 512 + plan["place"]["tool_mem_mb"] + 256
    if R.meminfo_avail_mb() < need_mem:
        print(f"CURVES-SUMMARY tier={a.tier} SKIPPED (MemAvailable {R.meminfo_avail_mb()} MB < {need_mem} MB)")
        return 3
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-curves-{a.tier}"
    work = os.path.join(os.environ.get("LOAD_WORK", "/var/tmp/elasticdump-rs-load"), run_id)
    os.makedirs(work, mode=0o700, exist_ok=True)
    resdir = os.path.join(ROOT, "results", "load", run_id)
    os.makedirs(resdir, exist_ok=True)
    edgen, edverify, tool = R.tool_path("edgen"), R.tool_path("edverify"), R.tool_path("elasticdump-rs")
    base = {}
    for line in subprocess.run([os.path.join(ROOT, "scripts/it/matrix.sh"), a.engine],
                               check=True, capture_output=True, text=True).stdout.splitlines():
        k, v = line.split("=", 1)
        base[k] = v
    up_profile = base.pop("UP_PROFILE")
    heap = f"{plan['heap_mb']}m"
    env = dict(os.environ, **base, IT_DIR=os.path.join(work, "it"), IT_LOCK_HELD="1",
               RUN_ID=f"load{os.getpid()}curves", ES_HEAP=heap, OS_HEAP=heap, PORT="19202",
               IT_CONTAINER_MEM=plan["place"]["container_mem"], IT_CONTAINER_CPUS=str(os.cpu_count()))
    cname = f"it-es-plain-{env['RUN_ID']}"
    res = dict(schema="edrs-curves/v1", run_id=run_id, tier=a.tier, place=a.place, engine=a.engine, heap=heap,
               seed=a.seed, reps=a.reps, limit=LIMIT, points=points, shards=shards, nproc=os.cpu_count(),
               git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                  capture_output=True, text=True).stdout.strip(),
               tool=subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.strip(),
               started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
               loadavg_start=open("/proc/loadavg").read().split()[:3], runs=[], checks=[], failures=[])
    rc = 1

    def fail(msg):
        res["failures"].append(msg)
        log("FAIL", msg)

    try:
        log(f"provisioning {a.engine} (heap {heap}, cpus {env['IT_CONTAINER_CPUS']})")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        url = R.sh_env(os.path.join(env["IT_DIR"], "env"))["IT_ES_URL"]
        cl = R.Cluster(url)
        res["engine_version"] = cl.req("GET", "/")["version"]["number"]
        srv_pid = int(subprocess.run(["podman", "inspect", "-f", "{{.State.Pid}}", cname], check=True,
                                     capture_output=True, text=True).stdout.strip())
        s = dict(plan["seeds"][0])
        log(f"seeding load-logs: {s['docs']} docs")
        res["seed_index"] = R.seed_index(cl, edgen, s, R.engine_mapping_flavour(a.engine))
        n = res["seed_index"]["stored"]
        mapping = json.loads(subprocess.run([edgen, "mapping", "--profile", "logs", "--engine",
                                             R.engine_mapping_flavour(a.engine)], check=True,
                                            capture_output=True).stdout)
        indices = {}
        for sh in shards:
            name = "load-logs" if sh == 1 else f"load-logs-{sh}sh"
            if sh != 1:
                cl.req("PUT", "/load-logs/_settings", {"index.blocks.write": True})
                cl.req("POST", f"/load-logs/_split/{name}",
                       {"settings": {"index.number_of_shards": sh, "index.number_of_replicas": 0}})
                for _ in range(600):
                    h = cl.req("GET", f"/_cluster/health/{name}?wait_for_status=green&timeout=5s")
                    if h["status"] == "green" and not h["initializing_shards"] and not h["relocating_shards"]:
                        break
                cl.req("POST", f"/{name}/_forcemerge?max_num_segments=1")
                cl.req("PUT", f"/{name}/_settings", {"index.blocks.write": None})
                cl.req("POST", f"/{name}/_refresh")
                cnt = cl.req("GET", f"/{name}/_count")["count"]
                if cnt != n:
                    raise RuntimeError(f"{name}: {cnt} docs after _split, expected {n}")
            indices[sh] = name
        cl.req("PUT", "/load-logs/_settings", {"index.blocks.write": None})
        cl.req("POST", "/load-logs/_forcemerge?max_num_segments=1")
        res["indices"] = {str(k): v for k, v in indices.items()}

        def digest(tag, src_index, b):
            rep = os.path.join(resdir, f"verify-{tag}.json")
            p = subprocess.run([edverify, "diff", "--a", f"{url}/{src_index}", "--b", *b, "--index-as", "x",
                                "--out", rep], capture_output=True, text=True)
            return p.returncode == 0

        ref = os.path.join(work, "ref.ndjson")
        r = timed([tool, "--input", f"{url}/load-logs", "--output", ref, "--type=data", f"--limit={LIMIT}"], srv_pid)
        if r["exit"] or not digest("ref", "load-logs", [ref]):
            raise RuntimeError(f"reference dump failed: {r['err']}")
        raw = os.path.getsize(ref)
        res["raw_bytes"] = raw
        log(f"reference: {raw / 1e6:.0f} MB, {r['wall_s']} s")

        def count_lines(path):
            c = 0
            with open(path, "rb") as f:
                for b in iter(lambda: f.read(1 << 22), b""):
                    c += b.count(b"\n")
            return c

        cells = []
        for sh in shards:
            for mode in ("scroll", "pit"):
                for p_ in points:
                    cells.append(("dump", mode, sh, p_))
        for p_ in points:
            cells.append(("restore", "bulk", 1, p_))
        out = os.path.join(work, "out.ndjson")
        for rep in range(a.reps):
            order = cells[:]
            rng.shuffle(order)
            for kind, mode, sh, p_ in order:
                tag = f"{kind}-{mode}-{sh}sh-{p_}"
                if kind == "dump":
                    flags = [f"--slices={p_}", f"--workers={p_}"] + (["--searchAfter", "--pit"] if mode == "pit" else [])
                    cmd = [tool, "--input", f"{url}/{indices[sh]}", "--output", out, "--type=data",
                           f"--limit={LIMIT}", "--overwrite", *flags]
                else:
                    cl.req("DELETE", "/dst", ok404=True)
                    cl.req("PUT", "/dst", mapping)
                    flags = [f"--concurrency={p_}", "--noRefresh"]
                    cmd = [tool, "--input", ref, "--output", f"{url}/dst", "--type=data", f"--limit={LIMIT}", *flags]
                subprocess.run(["sync"])
                r = timed(cmd, srv_pid)
                row = dict(rep=rep, kind=kind, mode=mode, shards=sh, point=p_, flags=flags,
                           **{k: r[k] for k in ("exit", "wall_s", "cpu_s", "server_cpu_s")})
                row["docs_per_s"] = round(n / r["wall_s"]) if r["wall_s"] else None
                if r["exit"]:
                    row["err"] = r["err"]
                    fail(f"{tag} rep{rep}: exit {r['exit']}: {r['err'][-1:]}")
                else:
                    if kind == "dump":
                        got = count_lines(out)
                        row["count_ok"] = got == n
                        if rep == 0:
                            row["digest_equal"] = digest(tag, indices[sh], [out])
                    else:
                        cl.req("POST", "/dst/_refresh")
                        got = cl.req("GET", "/dst/_count")["count"]
                        row["count_ok"] = got == n
                        if rep == 0:
                            row["digest_equal"] = digest(tag, "dst", [ref])
                    if not row["count_ok"] or row.get("digest_equal") is False:
                        fail(f"{tag} rep{rep}: count {got} of {n}, digest {row.get('digest_equal')}")
                res["runs"].append(row)
                log(f"rep{rep} {tag}: exit {r['exit']} {r['wall_s']} s, {row['docs_per_s']} docs/s, "
                    f"{r['cpu_s']} CPU-s, server {r['server_cpu_s']} CPU-s")

        # per-point summary
        summ = []
        for kind, mode, sh, p_ in cells:
            rs = [x for x in res["runs"] if (x["kind"], x["mode"], x["shards"], x["point"]) == (kind, mode, sh, p_)
                  and not x["exit"]]
            if not rs:
                continue
            dps = [x["docs_per_s"] for x in rs]
            med = statistics.median(dps)
            summ.append(dict(kind=kind, mode=mode, shards=sh, point=p_, n=len(rs), docs_per_s=round(med),
                             docs_per_s_min=min(dps), docs_per_s_max=max(dps),
                             cv_pct=round(statistics.pstdev(dps) / statistics.mean(dps) * 100, 1),
                             wall_s=statistics.median(x["wall_s"] for x in rs),
                             cpu_s=statistics.median(x["cpu_s"] for x in rs),
                             server_cpu_s=statistics.median(x["server_cpu_s"] for x in rs)))
        for row in summ:
            one = next((x for x in summ if (x["kind"], x["mode"], x["shards"], x["point"]) ==
                        (row["kind"], row["mode"], row["shards"], points[0])), None)
            row["speedup_vs_first_point"] = round(row["docs_per_s"] / one["docs_per_s"], 2) if one else None
        res["summary"] = summ
        with open(os.path.join(resdir, "curves.csv"), "w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=list(summ[0].keys()))
            w.writeheader()
            w.writerows(summ)

        if a.baseline:
            b = json.load(open(a.baseline))
            bmap = {(x["kind"], x["mode"], x["shards"], x["point"]): x for x in b["summary"]}
            cmp_ = []
            for row in summ:
                k = (row["kind"], row["mode"], row["shards"], row["point"])
                if k not in bmap:
                    continue
                d = (row["docs_per_s"] - bmap[k]["docs_per_s"]) / bmap[k]["docs_per_s"] * 100
                reg = -d > a.threshold
                cmp_.append(dict(kind=k[0], mode=k[1], shards=k[2], point=k[3], base=bmap[k]["docs_per_s"],
                                 new=row["docs_per_s"], delta_pct=round(d, 1), regression=reg))
                if reg:
                    fail(f"regression {k}: {bmap[k]['docs_per_s']} -> {row['docs_per_s']} docs/s ({d:+.1f}%)")
            res["baseline"] = dict(file=os.path.relpath(a.baseline, ROOT), threshold_pct=a.threshold, points=cmp_)
        if a.write_baseline:
            json.dump(dict(schema="edrs-curves-baseline/v1", source_run=run_id, tier=a.tier, engine=a.engine,
                           docs=n, limit=LIMIT, git=res["git"], summary=summ),
                      open(a.write_baseline, "w"), indent=1)
        res["leak"], f = R.leak_check(cl, work)  # LOAD-017
        for x in f:
            fail(x)
        rc = 0 if not res["failures"] else 1
    except Exception as e:
        res["failures"].append(f"error: {e!r}")
        log("ERROR", repr(e))
        rc = 1
    finally:
        res["finished"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
        res["loadavg_end"] = open("/proc/loadavg").read().split()[:3]
        res["pass"] = rc == 0
        json.dump(res, open(os.path.join(resdir, "curves.json"), "w"), indent=1)
        if not a.keep:
            subprocess.run(["podman", "rm", "-f", cname], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["podman", "unshare", "rm", "-rf", work])
    print(f"CURVES-SUMMARY tier={a.tier} engine={a.engine} docs={res.get('seed_index', {}).get('stored')} "
          f"{'PASS' if rc == 0 else 'FAIL'} runs={len(res['runs'])} result=results/load/{run_id}/curves.json")
    for f in res["failures"]:
        print("  failure:", f)
    return rc


if __name__ == "__main__":
    sys.exit(main())
