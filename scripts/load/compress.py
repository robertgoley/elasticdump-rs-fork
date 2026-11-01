#!/usr/bin/env python3
"""LOAD-008 (L-805): compression levels at scale.

    scripts/load/compress.py <tier> [--docs N] [--engine es717-plain] [--place box] [--reps 3]
                             [--levels gzip:1,gzip:6,gzip:9,zstd:1,zstd:3,zstd:9,zstd:19]
                             [--part-size 64mb] [--slow-reps 1] [--seed 1] [--run-id ID] [--keep]

Provisions one engine (scripts/it/up.sh), seeds `load-logs` (edgen logs, seed 1) and takes an
uncompressed reference dump. Then, per rep (codec order shuffled per rep, a `none` baseline in
every rep; zstd levels >= 19 run --slow-reps times, as they compress at ~2 MB/s per core):

  f2f   file -> file: --input ref.ndjson --output out --compressionType T --compressionLevel L.
        No cluster in the loop, so CPU-s minus the `none` baseline's CPU-s is the codec's cost:
        `MB/s per core` = uncompressed MB / (CPU-s - baseline CPU-s).
  dump  cluster -> file with the same codec (--limit 1000, one slice): wall, CPU-s, size.

Checks (first rep): every f2f and dump output decodes with an independent decoder (gzip(1) for
gzip, the Python zstandard module for zstd) to the same bytes as the uncompressed output; the
tool reads each compressed file back (auto-detect) to the same bytes; dump outputs are
digest-equal to the index (edverify on the decoded file). Parts (--fileSize, gzip:6 and zstd:3):
`cat` of the parts decodes, and the tool reads the cat'ed file, to the same bytes; each part
also decodes on its own.
Results: results/load/<run-id>/compress.json. Exit 0 when every check passed, 1 otherwise,
2 refused, 3 skipped (resource guard). Multi-GB single files at 10M are Phase B.
"""
import argparse, datetime, fcntl, glob, hashlib, json, os, random, shutil, statistics, subprocess, sys, \
    time, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run as R  # noqa: E402

ROOT = R.ROOT
LIMIT = 1000
ZPY = os.environ.get("ZSTD_PYTHON", "/workspace/tools/zvenv/bin/python")
EXT = {"gzip": ".gz", "zstd": ".zst", "none": ""}


def log(*a):
    print("compress:", *a, file=sys.stderr, flush=True)


def timed(cmd, env=None):
    t0 = time.monotonic()
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env)
    err = p.stderr.read().decode(errors="replace")
    _, status, ru = os.wait4(p.pid, 0)
    p.returncode = os.waitstatus_to_exitcode(status)
    return dict(exit=p.returncode, wall_s=round(time.monotonic() - t0, 3),
                cpu_s=round(ru.ru_utime + ru.ru_stime, 3), maxrss_mb=round(ru.ru_maxrss / 1024, 1),
                err=err.strip().splitlines()[-3:])


