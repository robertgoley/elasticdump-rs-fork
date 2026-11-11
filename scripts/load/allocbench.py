#!/usr/bin/env python3
"""L-802a: allocator comparison (glibc default, MALLOC_ARENA_MAX=2, jemalloc, mimalloc,
musl default / jemalloc / mimalloc) on restore, copy and dump (plain and sliced).

  allocbench.py --variant NAME=BIN[,KEY=VAL...] [--variant ...] --docs 100000,1000000
                [--ops restore,copy,dump,dump-sliced] [--reps 3] [--engine es717-plain]
                [--heap 2048m] [--container-mem 4g] [--run-id ID] [--keep]

BIN may be `ldso:LIBDIR:BIN` to run a musl (Alpine-built, dynamically linked) binary on
this host through LIBDIR/ld-musl-*.so.1 with LIBDIR as the library path (the binary and
its musl libc are the ones from the Alpine builder image; nothing is emulated).

One engine (same heap at every size), one source index per size, seeded by a restore
with the first variant (not timed). Every cell runs --concurrency=8 --limit=5000 (dumps:
--limit=5000; sliced: --slices=4) and is repeated --reps times, variants interleaved
within each repetition. Per run: wall time, docs/s, peak RSS (the child's own VmHWM,
read from /proc every 0.1 s) and the memcheck plateau check (peak RSS after the first
half of the run vs before it, from VmRSS sampled every 0.1 s; at most 10% passes).

Peak RSS is NOT getrusage/wait4 ru_maxrss: Python's subprocess starts children with
vfork, and Linux then counts the parent's resident set in the child's maxrss at exec
(a /bin/true child of a 200 MB Python reports ~210 MB), so every small run read as the
driver's own RSS (~57 MB here). VmHWM belongs to the child's own address space. Restore and copy targets are recreated with the
mapping and checked by _count; dumps go to stdout (/dev/null) and are checked by line
count. Results: results/load/<run-id>/allocbench.json (one row per run).
"""
import argparse, datetime, fcntl, glob, json, os, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
import run as R  # noqa: E402

LIMIT, CONC, SLICES = 5000, 8, 4


def log(*a):
    print("allocbench:", *a, file=sys.stderr, flush=True)


def parse_variant(spec):
    name, rest = spec.split("=", 1)
    parts = rest.split(",")
    env = dict(p.split("=", 1) for p in parts[1:])
    b = parts[0]
    if b.startswith("ldso:"):
        _, libdir, binp = b.split(":", 2)
        ldso = glob.glob(os.path.join(libdir, "ld-musl-*.so.1"))[0]
        argv = [ldso, "--library-path", libdir, binp]
    else:
        argv = [b]
    return dict(name=name, argv=argv, env=env, bin=os.path.basename(argv[-1]))


def run_sampled(argv, env, stdout):
    """Run, sample VmRSS/VmHWM every 0.1 s; return (rc, wall_s, peak_kb, samples, stderr).
    peak_kb is the child's own VmHWM (see the module doc for why not ru_maxrss)."""
    t0 = time.time()
    p = subprocess.Popen(argv, env=env, stdout=stdout, stderr=subprocess.PIPE)
    os.set_blocking(p.stderr.fileno(), False)
    err, samples, hwm = b"", [], 0
    while True:
        try:
            with open(f"/proc/{p.pid}/status") as fh:
                for line in fh:
                    if line.startswith("VmRSS:"):
                        samples.append((time.time() - t0, int(line.split()[1])))
                    elif line.startswith("VmHWM:"):
                        hwm = max(hwm, int(line.split()[1]))
        except (FileNotFoundError, ProcessLookupError):
            pass
        try:
            chunk = p.stderr.read()
            if chunk:
                err += chunk
        except (BlockingIOError, TypeError):
            pass
        pid, status, ru = os.wait4(p.pid, os.WNOHANG)
        if pid:
            break
        time.sleep(0.1)
    wall = time.time() - t0
    p.returncode = os.waitstatus_to_exitcode(status)
    try:
        rest = p.stderr.read()
        if rest:
            err += rest
    except (BlockingIOError, TypeError):
        pass
    del ru  # ru_maxrss includes the parent's RSS (vfork); see the module doc
    return p.returncode, wall, hwm, samples, err.decode(errors="replace")


