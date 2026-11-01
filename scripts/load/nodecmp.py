#!/usr/bin/env python3
"""LOAD-011 (L-808): Rust vs Node elasticdump at load scale.

    scripts/load/nodecmp.py <tier> [--docs N] [--engine es717-plain] [--place box] [--reps 3]
                            [--node /tmp/ed/node_modules/.bin/elasticdump] [--scenarios a,b,...]
                            [--seed 1] [--run-id ID] [--keep]

Runs the README Performance scenarios (scripts/benchmark/quick-vs-node.py) and the PLAN.md §8
targets at load scale. It provisions one engine (no CPU cap), seeds `load-logs` (edgen logs, seed 1),
and makes a 2-shard copy `load-logs-2sh` with _split (force-merged, like quick-vs-node's 2-shard
`bsrc`). That copy is the source for every scenario. Both tools get the same flags:

  dump-defaults   --limit 100 (both tools' default)               §8: >= 1.8x Node docs/s
  dump-5000       --limit 5000
  dump-5000-gzip  --limit 5000 --fsCompress
  dump-sliced     Node --limit 5000 vs Rust --limit 5000 with --slices 2 (scroll, slices = shards) and
                  --searchAfter --pit --slices 8 (Node has no slicing)  §8: >= 3x Node
  restore         file -> fresh 2-shard index, --limit 5000 --concurrency 4 --noRefresh   §8: >= 3x
  copy            index -> fresh 2-shard index, --limit 5000 --concurrency 4 --noRefresh  §8: >= 2.5x
  CPU             §8: >= 3x fewer CPU-seconds than Node for the same work

Each rep runs the scenarios in shuffled order, and the two tools in shuffled order within each
scenario. Each run records wall s, docs/s, CPU-s and max RSS (wait4). Checks: every run exits 0;
on the first rep, every dump output (Node and Rust, gzip decoded) is digest-equal to the index and
to each other, and every restore and copy target is digest-equal to the source after a refresh.
Results: results/load/<run-id>/nodecmp.json. Exit 0 when every check passed, 1 otherwise,
2 refused, 3 skipped. The ratios against §8 and the README are reported, not asserted (the
box is shared). Node at 10M on a quiet machine is Phase B.
Upstream Node code is untrusted: this script only runs the installed CLI and never runs npm.
"""
import argparse, datetime, fcntl, json, os, random, statistics, subprocess, sys, time, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run as R  # noqa: E402

ROOT = R.ROOT
SCEN = ["dump-defaults", "dump-5000", "dump-5000-gzip", "dump-sliced", "restore", "copy"]


def log(*a):
    print("nodecmp:", *a, file=sys.stderr, flush=True)


