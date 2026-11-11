#!/usr/bin/env python3
"""LOAD-006 (L-803): resumability under load, with the README recipes run verbatim.

    scripts/load/resume.py <tier> [--docs N] [--engine es717-plain] [--place box]
                           [--points 10] [--seed 1] [--only a,b,c,c1,d] [--run-id ID] [--keep]

Provisions one engine (scripts/it/up.sh), seeds `load-logs` with edgen (logs,
seed 1), takes the reference digest and a reference dump, then:

  (a)  dump to a file, `kill -9` at --points random points (KILL_AT_DOCS: the
       staged file has reached that many documents' worth of bytes). Each time:
       the final path is absent, the hidden staging files are left, the rerun
       with `--overwrite` names them (BRK-SIG-003) and gives the full digest.
  (b)  restore (--concurrency 8 --limit 1000) `kill -9` when the target has
       KILL_AT_DOCS documents, then the same command again: digest equal.
  (c)  restore stopped by SIGINT / SIGTERM at three points in a row, each time
       resumed with the `--offset` the message prints (checked equal to the
       last `Total Writes`); --concurrency 8 and 1. Digest equal.
  (c1) restore --concurrency 1 `kill -9`, then `--offset` = the target's
       `_count` ("count from what the cluster has"). Digest equal.
  (d)  `--scrollId`: the first page from our own `_search?scroll=`, the rest
       from `--scrollId` (README recipe). Digest equal (the first page is taken
       byte for byte from the verified reference dump). Plus `kill -9` mid-way and a
       rerun with the same id: exits 0, no duplicates, and only the pages the
       killed run fetched are missing (the documented behaviour).

Kill points come from random.Random(--seed). Results: results/load/<run-id>/resume.json.
Exit 0 when every case holds, 1 otherwise, 2 refused, 3 skipped (resource guard).
"""
import argparse, datetime, fcntl, glob, json, os, random, re, shutil, signal, subprocess, sys, time, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run as R  # noqa: E402  (Cluster, seed_index, sh_env, tool_path, fit, ...)

ROOT = R.ROOT
LIMIT = 1000