def plateau(samples, settle=50):
    if len(samples) < 10:
        return None
    end = samples[-1][0]
    early = max(r for t, r in samples if t <= end * settle / 100)
    late = max((r for t, r in samples if t > end * settle / 100), default=0)
    return round((late / early - 1) * 100, 1)


PLATEAU_PCT = 10.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--variant", action="append", required=True)
    ap.add_argument("--docs", default="100000,1000000")
    ap.add_argument("--ops", default="restore,copy,dump,dump-sliced")
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--heap", default="2048m")
    ap.add_argument("--container-mem", default="4g")
    ap.add_argument("--run-id")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    variants = [parse_variant(v) for v in a.variant]
    ops = a.ops.split(",")
    sizes = [int(x) for x in a.docs.split(",")]
    lock = open("/tmp/searchdump-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        fcntl.flock(lock, fcntl.LOCK_EX)
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + "-allocbench"
    work = os.path.join(os.environ.get("LOAD_WORK", "/var/tmp/searchdump-load"), run_id)
    os.makedirs(work, mode=0o700, exist_ok=True)
    resdir = os.path.join(ROOT, "results", "load", run_id)
    os.makedirs(resdir, exist_ok=True)
    out_path = os.path.join(resdir, "allocbench.json")
    edgen = R.tool_path("edgen")
    it_dir = os.path.join(work, "it")
    env = dict(os.environ, IT_DIR=it_dir, IT_LOCK_HELD="1", RUN_ID=f"load{os.getpid()}", IT_ES_DATA_MOUNT="1")
    for line in subprocess.run([os.path.join(ROOT, "scripts/it/matrix.sh"), a.engine],
                               check=True, capture_output=True, text=True).stdout.splitlines():
        k, v = line.split("=", 1)
        env[k] = v
    up_profile = env.pop("UP_PROFILE")
    env.update(ES_HEAP=a.heap, OS_HEAP=a.heap, IT_CONTAINER_MEM=a.container_mem)
    res = dict(schema="edrs-allocbench/v1", run_id=run_id, engine=a.engine, heap=a.heap,
               flags=dict(limit=LIMIT, concurrency=CONC, slices_for_dump_sliced=SLICES),
               variants=[dict(name=v["name"], bin=v["bin"], env=v["env"], musl_ldso=len(v["argv"]) > 1)
                         for v in variants],
               git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                  capture_output=True, text=True).stdout.strip(),
               started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"), runs=[], failures=[])

    def save():
        json.dump(res, open(out_path, "w"), indent=1)

    rc = 1
    try:
        log(f"provisioning {a.engine} (heap {a.heap})")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        url = R.sh_env(os.path.join(it_dir, "env"))["IT_ES_URL"]
        cl = R.Cluster(url)
        res["engine_version"] = cl.req("GET", "/")["version"]["number"]
        mapping = json.loads(subprocess.run([edgen, "mapping", "--profile", "logs", "--engine",
                                             R.engine_mapping_flavour(a.engine)], check=True,
                                            capture_output=True).stdout)

        def fresh(index):
            cl.req("DELETE", "/" + index, ok404=True)
            cl.req("PUT", "/" + index, mapping)

        def count(index):
            cl.req("POST", f"/{index}/_refresh")
            return cl.req("GET", f"/{index}/_count")["count"]

        for n in sizes:
            data = os.path.join(work, f"logs-{n}.ndjson")
            log(f"generating {n} docs")
            with open(data, "wb") as fh:
                subprocess.run([edgen, "gen", "--profile", "logs", "--seed", "1", "--count", str(n),
                                "--index", "load-logs"], check=True, stdout=fh)
            fresh("src")
            v0 = variants[0]
            p = subprocess.run([*v0["argv"], f"--input={data}", f"--output={url}/src", "--type=data",
                                f"--concurrency={CONC}", f"--limit={LIMIT}"],
                               env=dict(os.environ, **v0["env"]), capture_output=True, text=True)
            got = count("src")
            if p.returncode or got != n:
                raise RuntimeError(f"seeding src: exit {p.returncode} count {got}: {p.stderr[-400:]}")
            cl.req("POST", "/src/_forcemerge?max_num_segments=1")
            for rep in range(a.reps):
                for op in ops:
                    for v in variants:
                        venv = dict(os.environ, **v["env"])
                        if op == "restore":
                            fresh("dst")
                            args = [f"--input={data}", f"--output={url}/dst", "--type=data",
                                    f"--concurrency={CONC}", f"--limit={LIMIT}"]
                        elif op == "copy":
                            fresh("dst")
                            args = [f"--input={url}/src", f"--output={url}/dst", "--type=data",
                                    f"--concurrency={CONC}", f"--limit={LIMIT}"]
                        elif op == "dump":
                            args = [f"--input={url}/src", "--output=$", "--type=data", f"--limit={LIMIT}"]
                        else:
                            args = [f"--input={url}/src", "--output=$", "--type=data", f"--limit={LIMIT}",
                                    f"--slices={SLICES}"]
                        if op.startswith("dump"):
                            cnt = os.path.join(work, "dump.out")
                            with open(cnt, "wb") as fh:
                                code, wall, maxrss, s, err = run_sampled([*v["argv"], *args], venv, fh)
                            with open(cnt, "rb") as fh:
                                got = sum(chunk.count(b"\n") for chunk in iter(lambda: fh.read(1 << 24), b""))
                            os.unlink(cnt)
                        else:
                            code, wall, maxrss, s, err = run_sampled([*v["argv"], *args], venv,
                                                                     subprocess.DEVNULL)
                            got = count("dst")
                        row = dict(docs=n, op=op, variant=v["name"], rep=rep, exit=code, wall_s=round(wall, 2),
                                   docs_per_s=round(n / wall) if wall else None, peak_rss_mb=round(maxrss / 1024, 1),
                                   plateau_growth_pct=plateau(s), samples=len(s), count=got)
                        pg = row["plateau_growth_pct"]
                        row["plateau_ok"] = None if pg is None else pg <= PLATEAU_PCT
                        res["runs"].append(row)
                        if code or got != n:
                            res["failures"].append(f"{n}/{op}/{v['name']}/{rep}: exit {code} count {got}: {err[-300:]}")
                        log(f"{n} {op:11} {v['name']:16} rep{rep} {row['docs_per_s']} d/s "
                            f"{row['peak_rss_mb']} MB plateau {row['plateau_growth_pct']}%"
                            f"{'' if row['plateau_growth_pct'] is None or row['plateau_growth_pct'] <= PLATEAU_PCT else ' (>10%)'}"
                            f" {'OK' if not code and got == n else 'FAIL'}")
                        save()
            cl.req("DELETE", "/dst", ok404=True)
            cl.req("DELETE", "/src", ok404=True)
            os.unlink(data)
        res["leak"], f = R.leak_check(cl, work)  # LOAD-017
        res["failures"] += f
        rc = 1 if res["failures"] else 0
    except Exception as e:
        res["failures"].append(f"error: {e}")
        log("ERROR", e)
    finally:
        res["finished"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
        save()
        if not a.keep:
            subprocess.run([os.path.join(ROOT, "scripts/it/down.sh")], env=env, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
            shutil.rmtree(work, ignore_errors=True)
    print(f"ALLOCBENCH-SUMMARY runs={len(res['runs'])} failures={len(res['failures'])} result={os.path.relpath(out_path, ROOT)}")
    return rc


if __name__ == "__main__":
    sys.exit(main())