def sha_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def sha_decoded(path, codec):
    """sha256 of the bytes an independent decoder gets from `path`."""
    if codec == "none":
        return sha_file(path)
    if codec == "gzip":
        cmd = f"gzip -dc < '{path}' | sha256sum"
        out = subprocess.run(["bash", "-o", "pipefail", "-c", cmd], capture_output=True, text=True)
    else:
        code = ("import hashlib,sys,zstandard;h=hashlib.sha256();"
                "r=zstandard.ZstdDecompressor(max_window_size=1<<27).stream_reader(open(sys.argv[1],'rb'),"
                "read_across_frames=True)\n"
                "for b in iter(lambda:r.read(1<<20),b''): h.update(b)\nprint(h.hexdigest())")
        out = subprocess.run([ZPY, "-c", code, path], capture_output=True, text=True)
    return out.stdout.split()[0] if out.returncode == 0 and out.stdout.strip() else f"ERROR: {out.stderr[-200:]}"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("--docs", type=int)
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "box"))
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--levels", default="gzip:1,gzip:6,gzip:9,zstd:1,zstd:3,zstd:9,zstd:19")
    ap.add_argument("--part-size", default="64mb")
    ap.add_argument("--slow-reps", type=int, default=1, help="reps for zstd levels >= 19 (~2 MB/s per core)")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--run-id")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    codecs = [("none", None)] + [(c.split(":")[0], int(c.split(":")[1])) for c in a.levels.split(",")]
    rng = random.Random(a.seed)

    tiers = tomllib.load(open(os.path.join(HERE, "scenarios", "tiers.toml"), "rb"))
    scen = {"name": "compress", "seed": [{"index": "load-logs", "profile": "logs", "seed": 1, "docs": "tier"}],
            "step": [{"kind": "dump"}] * 3}
    ok, why, plan = R.fit(a.tier, a.place, tiers, scen, a.docs)
    if not ok:
        print(f"COMPRESS-SUMMARY tier={a.tier} place={a.place} REFUSED: {why}")
        return 2
    lock = open("/tmp/elasticdump-rs-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        fcntl.flock(lock, fcntl.LOCK_EX)
    need_mem = plan["heap_mb"] * 2 + 512 + plan["place"]["tool_mem_mb"] + 256
    if R.meminfo_avail_mb() < need_mem:
        print(f"COMPRESS-SUMMARY tier={a.tier} SKIPPED (MemAvailable {R.meminfo_avail_mb()} MB < {need_mem} MB)")
        return 3
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-compress-{a.tier}"
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
    env = dict(os.environ, **base, IT_DIR=os.path.join(work, "it"), IT_LOCK_HELD="1", RUN_ID=f"load{os.getpid()}cmp",
               ES_HEAP=heap, OS_HEAP=heap, PORT="19202", IT_CONTAINER_MEM=plan["place"]["container_mem"])
    res = dict(schema="edrs-compress/v1", run_id=run_id, tier=a.tier, place=a.place, engine=a.engine, heap=heap,
               seed=a.seed, reps=a.reps, limit=LIMIT, nproc=os.cpu_count(),
               git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                  capture_output=True, text=True).stdout.strip(),
               tool=subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.strip(),
               started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
               runs=[], checks=[], failures=[])
    rc = 1

    def fail(msg):
        res["failures"].append(msg)
        log("FAIL", msg)

    try:
        log(f"provisioning {a.engine} (heap {heap})")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        url = R.sh_env(os.path.join(env["IT_DIR"], "env"))["IT_ES_URL"]
        cl = R.Cluster(url)
        res["engine_version"] = cl.req("GET", "/")["version"]["number"]
        s = dict(plan["seeds"][0])
        log(f"seeding load-logs: {s['docs']} docs")
        res["seed_index"] = R.seed_index(cl, edgen, s, R.engine_mapping_flavour(a.engine))

        def digest(tag, files):
            rep = os.path.join(resdir, f"verify-{tag}.json")
            p = subprocess.run([edverify, "diff", "--a", f"{url}/load-logs", "--b", *files, "--index-as", "x",
                                "--out", rep], capture_output=True, text=True)
            return p.returncode == 0

        ref = os.path.join(work, "ref.ndjson")
        r = timed([tool, "--input", f"{url}/load-logs", "--output", ref, "--type=data", f"--limit={LIMIT}"])
        if r["exit"] or not digest("ref", [ref]):
            raise RuntimeError(f"reference dump failed: {r['err']}")
        raw = os.path.getsize(ref)
        res["raw_bytes"] = raw
        res["raw_sha"] = sha_file(ref)
        log(f"reference: {raw / 1e6:.0f} MB")

        def cflags(c, lv):
            return [] if c == "none" else [f"--compressionType={c}", f"--compressionLevel={lv}"]

        dump_sha = {}
        for rep in range(a.reps):
            order = codecs[:]
            rng.shuffle(order)
            for c, lv in order:
                if c == "zstd" and lv >= 19 and rep >= a.slow_reps:
                    continue
                tag = c if c == "none" else f"{c}{lv}"
                for mode in ("f2f", "dump"):
                    out = os.path.join(work, f"{mode}-{tag}.json{EXT[c]}")
                    src = ref if mode == "f2f" else f"{url}/load-logs"
                    r = timed([tool, "--input", src, "--output", out, "--type=data", f"--limit={LIMIT}",
                               "--overwrite", *cflags(c, lv)])
                    row = dict(rep=rep, mode=mode, codec=c, level=lv, **{k: r[k] for k in
                               ("exit", "wall_s", "cpu_s", "maxrss_mb")})
                    if r["exit"]:
                        row["err"] = r["err"]
                        fail(f"{mode}-{tag} rep{rep}: exit {r['exit']}: {r['err'][-1:]}")
                    else:
                        row["bytes"] = os.path.getsize(out)
                        row["ratio"] = round(raw / row["bytes"], 3)
                    res["runs"].append(row)
                    log(f"rep{rep} {mode} {tag}: exit {r['exit']} {r['wall_s']} s, {r['cpu_s']} CPU-s, "
                        f"{row.get('bytes', 0) / 1e6:.1f} MB")
                    if rep == 0 and not r["exit"]:
                        ck = dict(case=f"{mode}-{tag}")
                        if mode == "f2f":
                            ck["decoded_equal_ref"] = sha_decoded(out, c) == res["raw_sha"]
                        else:
                            d_sha = sha_decoded(out, c)
                            dump_sha[tag] = d_sha
                            ck["decoded_equal_plain_dump"] = d_sha == dump_sha.get("none", d_sha)
                        back = os.path.join(work, "back.json")
                        rb = timed([tool, "--input", out, "--output", back, "--type=data", f"--limit={LIMIT}",
                                    "--overwrite"])
                        ck["tool_reads_back_equal"] = rb["exit"] == 0 and sha_file(back) == sha_decoded(out, c)
                        if mode == "dump":
                            ck["digest_equal_index"] = rb["exit"] == 0 and digest(f"dump-{tag}", [back])
                        os.remove(back)
                        res["checks"].append(ck)
                        bad = [k for k, v in ck.items() if v is False]
                        if bad:
                            fail(f"{ck['case']}: {bad}")
                    if os.path.exists(out) and (rep or mode == "f2f" or c != "none"):
                        os.remove(out)

        # parts: --fileSize, cat, decode
        for c, lv in (("gzip", 6), ("zstd", 3)):
            pd = os.path.join(work, f"parts-{c}")
            shutil.rmtree(pd, ignore_errors=True)
            os.makedirs(pd)
            # --output p.json: parts are p.split-N.json.gz / .zst, as Node names them (Node's
            # generateFilePath appends the codec suffix after the extension, so p.json.gz would give
            # p.json.split-N.gz.gz in both tools)
            out = os.path.join(pd, "p.json")
            r = timed([tool, "--input", ref, "--output", out, "--type=data", f"--limit={LIMIT}",
                       f"--fileSize={a.part_size}", *cflags(c, lv)])
            parts = sorted(glob.glob(os.path.join(pd, "*")), key=lambda p: (len(p), p))
            ck = dict(case=f"parts-{c}{lv}", exit=r["exit"], parts=len(parts),
                      part_names=[os.path.basename(p) for p in parts[:3]] + (["..."] if len(parts) > 3 else []))
            # order the parts the way the tool numbered them; the concatenation must equal the reference
            cat = os.path.join(work, f"cat{EXT[c]}")
            with open(cat, "wb") as w:
                for p in parts:
                    with open(p, "rb") as f:
                        shutil.copyfileobj(f, w)
            ck["cat_decodes_to_ref"] = sha_decoded(cat, c) == res["raw_sha"]
            ck["each_part_decodes"] = all(not sha_decoded(p, c).startswith("ERROR") for p in parts)
            back = os.path.join(work, "back.json")
            rb = timed([tool, "--input", cat, "--output", back, "--type=data", f"--limit={LIMIT}", "--overwrite"])
            ck["tool_reads_cat_equal_ref"] = rb["exit"] == 0 and sha_file(back) == res["raw_sha"]
            ck["max_part_bytes"] = max(os.path.getsize(p) for p in parts) if parts else 0
            res["checks"].append(ck)
            log(f"parts {c}{lv}: {ck}")
            bad = [k for k, v in ck.items() if v is False] + (["exit"] if r["exit"] else [])
            if bad or len(parts) < 2:
                fail(f"{ck['case']}: {bad or 'fewer than 2 parts'}")
            shutil.rmtree(pd)
            os.remove(cat)
            if os.path.exists(back):
                os.remove(back)

        # medians
        summ = []
        for c, lv in codecs:
            for mode in ("f2f", "dump"):
                rows = [x for x in res["runs"] if x["codec"] == c and x["level"] == lv and x["mode"] == mode
                        and not x["exit"]]
                base_rows = [x for x in res["runs"] if x["codec"] == "none" and x["mode"] == mode and not x["exit"]]
                if not rows:
                    continue
                med = lambda k, rr=rows: round(statistics.median(x[k] for x in rr), 3)  # noqa: E731
                d = dict(mode=mode, codec=c, level=lv, n=len(rows), wall_s=med("wall_s"), cpu_s=med("cpu_s"),
                         bytes=int(med("bytes")), ratio=med("ratio"), maxrss_mb=med("maxrss_mb"))
                d["docs_per_s"] = round(res["seed_index"]["stored"] / d["wall_s"])
                if c != "none" and base_rows:
                    extra = d["cpu_s"] - statistics.median(x["cpu_s"] for x in base_rows)
                    d["codec_cpu_s"] = round(extra, 3)
                    d["mb_per_s_per_core"] = round(raw / 1e6 / extra, 1) if extra > 0 else None
                summ.append(d)
        res["summary"] = summ
        res["leak"], f = R.leak_check(cl, work)  # LOAD-017
        res["failures"] += f
        rc = 0 if not res["failures"] else 1
    except Exception as e:
        res["failures"].append(f"error: {e!r}")
        log("ERROR", repr(e))
        rc = 1
    finally:
        res["finished"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
        res["pass"] = rc == 0
        json.dump(res, open(os.path.join(resdir, "compress.json"), "w"), indent=1)
        if not a.keep:
            subprocess.run(["podman", "rm", "-f", f"it-es-plain-{env['RUN_ID']}"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
            subprocess.run(["podman", "unshare", "rm", "-rf", work])
    for d in res.get("summary", []):
        log(f"{d['mode']:4} {d['codec']}{'' if d['level'] is None else d['level']:<3} wall {d['wall_s']} s "
            f"cpu {d['cpu_s']} s ratio {d['ratio']} MB/s/core {d.get('mb_per_s_per_core')}")
    print(f"COMPRESS-SUMMARY tier={a.tier} engine={a.engine} docs={res.get('seed_index', {}).get('stored')} "
          f"{'PASS' if rc == 0 else 'FAIL'} runs={len(res['runs'])} checks={len(res['checks'])} "
          f"result=results/load/{run_id}/compress.json")
    for f in res["failures"]:
        print("  failure:", f)
    return rc


if __name__ == "__main__":
    sys.exit(main())