def log(*a):
    print("resume:", *a, file=sys.stderr, flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("--docs", type=int)
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "box"))
    ap.add_argument("--points", type=int, default=10)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--only", default="a,b,c,c1,d")
    ap.add_argument("--run-id")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    only = set(a.only.split(","))
    rng = random.Random(a.seed)

    tiers = tomllib.load(open(os.path.join(HERE, "scenarios", "tiers.toml"), "rb"))
    scen = {"name": "resume", "seed": [{"index": "load-logs", "profile": "logs", "seed": 1, "docs": "tier"}],
            "step": [{"kind": "dump"}, {"kind": "dump"}, {"kind": "restore"}]}
    ok, why, plan = R.fit(a.tier, a.place, tiers, scen, a.docs)
    if not ok:
        print(f"RESUME-SUMMARY tier={a.tier} place={a.place} REFUSED: {why}")
        return 2
    lock = open("/tmp/searchdump-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        fcntl.flock(lock, fcntl.LOCK_EX)
    need_mem = plan["heap_mb"] * 2 + 512 + plan["place"]["tool_mem_mb"] + 256
    if R.meminfo_avail_mb() < need_mem:
        print(f"RESUME-SUMMARY tier={a.tier} SKIPPED (MemAvailable {R.meminfo_avail_mb()} MB < {need_mem} MB)")
        return 3
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-resume-{a.tier}"
    work = os.path.join(os.environ.get("LOAD_WORK", "/var/tmp/searchdump-load"), run_id)
    os.makedirs(work, mode=0o700, exist_ok=True)
    resdir = os.path.join(ROOT, "results", "load", run_id)
    os.makedirs(resdir, exist_ok=True)
    edgen, edverify, tool = R.tool_path("edgen"), R.tool_path("edverify"), R.tool_path("searchdump")
    it_dir = os.path.join(work, "it")
    env = dict(os.environ, IT_DIR=it_dir, IT_LOCK_HELD="1", RUN_ID=f"load{os.getpid()}", IT_ES_DATA_MOUNT="1")
    for line in subprocess.run([os.path.join(ROOT, "scripts/it/matrix.sh"), a.engine],
                               check=True, capture_output=True, text=True).stdout.splitlines():
        k, v = line.split("=", 1)
        env[k] = v
    up_profile = env.pop("UP_PROFILE")
    heap = f"{plan['heap_mb']}m"
    env.update(ES_HEAP=heap, OS_HEAP=heap, IT_CONTAINER_MEM=plan["place"]["container_mem"])
    res = dict(schema="edrs-resume/v1", run_id=run_id, tier=a.tier, place=a.place, engine=a.engine, heap=heap,
               seed=a.seed, git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                               capture_output=True, text=True).stdout.strip(),
               tool=subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.strip(),
               started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"), cases={}, failures=[])
    rc = 1
    try:
        log(f"provisioning {a.engine} (heap {heap})")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        ce = R.sh_env(os.path.join(it_dir, "env"))
        url = ce["IT_ES_URL"]
        if ce.get("IT_ADMIN_USER"):
            raise RuntimeError("resume.py supports plain (no-auth) engines only")
        cl = R.Cluster(url)
        res["engine_version"] = cl.req("GET", "/")["version"]["number"]
        s = dict(plan["seeds"][0])
        log(f"seeding load-logs: {s['docs']} docs")
        res["seed_index"] = R.seed_index(cl, edgen, s, R.engine_mapping_flavour(a.engine))
        n = res["seed_index"]["stored"]
        src = f"{url}/load-logs"
        mapping = json.loads(subprocess.run([edgen, "mapping", "--profile", "logs", "--engine",
                                             R.engine_mapping_flavour(a.engine)], check=True,
                                            capture_output=True).stdout)

        def diff(name, a_src, b_src, canonical=False):
            rep = os.path.join(resdir, f"verify-{name}.json")
            b_src = [b_src] if isinstance(b_src, str) else list(b_src)
            p = subprocess.run([edverify, "diff", "--a", a_src, "--b", *b_src, "--index-as", "x", "--out", rep,
                                *(["--canonical"] if canonical else [])], capture_output=True, text=True)
            v = json.load(open(rep)) if os.path.exists(rep) else {}
            return dict(ok=p.returncode == 0, a=v.get("a", {}).get("count"), b=v.get("b", {}).get("count"),
                        detail=None if p.returncode == 0 else (p.stdout + p.stderr)[-600:])

        def tool_run(args, timeout=3600):
            p = subprocess.run([tool, *args], capture_output=True, text=True, timeout=timeout)
            return p.returncode, p.stderr

        def spawn(args):
            return subprocess.Popen([tool, *args], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)

        def fail(case, msg):
            res["failures"].append(f"{case}: {msg}")
            log("FAIL", case, msg)

        # Reference dump: its size gives bytes per document for the dump kill points.
        ref = os.path.join(work, "ref.ndjson")
        t0 = time.time()
        code, err = tool_run(["--input", src, "--output", ref, "--type=data", f"--limit={LIMIT}"])
        res["reference_dump"] = dict(exit=code, wall_s=round(time.time() - t0, 2), bytes=os.path.getsize(ref),
                                     verify=diff("ref", src, ref))
        if code or not res["reference_dump"]["verify"]["ok"]:
            raise RuntimeError(f"reference dump failed: {err[-400:]}")
        bpd = os.path.getsize(ref) / n

        def points(k, lo=0.05, hi=0.95):
            return sorted(int(n * rng.uniform(lo, hi)) for _ in range(k))

        # (a) dump kill -9 at random points, rerun with --overwrite.
        if "a" in only:
            rows = []
            for i, kat in enumerate(points(a.points)):
                out = os.path.join(work, f"a{i}.ndjson")
                cmd = ["--input", src, "--output", out, "--type=data", f"--limit={LIMIT}"]
                p = spawn(cmd)
                staged = None
                while p.poll() is None:
                    parts = glob.glob(os.path.join(work, f".a{i}.ndjson.part-*"))
                    if parts:
                        staged = parts[0]
                        try:
                            if os.path.getsize(staged) >= kat * bpd:
                                p.kill()
                                break
                        except FileNotFoundError:
                            pass
                    time.sleep(0.005)
                p.wait()
                killed = p.returncode == -signal.SIGKILL
                left = sorted(os.path.basename(x) for x in glob.glob(os.path.join(work, f".a{i}.ndjson*")))
                row = dict(kill_at_docs=kat, killed=killed, exit=p.returncode, final_absent=not os.path.exists(out),
                           leftovers=left, staged_docs_approx=int(os.path.getsize(os.path.join(work, left[0])) / bpd)
                           if left else 0)
                code, err = tool_run(cmd + ["--overwrite"])
                row["rerun_exit"] = code
                row["rerun_names_leftovers"] = bool(left) and all(x in err for x in left)
                row["verify"] = diff(f"a{i}", src, out) if code == 0 else dict(ok=False, detail=err[-400:])
                if not killed:
                    fail(f"a-{i}", "the run finished before the kill point")
                elif not row["final_absent"]:
                    fail(f"a-{i}", "final path exists after kill -9")
                elif not left:
                    fail(f"a-{i}", "no staging leftovers after kill -9")
                elif not row["rerun_names_leftovers"]:
                    fail(f"a-{i}", f"rerun does not name the leftovers: {err[-400:]}")
                if not row["verify"]["ok"]:
                    fail(f"a-{i}", f"digest after rerun: {row['verify'].get('detail')}")
                for x in left:
                    os.remove(os.path.join(work, x))
                os.remove(out) if os.path.exists(out) else None
                rows.append(row)
            res["cases"]["a_dump_kill9_overwrite"] = rows

        def target(name):
            cl.req("DELETE", "/" + name, ok404=True)
            cl.req("PUT", "/" + name, mapping)
            return f"{url}/{name}"

        def count(name):
            cl.req("POST", f"/{name}/_refresh")
            return cl.req("GET", f"/{name}/_count")["count"]

        def wait_count(name, p, kat):
            while p.poll() is None:
                try:
                    if cl.req("GET", f"/{name}/_count")["count"] >= kat:
                        return True
                except RuntimeError:
                    pass
                time.sleep(0.05)
            return False

        # (b) restore kill -9, then the same command again.
        if "b" in only:
            rows = []
            for i, kat in enumerate(points(max(3, a.points // 2))):
                dst = target("rb")
                cmd = ["--input", ref, "--output", dst, "--type=data", "--concurrency=8", f"--limit={LIMIT}"]
                p = spawn(cmd)
                hit = wait_count("rb", p, kat)
                if hit:
                    p.kill()
                p.wait()
                row = dict(kill_at_docs=kat, killed=p.returncode == -signal.SIGKILL, count_after_kill=count("rb"))
                code, err = tool_run(cmd)
                row["rerun_exit"] = code
                row["verify"] = diff(f"b{i}", src, dst)
                if not row["killed"]:
                    fail(f"b-{i}", "the run finished before the kill point")
                if code or not row["verify"]["ok"]:
                    fail(f"b-{i}", f"rerun exit {code}, digest {row['verify']}")
                rows.append(row)
            cl.req("DELETE", "/rb", ok404=True)
            res["cases"]["b_restore_kill9_rerun"] = rows

        # (c) restore SIGINT/SIGTERM, resume with the printed --offset, chained.
        if "c" in only:
            rows = []
            for conc in (8, 1):
                for sig in (signal.SIGINT, signal.SIGTERM):
                    dst = target("rc")
                    base = ["--input", ref, "--output", dst, "--type=data", f"--concurrency={conc}", f"--limit={LIMIT}"]
                    offset, stops = 0, []
                    for kat in sorted(rng.sample(range(int(n * 0.05), int(n * 0.95)), 3)):
                        if kat <= offset:
                            continue
                        p = spawn(base + ([f"--offset={offset}"] if offset else []))
                        hit = wait_count("rc", p, kat)
                        if hit:
                            p.send_signal(sig)
                        _, err = p.communicate()
                        m = re.search(r"rerun the same command with --offset (\d+)", err)
                        tw = re.findall(r"Total Writes: (\d+)", err)
                        st = dict(stop_at_docs=kat, signal=sig.name, exit=p.returncode,
                                  printed_offset=int(m.group(1)) if m else None,
                                  total_writes=int(tw[-1]) if tw else None, offset_before=offset)
                        stops.append(st)
                        if p.returncode == 0 and st["total_writes"] is not None and offset + st["total_writes"] == n:
                            # The last bulk request was already sent when the signal came: a complete run.
                            st["finished_before_stop"] = True
                            offset = None
                            break
                        if not hit or p.returncode not in (130, 143):
                            fail(f"c-{conc}-{sig.name}", f"stop at {kat}: exit {p.returncode}: {err[-300:]}")
                            break
                        if st["printed_offset"] is None:
                            fail(f"c-{conc}-{sig.name}", f"no --offset in the message: {err[-300:]}")
                            break
                        # The offset counts input documents from the start; Total Writes counts this run.
                        if st["total_writes"] is not None and offset + st["total_writes"] != st["printed_offset"]:
                            fail(f"c-{conc}-{sig.name}", f"offset {st['printed_offset']} != earlier offset "
                                 f"{offset} + Total Writes {st['total_writes']}")
                        offset = st["printed_offset"]
                    if offset is None:  # already complete
                        code, err, offset = 0, "", n
                        tw = ["0"]
                    else:
                        code, err = tool_run(base + [f"--offset={offset}"])
                        tw = re.findall(r"Total Writes: (\d+)", err)
                    row = dict(concurrency=conc, signal=sig.name, stops=stops, final_offset=offset, final_exit=code,
                               final_total_writes=int(tw[-1]) if tw else None, verify=diff(f"c{conc}{sig.name}", src, dst))
                    if code or not row["verify"]["ok"]:
                        fail(f"c-{conc}-{sig.name}", f"final run exit {code}, digest {row['verify']}")
                    if row["final_total_writes"] is not None and offset + row["final_total_writes"] != n:
                        fail(f"c-{conc}-{sig.name}", f"offset {offset} + final Total Writes {row['final_total_writes']} != {n}")
                    rows.append(row)
            cl.req("DELETE", "/rc", ok404=True)
            res["cases"]["c_restore_signal_offset"] = rows

        # (c1) restore --concurrency 1 kill -9, --offset from the target's count.
        if "c1" in only:
            rows = []
            for i, kat in enumerate(points(3)):
                dst = target("rc1")
                base = ["--input", ref, "--output", dst, "--type=data", "--concurrency=1", f"--limit={LIMIT}"]
                p = spawn(base)
                hit = wait_count("rc1", p, kat)
                if hit:
                    p.kill()
                p.wait()
                have = count("rc1")
                code, err = tool_run(base + [f"--offset={have}"])
                row = dict(kill_at_docs=kat, killed=p.returncode == -signal.SIGKILL, offset_from_count=have,
                           rerun_exit=code, verify=diff(f"c1-{i}", src, dst))
                if not row["killed"] or code or not row["verify"]["ok"]:
                    fail(f"c1-{i}", f"killed={row['killed']} exit {code} digest {row['verify']}")
                rows.append(row)
            cl.req("DELETE", "/rc1", ok404=True)
            res["cases"]["c1_restore_kill9_offset_from_count"] = rows

        # (d) --scrollId while the context is alive.
        if "d" in only:
            rows = {}

            ref_line = {}

            def first_page(path):
                # The first page as the tool would write it: the reference dump's raw line for
                # each hit (byte-exact _source; the reference dump is verified against the index).
                r = cl.req("POST", "/load-logs/_search?scroll=10m", {"size": LIMIT, "sort": ["_doc"]})
                want = {h["_id"] for h in r["hits"]["hits"]}
                if not ref_line:
                    with open(ref, "rb") as f:
                        for line in f:
                            ref_line[json.loads(line)["_id"]] = line
                with open(path, "wb") as f:
                    for i in want:
                        f.write(ref_line[i])
                return r["_scroll_id"], len(want)

            # Recipe verbatim: keep the first page yourself, pass its _scroll_id.
            p1 = os.path.join(work, "d-first.ndjson")
            rest = os.path.join(work, "d-rest.ndjson")
            sid, got = first_page(p1)
            code, err = tool_run(["--input", src, "--output", rest, "--type=data", f"--limit={LIMIT}",
                                  f"--scrollId={sid}"])
            rows["recipe"] = dict(first_page_docs=got, exit=code, verify=diff("d-recipe", src, [p1, rest]))
            if code or not rows["recipe"]["verify"]["ok"]:
                fail("d-recipe", f"exit {code} digest {rows['recipe']['verify']} {err[-300:]}")
            os.remove(rest)
            cl.req("DELETE", "/_search/scroll", {"scroll_id": sid}, ok404=True)
            # kill -9 mid-way, rerun with the same id.
            sid, got = first_page(p1)
            killed_out = os.path.join(work, "d-killed.ndjson")
            p = spawn(["--input", src, "--output", killed_out, "--type=data", f"--limit={LIMIT}", f"--scrollId={sid}"])
            kat = int(n * 0.4)
            while p.poll() is None:
                parts = glob.glob(os.path.join(work, ".d-killed.ndjson.part-*"))
                if parts and os.path.getsize(parts[0]) >= kat * bpd:
                    p.kill()
                    break
                time.sleep(0.005)
            p.wait()
            parts = glob.glob(os.path.join(work, ".d-killed.ndjson.part-*"))
            staged = sum(1 for _ in open(parts[0], "rb")) if parts else 0
            code, err = tool_run(["--input", src, "--output", rest, "--type=data", f"--limit={LIMIT}",
                                  f"--scrollId={sid}"])
            got_rest = sum(1 for _ in open(rest, "rb")) if code == 0 else None
            ids = set()
            dups = 0
            for fpath in [p1, rest] if code == 0 else []:
                for line in open(fpath, "rb"):
                    i = json.loads(line)["_id"]
                    dups += i in ids
                    ids.add(i)
            row = dict(killed=p.returncode == -signal.SIGKILL, staged_docs_of_killed_run=staged, rerun_exit=code,
                       rerun_docs=got_rest, first_page_docs=got, duplicates=dups,
                       missing=n - len(ids) if code == 0 else None)
            # Pages the killed run fetched are gone; the staged file holds at most those (whole pages).
            if not row["killed"] or code or dups:
                fail("d-kill9", f"{row} {err[-300:]}")
            elif not (staged <= row["missing"] <= staged + 20 * LIMIT):
                fail("d-kill9", f"missing {row['missing']} not explained by the killed run's pages (staged {staged})")
            rows["kill9_then_same_scroll_id"] = row
            for x in parts:
                os.remove(x)
            cl.req("DELETE", "/_search/scroll", {"scroll_id": sid}, ok404=True)
            res["cases"]["d_scroll_id"] = rows

        res["leak"], f = R.leak_check(cl, work)  # LOAD-017
        res["failures"] += f
        rc = 0 if not res["failures"] else 1
    except Exception as e:
        res["failures"].append(f"error: {e!r}")
        rc = 1
    finally:
        res["finished"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
        res["pass"] = rc == 0
        out = os.path.join(resdir, "resume.json")
        json.dump(res, open(out, "w"), indent=1)
        if not a.keep:
            subprocess.run([os.path.join(ROOT, "scripts/it/down.sh")], env=env, stdout=subprocess.DEVNULL)
            shutil.rmtree(work, ignore_errors=True)
    bad = {f.split(":")[0].split("-")[0] for f in res["failures"]}
    summ = " ".join(f"{k}={'FAIL' if k.split('_')[0] in bad else 'ok'}" for k in res["cases"])
    print(f"RESUME-SUMMARY tier={a.tier} engine={a.engine} docs={res.get('seed_index', {}).get('stored')} "
          f"{'PASS' if rc == 0 else 'FAIL'} {summ} result={os.path.relpath(out, ROOT)}")
    for f in res["failures"]:
        print("  failure:", f)
    return rc


if __name__ == "__main__":
    sys.exit(main())
