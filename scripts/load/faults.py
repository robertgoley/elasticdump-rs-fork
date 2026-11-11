#!/usr/bin/env python3
"""LOAD-007 (L-804): failure injection under load.

    scripts/load/faults.py <tier> [--docs N] [--engine es717-plain] [--place box]
                           [--only cut,storm,flood,restart] [--seed 1] [--run-id ID] [--keep]

Provisions one engine (scripts/it/up.sh), seeds `load-logs` with edgen (logs, seed 1)
and takes a reference dump, then runs restores (file -> cluster, --concurrency 8),
copies (cluster -> cluster, --concurrency 8) and dumps (cluster -> file) under:

  cut      a fault proxy in front of the node (in this script): `cut-conn` resets 1 in 50
           TCP connections at a random byte of the response stream; `cut-req` resets the
           connection on 1 in 50 HTTP requests, half before the request reaches the node
           and half after it (the response is lost, so a bulk may be applied and resent).
  storm    a second node started with thread_pool.write.size=1 and queue_size=1: bulk
           items come back 429 (es_rejected_execution_exception) all the time.
  flood    mid-run, the disk watermarks are set to 1% so the node puts every index under
           the flood-stage block (index.blocks.read_only_allow_delete, 429
           cluster_block_exception); 30 s later they are reset and the node lifts it.
  restart  `podman restart` of the node mid-run.

Each run must either exit 0 with the target digest-equal to the source (edverify diff),
or exit non-zero with a clear message (no panic, a final error line; dumps leave no final
file). Exit 0 with missing or extra documents is a failure (BRK-SRV-001). Non-zero exits
are recorded with their message for the lead to classify.
Results: results/load/<run-id>/faults.json. Exit 0 when no run lost data silently, 1
otherwise, 2 refused, 3 skipped (resource guard).
"""
import argparse, asyncio, datetime, fcntl, json, os, random, re, shutil, socket, struct, subprocess, sys, \
    threading, time, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run as R  # noqa: E402

ROOT = R.ROOT
LIMIT = 1000
CONC = 8
FLOOD_S = 30


def log(*a):
    print("faults:", *a, file=sys.stderr, flush=True)