def timed(cmd):
    t0 = time.monotonic()
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    err = p.stderr.read().decode(errors="replace")
    _, status, ru = os.wait4(p.pid, 0)
    return dict(exit=os.waitstatus_to_exitcode(status), wall_s=round(time.monotonic() - t0, 3),
                cpu_s=round(ru.ru_utime + ru.ru_stime, 3), maxrss_mb=round(ru.ru_maxrss / 1024, 1),
                err=err.strip().splitlines()[-3:])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("--docs", type=int)
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "box"))
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--node", default=os.environ.get("NODE_ED", "/tmp/ed/node_modules/.bin/elasticdump"))
    ap.add_argument("--scenarios", default=",".join(SCEN))
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--run-id")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    scen = a.scenarios.split(",")
    rng = random.Random(a.seed)
    if not os.access(a.node, os.X_OK):
        print(f"NODECMP-SUMMARY tier={a.tier} SKIPPED (no Node elasticdump at {a.node})")
        return 3

    tiers = tomllib.load(open(os.path.join(HERE, "scenarios", "tiers.toml"), "rb"))
    sc = {"name": "nodecmp", "seed": [{"index": "load-logs", "profile": "logs", "seed": 1, "docs": "tier"}],
          "step": [{"kind": "dump"}] * 3 + [{"kind": "restore"}] * 2}
    ok, why, plan = R.fit(a.tier, a.place, tiers, sc, a.docs)
    if not ok:
        print(f"NODECMP-SUMMARY tier={a.tier} place={a.place} REFUSED: {why}")
        return 2
    lock = open("/tmp/elasticdump-rs-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        fcntl.flock(lock, fcntl.LOCK_EX)
    need_mem = plan["heap_mb"] * 2 + 512 + plan["place"]["tool_mem_mb"] + 256
    if R.meminfo_avail_mb() < need_mem:
        print(f"NODECMP-SUMMARY tier={a.tier} SKIPPED (MemAvailable {R.meminfo_avail_mb()} MB < {need_mem} MB)")
        return 3
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-nodecmp-{a.tier}"
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
               RUN_ID=f"load{os.getpid()}node", ES_HEAP=heap, OS_HEAP=heap, PORT="19202",
               IT_CONTAINER_MEM=plan["place"]["container_mem"], IT_CONTAINER_CPUS=str(os.cpu_count()))
    cname = f"it-es-plain-{env['RUN_ID']}"
    node_ver = subprocess.run([a.node, "--version"], capture_output=True, text=True).stdout.strip().splitlines()
    res = dict(schema="edrs-nodecmp/v1", run_id=run_id, tier=a.tier, place=a.place, engine=a.engine, heap=heap,
               seed=a.seed, reps=a.reps, scenarios=scen, nproc=os.cpu_count(),
               git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                  capture_output=True, text=True).stdout.strip(),
               tool=subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.strip(),
               node_elasticdump=node_ver[-1] if node_ver else None,
               node_runtime=subprocess.run(["node", "--version"], capture_output=True, text=True).stdout.strip(),
               started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
               loadavg_start=open("/proc/loadavg").read().split()[:3], runs=[], checks=[], failures=[])
    rc = 1

    def fail(msg):
        res["failures"].append(msg)
        log("FAIL", msg)

    try:
        log(f"provisioning {a.engine} (heap {heap}); node elasticdump {res['node_elasticdump']} "
            f"on node {res['node_runtime']}")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        url = R.sh_env(os.path.join(env["IT_DIR"], "env"))["IT_ES_URL"]
        cl = R.Cluster(url)
        res["engine_version"] = cl.req("GET", "/")["version"]["number"]
        s = dict(plan["seeds"][0])
        log(f"seeding load-logs: {s['docs']} docs")
        res["seed_index"] = R.seed_index(cl, edgen, s, R.engine_mapping_flavour(a.engine))
        n = res["seed_index"]["stored"]
        mapping = json.loads(subprocess.run([edgen, "mapping", "--profile", "logs", "--engine",
                                             R.engine_mapping_flavour(a.engine)], check=True,
                                            capture_output=True).stdout)
        mapping["settings"]["index"]["number_of_shards"] = 2
        src = "load-logs-2sh"
        cl.req("PUT", "/load-logs/_settings", {"index.blocks.write": True})
        cl.req("POST", f"/load-logs/_split/{src}",
               {"settings": {"index.number_of_shards": 2, "index.number_of_replicas": 0}})
        for _ in range(600):
            h = cl.req("GET", f"/_cluster/health/{src}?wait_for_status=green&timeout=5s")
            if h["status"] == "green" and not h["initializing_shards"] and not h["relocating_shards"]:
                break
        cl.req("PUT", f"/{src}/_settings", {"index.blocks.write": None})
        cl.req("POST", f"/{src}/_forcemerge?max_num_segments=1")
        cl.req("POST", f"/{src}/_refresh")
        cl.req("DELETE", "/load-logs")
        if cl.req("GET", f"/{src}/_count")["count"] != n:
            raise RuntimeError(f"{src}: count after _split is not {n}")
        surl = f"{url}/{src}"

        def digest(tag, a_, b_):
            rep = os.path.join(resdir, f"verify-{tag}.json")
            p = subprocess.run([edverify, "diff", "--a", *a_, "--b", *b_, "--index-as", "x", "--out", rep],
                               capture_output=True, text=True)
            return p.returncode == 0

        ref = os.path.join(work, "ref.ndjson")
        r = timed([tool, "--input", surl, "--output", ref, "--type=data", "--limit=5000"])
        if r["exit"] or not digest("ref", [surl], [ref]):
            raise RuntimeError(f"reference dump failed: {r['err']}")
        raw = os.path.getsize(ref)
        res["raw_bytes"] = raw
        log(f"reference: {raw / 1e6:.0f} MB")

        def open_scrolls():
            st = cl.req("GET", "/_nodes/stats/indices/search")["nodes"]
            return sum(v["indices"]["search"].get("scroll_current", 0) for v in st.values())

        def fresh_dst():
            cl.req("DELETE", "/dst", ok404=True)
            cl.req("PUT", "/dst", mapping)

        def cmd_for(sc_, tl, variant):
            exe = [a.node] if tl == "node" else [tool, "--quiet"]
            if sc_.startswith("dump"):
                out = os.path.join(work, f"out-{sc_}-{tl}{variant}.json")
                flags = {"dump-defaults": [], "dump-5000": ["--limit", "5000"],
                         "dump-5000-gzip": ["--limit", "5000", "--fsCompress"],
                         "dump-sliced": ["--limit", "5000"] + {"": [], "-scroll2": ["--slices", "2"],
                                                               "-pit8": ["--searchAfter", "--pit", "--slices", "8"]
                                                               }[variant]}[sc_]
                return exe + ["--input", surl, "--output", out] + flags, flags, out
            flags = ["--limit", "5000", "--concurrency", "4", "--noRefresh"]
            inp = ref if sc_ == "restore" else surl
            return exe + ["--input", inp, "--output", f"{url}/dst"] + flags, flags, None

        cells = []
        for sc_ in scen:
            variants = [("node", "")] + ([("rust", "-scroll2"), ("rust", "-pit8")] if sc_ == "dump-sliced"
                                          else [("rust", "")])
            cells.append((sc_, variants))
        outs = {}
        for rep in range(a.reps):
            order = cells[:]
            rng.shuffle(order)
            for sc_, variants in order:
                vs = variants[:]
                rng.shuffle(vs)
                for tl, variant in vs:
                    cmd, flags, out = cmd_for(sc_, tl, variant)
                    if out:
                        for f in (out, out + ".gz"):
                            if os.path.exists(f):
                                os.remove(f)
                    else:
                        fresh_dst()
                    subprocess.run(["sync"])
                    open0 = open_scrolls()
                    r = timed(cmd)
                    row = dict(rep=rep, scenario=sc_, tool=tl + variant, flags=flags,
                               **{k: r[k] for k in ("exit", "wall_s", "cpu_s", "maxrss_mb")})
                    row["docs_per_s"] = round(n / r["wall_s"]) if r["wall_s"] else None
                    tag = f"{sc_}-{tl}{variant}"
                    if r["exit"]:
                        row["err"] = r["err"]
                        fail(f"{tag} rep{rep}: exit {r['exit']}: {r['err'][-1:]}")
                    elif out:
                        path = out if os.path.exists(out) else out + ".gz"
                        with open(path, "rb") as f:
                            gz = f.read(2) == b"\x1f\x8b"
                        if gz and not path.endswith(".gz"):  # name it so edverify decodes it
                            os.rename(path, path + ".gz")
                            path += ".gz"
                        row["gzip"] = gz
                        row["bytes"] = os.path.getsize(path)
                        if rep == 0:
                            row["digest_equal_index"] = digest(tag, [surl], [path])
                            outs[(sc_, tl + variant)] = path
                            if not row["digest_equal_index"]:
                                fail(f"{tag}: output not digest-equal to the index")
                        else:
                            os.remove(path)
                    else:
                        cl.req("POST", "/dst/_refresh")
                        cnt = cl.req("GET", "/dst/_count")["count"]
                        row["count_ok"] = cnt == n
                        if rep == 0:
                            row["digest_equal_source"] = digest(tag, [surl], [f"{url}/dst"])
                        if not row["count_ok"] or row.get("digest_equal_source") is False:
                            fail(f"{tag} rep{rep}: dst count {cnt} of {n}, digest {row.get('digest_equal_source')}")
                    # scroll contexts this run left open on the engine (Node does not clear them)
                    row["scrolls_left_open"] = open_scrolls() - open0
                    if tl == "rust" and row["scrolls_left_open"] > 0:
                        fail(f"{tag} rep{rep}: left {row['scrolls_left_open']} scroll contexts open")
                    res["runs"].append(row)
                    log(f"rep{rep} {tag}: exit {r['exit']} {r['wall_s']} s, {row['docs_per_s']} docs/s, "
                        f"{r['cpu_s']} CPU-s, {r['maxrss_mb']} MB")
            if rep == 0:
                # Node output and Rust output for the same scenario digest-equal to each other
                for sc_ in scen:
                    if not sc_.startswith("dump"):
                        continue
                    nd = outs.get((sc_, "node"))
                    for (s2, t2), p2 in outs.items():
                        if s2 == sc_ and t2 != "node" and nd:
                            eq = digest(f"{sc_}-node-vs-{t2}", [nd], [p2])
                            res["checks"].append(dict(check=f"{sc_}: node output == {t2} output", ok=eq))
                            if not eq:
                                fail(f"{sc_}: node and {t2} outputs differ")
                for p in list(outs.values()):
                    os.remove(p)

        summ = []
        for sc_, variants in cells:
            for tl, variant in variants:
                rs = [x for x in res["runs"] if x["scenario"] == sc_ and x["tool"] == tl + variant and not x["exit"]]
                if not rs:
                    continue
                w = [x["wall_s"] for x in rs]
                summ.append(dict(scenario=sc_, tool=tl + variant, n=len(rs), wall_s=statistics.median(w),
                                 wall_min=min(w), wall_max=max(w),
                                 cv_pct=round(statistics.pstdev(w) / statistics.mean(w) * 100, 1),
                                 docs_per_s=round(n / statistics.median(w)),
                                 cpu_s=statistics.median(x["cpu_s"] for x in rs),
                                 maxrss_mb=max(x["maxrss_mb"] for x in rs)))
        for row in summ:
            nd = next((x for x in summ if x["scenario"] == row["scenario"] and x["tool"] == "node"), None)
            if nd and row["tool"] != "node":
                row["speedup_vs_node"] = round(nd["wall_s"] / row["wall_s"], 2)
                row["cpu_ratio_node_over_rust"] = round(nd["cpu_s"] / row["cpu_s"], 2) if row["cpu_s"] else None
        res["summary"] = summ
        # LOAD-017: Node elasticdump may leave scrolls open, so contexts are recorded, not asserted;
        # the Rust runs are held to zero through the per-run scrolls_left_open count below.
        res["leak"], f = R.leak_check(cl, work, contexts=False)
        res["failures"] += f
        res["leak"]["by_tool"] = {t: sum(x.get("scrolls_left_open", 0) for x in res["runs"] if x["tool"].startswith(t))
                                  for t in ("node", "rust")}
        if res["leak"]["by_tool"]["rust"]:
            res["failures"].append(f"leak: rust runs left {res['leak']['by_tool']['rust']} scroll(s) open")
        rc = 0 if not res["failures"] else 1
    except Exception as e:
        res["failures"].append(f"error: {e!r}")
        log("ERROR", repr(e))
        rc = 1
    finally:
        res["finished"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
        res["loadavg_end"] = open("/proc/loadavg").read().split()[:3]
        res["pass"] = rc == 0
        json.dump(res, open(os.path.join(resdir, "nodecmp.json"), "w"), indent=1)
        if not a.keep:
            subprocess.run(["podman", "rm", "-f", cname], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["podman", "unshare", "rm", "-rf", work])
    print(f"NODECMP-SUMMARY tier={a.tier} engine={a.engine} docs={res.get('seed_index', {}).get('stored')} "
          f"{'PASS' if rc == 0 else 'FAIL'} runs={len(res['runs'])} result=results/load/{run_id}/nodecmp.json")
    for f in res["failures"]:
        print("  failure:", f)
    return rc


if __name__ == "__main__":
    sys.exit(main())