class CutProxy:
    """TCP proxy that resets 1 in `every` connections (mode conn) or requests (mode req)."""

    def __init__(self, upstream_port, mode, every=50, seed=1):
        self.up, self.mode, self.every, self.rng = upstream_port, mode, every, random.Random(seed)
        self.stats = dict(mode=mode, every=every, connections=0, requests=0, cuts=0, cuts_before=0, cuts_after=0)
        self.loop = asyncio.new_event_loop()
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        self.port = s.getsockname()[1]
        s.close()
        self.ready = threading.Event()
        threading.Thread(target=self._run, daemon=True).start()
        self.ready.wait(10)

    def _run(self):
        asyncio.set_event_loop(self.loop)
        self.server = self.loop.run_until_complete(asyncio.start_server(self._conn, "127.0.0.1", self.port))
        self.ready.set()
        self.loop.run_forever()

    def stop(self):
        def _s():
            self.server.close()
            self.loop.stop()
        self.loop.call_soon_threadsafe(_s)

    @staticmethod
    def _rst(w):
        s = w.get_extra_info("socket")
        if s is not None:
            try:
                s.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            except OSError:
                pass
        w.close()

    async def _conn(self, cr, cw):
        self.stats["connections"] += 1
        cid = self.stats["connections"]
        try:
            ur, uw = await asyncio.open_connection("127.0.0.1", self.up)
        except OSError:
            self._rst(cw)
            return
        st = dict(resp_seen=True, cut=False, drop_resp=False)
        cut_at = self.rng.randint(0, 4 << 20) if self.mode == "conn" and cid % self.every == 0 else None

        def cut(kind):
            if not st["cut"]:
                st["cut"] = True
                self.stats["cuts"] += 1
                self.stats["cuts_" + kind] = self.stats.get("cuts_" + kind, 0) + 1
            self._rst(cw)
            self._rst(uw)

        async def c2u():
            try:
                while not st["cut"]:
                    b = await cr.read(65536)
                    if not b:
                        break
                    if self.mode == "req" and st["resp_seen"]:
                        st["resp_seen"] = False
                        self.stats["requests"] += 1
                        if self.stats["requests"] % self.every == 0:
                            if self.rng.random() < 0.5:
                                cut("before")
                                return
                            st["drop_resp"] = True
                    uw.write(b)
                    await uw.drain()
            except (OSError, ConnectionError):
                pass
            finally:
                if not st["cut"]:
                    try:
                        uw.write_eof()
                    except (OSError, RuntimeError):
                        pass

        async def u2c():
            sent = 0
            try:
                while not st["cut"]:
                    b = await ur.read(65536)
                    if not b:
                        break
                    if st["drop_resp"]:
                        cut("after")
                        return
                    if cut_at is not None and sent + len(b) > cut_at:
                        cw.write(b[:max(0, cut_at - sent)])
                        cut("after")
                        return
                    st["resp_seen"] = True
                    sent += len(b)
                    cw.write(b)
                    await cw.drain()
            except (OSError, ConnectionError):
                pass
            finally:
                if not st["cut"]:
                    cw.close()

        await asyncio.gather(c2u(), u2c())
        for w in (cw, uw):
            try:
                w.close()
            except Exception:
                pass


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("--docs", type=int)
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "box"))
    ap.add_argument("--only", default="cut,storm,flood,restart")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--run-id")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    only = set(a.only.split(","))
    rng = random.Random(a.seed)

    tiers = tomllib.load(open(os.path.join(HERE, "scenarios", "tiers.toml"), "rb"))
    scen = {"name": "faults", "seed": [{"index": "load-logs", "profile": "logs", "seed": 1, "docs": "tier"}],
            "step": [{"kind": "dump"}, {"kind": "restore"}, {"kind": "restore"}]}
    ok, why, plan = R.fit(a.tier, a.place, tiers, scen, a.docs)
    if not ok:
        print(f"FAULTS-SUMMARY tier={a.tier} place={a.place} REFUSED: {why}")
        return 2
    lock = open("/tmp/searchdump-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        fcntl.flock(lock, fcntl.LOCK_EX)
    storm_heap = 1024
    need_mem = plan["heap_mb"] * 2 + 512 + plan["place"]["tool_mem_mb"] + 256 + \
        (storm_heap * 2 + 512 if "storm" in only else 0)
    if R.meminfo_avail_mb() < need_mem:
        print(f"FAULTS-SUMMARY tier={a.tier} SKIPPED (MemAvailable {R.meminfo_avail_mb()} MB < {need_mem} MB)")
        return 3
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-faults-{a.tier}"
    work = os.path.join(os.environ.get("LOAD_WORK", "/var/tmp/searchdump-load"), run_id)
    os.makedirs(work, mode=0o700, exist_ok=True)
    resdir = os.path.join(ROOT, "results", "load", run_id)
    os.makedirs(resdir, exist_ok=True)
    edgen, edverify, tool = R.tool_path("edgen"), R.tool_path("edverify"), R.tool_path("searchdump")
    base = {}
    for line in subprocess.run([os.path.join(ROOT, "scripts/it/matrix.sh"), a.engine],
                               check=True, capture_output=True, text=True).stdout.splitlines():
        k, v = line.split("=", 1)
        base[k] = v
    up_profile = base.pop("UP_PROFILE")
    heap = f"{plan['heap_mb']}m"

    def node_env(tag, heap_s, port, extra=""):
        d = os.path.join(work, "it-" + tag)
        e = dict(os.environ, **base, IT_DIR=d, IT_LOCK_HELD="1", RUN_ID=f"load{os.getpid()}{tag}",
                 IT_ES_DATA_MOUNT="1", ES_HEAP=heap_s, OS_HEAP=heap_s, PORT=str(port),
                 IT_CONTAINER_MEM=plan["place"]["container_mem"], IT_ES_EXTRA_SETTINGS=extra)
        return e

    env = node_env("main", heap, 19202)
    res = dict(schema="edrs-faults/v1", run_id=run_id, tier=a.tier, place=a.place, engine=a.engine, heap=heap,
               seed=a.seed, limit=LIMIT, concurrency=CONC,
               git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                  capture_output=True, text=True).stdout.strip(),
               tool=subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.strip(),
               started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"), runs=[], failures=[])
    envs = [env]
    rc = 1
    try:
        log(f"provisioning {a.engine} (heap {heap})")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        ce = R.sh_env(os.path.join(env["IT_DIR"], "env"))
        url = ce["IT_ES_URL"]
        name = f"it-es-plain-{env['RUN_ID']}"
        port = int(url.rsplit(":", 1)[1])
        cl = R.Cluster(url)
        res["engine_version"] = cl.req("GET", "/")["version"]["number"]
        s = dict(plan["seeds"][0])
        log(f"seeding load-logs: {s['docs']} docs")
        res["seed_index"] = R.seed_index(cl, edgen, s, R.engine_mapping_flavour(a.engine))
        n = res["seed_index"]["stored"]
        flavour = R.engine_mapping_flavour(a.engine)
        mapping = json.loads(subprocess.run([edgen, "mapping", "--profile", "logs", "--engine", flavour],
                                            check=True, capture_output=True).stdout)

        def diff(tag, a_src, b_src):
            rep = os.path.join(resdir, f"verify-{tag}.json")
            p = subprocess.run([edverify, "diff", "--a", a_src, "--b", b_src, "--index-as", "x", "--out", rep],
                               capture_output=True, text=True)
            v = json.load(open(rep)) if os.path.exists(rep) else {}
            return dict(ok=p.returncode == 0, a=v.get("a", {}).get("count"), b=v.get("b", {}).get("count"),
                        detail=None if p.returncode == 0 else (p.stdout + p.stderr)[-400:])

        def wait_up(c, timeout=300):
            t0 = time.time()
            while time.time() - t0 < timeout:
                try:
                    if c.req("GET", "/_cluster/health?wait_for_status=yellow&timeout=5s")["status"] in ("yellow", "green"):
                        return round(time.time() - t0, 1)
                except Exception:
                    pass
                time.sleep(0.5)
            raise RuntimeError("node not back after restart")

        def target(c, idx):
            c.req("DELETE", "/" + idx, ok404=True)
            c.req("PUT", "/" + idx, mapping)

        def count(c, idx):
            try:
                c.req("POST", f"/{idx}/_refresh")
                return c.req("GET", f"/{idx}/_count")["count"]
            except Exception:
                return None

        ref = os.path.join(work, "ref.ndjson")
        p = subprocess.run([tool, "--input", f"{url}/load-logs", "--output", ref, "--type=data", f"--limit={LIMIT}"],
                           capture_output=True, text=True)
        t_ref = None
        res["reference_dump"] = dict(exit=p.returncode, verify=diff("ref", f"{url}/load-logs", ref))
        if p.returncode or not res["reference_dump"]["verify"]["ok"]:
            raise RuntimeError(f"reference dump failed: {p.stderr[-400:]}")

        def classify(row, err):
            lines = [x for x in err.strip().splitlines() if x.strip()]
            row["stderr_tail"] = lines[-6:]
            panic = "panicked at" in err or "RUST_BACKTRACE" in err
            m = re.findall(r"Failed Writes:?\s*(\d+)", err)
            row["failed_writes"] = int(m[-1]) if m else None
            if row["exit"] == 0:
                row["outcome"] = "complete" if row["verify"]["ok"] else "SILENT_LOSS"
            elif panic or not lines:
                row["outcome"] = "UNCLEAR_FAILURE"
            else:
                row["outcome"] = "failed_with_message"
            if row.get("final_file_left"):
                row["outcome"] = "FAILED_DUMP_LEFT_FILE"
            if row["outcome"].isupper():
                res["failures"].append(f"{row['case']}: {row['outcome']}: {row['stderr_tail'][-2:]}")
                log("FAIL", row["case"], row["outcome"])

        def run(case, op, in_url, out_c, out_url, fault=None, extra=(), verify_url=None):
            """op restore|copy|dump. fault(proc, progress) runs in a thread and injects the fault."""
            if op == "dump":
                out = os.path.join(work, f"{case}.ndjson")
                args = ["--input", f"{in_url}/load-logs", "--output", out, "--type=data", f"--limit={LIMIT}"]
            else:
                target(out_c, "dst")
                src = ref if op == "restore" else f"{in_url}/load-logs"
                args = ["--input", src, "--output", f"{out_url}/dst", "--type=data", f"--limit={LIMIT}",
                        f"--concurrency={CONC}"]
            args += list(extra)
            log(f"{case}: {op} start")
            t0 = time.time()
            errf = open(os.path.join(work, f"{case}.err"), "w+")
            pr = subprocess.Popen([tool, *args], stdout=subprocess.DEVNULL, stderr=errf, text=True)
            info = {}
            th = None
            if fault:
                def progress():
                    if op == "dump":
                        parts = [f for f in os.listdir(work) if f.startswith(f".{case}.ndjson.part")]
                        try:
                            return os.path.getsize(os.path.join(work, parts[0])) / os.path.getsize(ref) * n \
                                if parts else 0
                        except OSError:
                            return 0
                    try:
                        return out_c.req("GET", "/dst/_count")["count"]
                    except Exception:
                        return 0
                th = threading.Thread(target=fault, args=(pr, progress, info), daemon=True)
                th.start()
            try:
                pr.wait(timeout=3600)
            except subprocess.TimeoutExpired:
                pr.kill()
                pr.wait()
                info["timeout"] = True
            if th:
                th.join(600)
            errf.seek(0)
            err = errf.read()
            row = dict(case=case, op=op, exit=pr.returncode, wall_s=round(time.time() - t0, 1), fault=info,
                       args=[re.sub(r"http://127\.0\.0\.1:\d+", "$NODE", x) for x in args if not x.startswith(work)])
            if op == "dump":
                row["final_file_left"] = pr.returncode != 0 and os.path.exists(out)
                row["verify"] = diff(case, f"{url}/load-logs", out) if pr.returncode == 0 else dict(ok=False)
                for f in os.listdir(work):
                    if f.startswith(f"{case}.ndjson") or f.startswith(f".{case}.ndjson"):
                        os.remove(os.path.join(work, f))
            else:
                if fault:
                    wait_up(out_c)
                row["target_count"] = count(out_c, "dst")
                row["verify"] = diff(case, f"{url}/load-logs", f"{verify_url or out_url}/dst") if pr.returncode == 0 \
                    else dict(ok=False, b=row["target_count"])
            classify(row, err)
            log(f"{case}: exit {row['exit']} {row['outcome']} ({row['wall_s']} s) {row['stderr_tail'][-1:] }")
            res["runs"].append(row)
            json.dump(res, open(os.path.join(resdir, "faults.json"), "w"), indent=1)
            return row

        def at(frac, then):
            """Fault helper: wait until progress >= frac * n, then call then(info)."""
            def f(pr, progress, info):
                kat = int(n * frac)
                while pr.poll() is None and progress() < kat:
                    time.sleep(0.05)
                if pr.poll() is not None:
                    info["fired"] = False
                    return
                info["fired"] = True
                info["at_docs"] = int(progress())
                then(info)
            return f

        # 1. connection and request cuts through the proxy
        if "cut" in only:
            # conn/50 is the literal spec; a run opens only ~1-15 pooled keep-alive connections, so it rarely
            # fires - conn/4 makes connection resets mid-response happen, req/50 cuts 1 in 50 requests.
            for mode, every in (("conn", 50), ("conn", 4), ("req", 50)):
                px = CutProxy(port, mode, every, a.seed)
                purl = f"http://127.0.0.1:{px.port}"
                for op, extra in (("restore", ()), ("copy", ()), ("dump", ()), ("copy-pit", ("--searchType=pit",)),
                                  ("dump-pit", ("--searchType=pit",))):
                    before = dict(px.stats)
                    row = run(f"cut-{mode}{'' if every == 50 else every}-{op}", op.split("-")[0], purl, cl, purl, extra=extra, verify_url=url)
                    row["proxy"] = {k: (v - before[k] if isinstance(v, int) and k != "every" else v)
                                    for k, v in px.stats.items()}
                    log(f"  proxy {row['proxy']}")
                px.stop()

        # 2. 429 storm: a second node with a one-thread, one-slot write pool
        if "storm" in only:
            senv = node_env("storm", f"{storm_heap}m", 19203,
                            "thread_pool.write.size=1 thread_pool.write.queue_size=1")
            envs.append(senv)
            log("provisioning the storm node (thread_pool.write.size=1, queue_size=1)")
            subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=senv, check=True,
                           stdout=subprocess.DEVNULL)
            surl = R.sh_env(os.path.join(senv["IT_DIR"], "env"))["IT_ES_URL"]
            sc = R.Cluster(surl)
            tp = sc.req("GET", "/_nodes/thread_pool")["nodes"]
            res["storm_node_write_pool"] = [v["thread_pool"]["write"] for v in tp.values()]
            for op in ("restore", "copy"):
                run(f"storm-{op}", op, url, sc, surl)
            run("storm-restore-retry20", "restore", url, sc, surl, extra=["--retryAttempts=20"])
            stats = sc.req("GET", "/_nodes/stats/thread_pool")["nodes"]
            res["storm_rejected_total"] = sum(v["thread_pool"]["write"]["rejected"] for v in stats.values())
            subprocess.run(["podman", "rm", "-f", f"it-es-plain-{senv['RUN_ID']}"], stdout=subprocess.DEVNULL)
            subprocess.run((["rm", "-rf"] if os.geteuid() == 0 else ["podman", "unshare", "rm", "-rf"]) + [senv["IT_DIR"]])

        # 3. flood-stage block for 30 s
        def flood(info):
            # Flood stage as ES 7.17 applies it: watermarks to 1% (so the disk monitor agrees) and the
            # index.blocks.read_only_allow_delete block on dst, set directly so it lands at the chosen
            # point instead of up to cluster.info.update.interval (min 10s) later. Held FLOOD_S, then lifted.
            keys = ["low", "high", "flood_stage"]
            t0 = time.time()
            cl.req("PUT", "/_cluster/settings", {"persistent": {
                f"cluster.routing.allocation.disk.watermark.{k}": "1%" for k in keys}})
            cl.req("PUT", "/dst/_settings", {"index.blocks.read_only_allow_delete": True})
            info["block_applied_after_s"] = round(time.time() - t0, 1)
            time.sleep(FLOOD_S)
            cl.req("PUT", "/_cluster/settings", {"persistent": {
                f"cluster.routing.allocation.disk.watermark.{k}": None for k in keys}})
            for i in ("dst", "load-logs"):
                cl.req("PUT", f"/{i}/_settings", {"index.blocks.read_only_allow_delete": None})
            info["blocked_s"] = round(time.time() - t0, 1)

        if "flood" in only:
            for op in ("restore", "copy"):
                run(f"flood-{op}", op, url, cl, url, fault=at(rng.uniform(0.2, 0.4), flood))
            run("flood-restore-retry10", "restore", url, cl, url, fault=at(rng.uniform(0.2, 0.4), flood),
                extra=["--retryAttempts=10"])

        # 4. podman restart of the node mid-run
        def restart(info):
            t0 = time.time()
            p = subprocess.run(["podman", "restart", "-t", "10", name], capture_output=True, text=True)
            info["podman_restart_exit"] = p.returncode
            # Rootless podman + pasta on the box: the restart's start half often fails with "Failed to bind
            # port ... Address already in use" because the old pasta has not released the port yet. The node
            # is then stopped, so start it again (counted) - the fault is the same: node down, then back.
            n = 0
            while p.returncode != 0 and n < 60:
                info["restart_error"] = (p.stderr or "").strip().splitlines()[-1:]
                time.sleep(1)
                n += 1
                p = subprocess.run(["podman", "start", name], capture_output=True, text=True)
            info["start_retries"] = n
            info["down_s"] = round(time.time() - t0 + wait_up(cl), 1)

        if "restart" in only:
            for op, extra in (("restore", ()), ("copy", ()), ("dump", ()), ("copy-pit", ("--searchType=pit",)),
                              ("dump-pit", ("--searchType=pit",))):
                run(f"restart-{op}", op.split("-")[0], url, cl, url, fault=at(rng.uniform(0.2, 0.6), restart),
                    extra=extra)
            run("restart-restore-retry20", "restore", url, cl, url, fault=at(rng.uniform(0.2, 0.6), restart),
                extra=["--retryAttempts=20"])

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
        out = os.path.join(resdir, "faults.json")
        json.dump(res, open(out, "w"), indent=1)
        if not a.keep:
            for e in envs:
                subprocess.run(["podman", "rm", "-f", f"it-es-plain-{e['RUN_ID']}"], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL)
            subprocess.run((["rm", "-rf"] if os.geteuid() == 0 else ["podman", "unshare", "rm", "-rf"]) + [work])
    outc = {}
    for r in res["runs"]:
        outc[r["outcome"]] = outc.get(r["outcome"], 0) + 1
    print(f"FAULTS-SUMMARY tier={a.tier} engine={a.engine} docs={res.get('seed_index', {}).get('stored')} "
          f"{'PASS' if rc == 0 else 'FAIL'} runs={len(res['runs'])} {outc} result={os.path.relpath(out, ROOT)}")
    for f in res["failures"]:
        print("  failure:", f)
    return rc


if __name__ == "__main__":
    sys.exit(main())
