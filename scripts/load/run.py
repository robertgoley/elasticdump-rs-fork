#!/usr/bin/env python3
"""Load runner (TST-LOAD-001..003, LOAD-012, LOAD-017).

    scripts/load/run.sh <tier> <scenario> [--place box|runner|big] [--engine es717-plain]
                        [--run-id ID] [--docs N] [--keep] [--dry-run]

The runner does the following, in order:

1. Refuses a tier/place combination that does not fit (scenarios/tiers.toml).
2. Takes the shared IT/load lock and checks free memory and disk.
3. Provisions ONE engine through scripts/it/up.sh.
4. Creates each seed index with `edgen mapping` and seeds it from `edgen gen --format bulk` through the harness's own `_bulk` client. Edge-case documents that the engine rejects are counted, not failed.
5. Takes a reference digest per index with `edverify` in cluster mode.
6. Runs the scenario steps (dump / restore / copy) with elasticdump-rs under measure.sh, and checks each against the reference with `edverify diff`.
7. Leak check: no open scroll/PIT contexts, no `.part` files, no unexpected indices.
8. Tears down. Results go to results/load/<run-id>/<scenario>.json, recording the seeds, image, tool version and per-step metrics.

Exit codes: 0 = pass, 1 = fail, 2 = refused (tier does not fit the place), 3 = skipped (resource guard; exit 1 with IT_REQUIRED=1).
"""
import argparse, base64, datetime, fcntl, glob, json, os, shutil, ssl, subprocess, sys, time, tomllib
import urllib.error, urllib.parse, urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
HERE = os.path.dirname(os.path.abspath(__file__))
# Average uncompressed bytes per document, used for the disk estimate.
AVG_DOC = {"logs": 1000, "metrics": 400, "nested": 1500, "geo": 300, "wide": 30000,
           "huge": 3 << 20, "edge": 400}
DUMP_FACTOR = {"none": 1.0, "gzip": 0.2, "zstd": 0.15}
INDEX_FACTOR = 0.8  # on-disk index size / raw source size (calibrate from seeds[].store_ratio)


def log(*a):
    print("load:", *a, file=sys.stderr, flush=True)


def tool_path(name):
    for d in ("release", "it", "debug"):
        p = os.path.join(ROOT, "target", d, name)
        if os.path.exists(p):
            return p
    sys.exit(f"load: {name} not built (cargo build --release -p {name} / --profile it)")


class Cluster:
    def __init__(self, url, user=None, pw=None, ca=None):
        self.url = url.rstrip("/")
        self.auth = None
        if user:
            self.auth = "Basic " + base64.b64encode(f"{user}:{pw}".encode()).decode()
        self.ctx = None
        if self.url.startswith("https"):
            self.ctx = ssl.create_default_context(cafile=ca) if ca else ssl._create_unverified_context()

    def req(self, method, path, body=None, ctype="application/json", ok404=False):
        data = body if isinstance(body, (bytes, type(None))) else json.dumps(body).encode()
        r = urllib.request.Request(self.url + path, data=data, method=method)
        if data is not None:
            r.add_header("Content-Type", ctype)
        if self.auth:
            r.add_header("Authorization", self.auth)
        try:
            with urllib.request.urlopen(r, context=self.ctx, timeout=300) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            if ok404 and e.code == 404:
                return None
            raise RuntimeError(f"{method} {path}: HTTP {e.code}: {e.read()[:500]!r}") from None
        return json.loads(raw) if raw else None


def fit(tier_name, place_name, tiers, scenario, docs_override):
    """Return (ok, reason, plan) for this tier/place/scenario."""
    place = tiers["place"][place_name]
    tier = tiers["tier"].get(tier_name)
    if tier is None:
        return False, f"unknown tier {tier_name}", None
    if tier_name not in place["tiers"]:
        return False, f"tier {tier_name} is not allowed on place '{place_name}' (allowed: {place['tiers']})", None
    if tier["heap_mb"] > place["max_heap_mb"]:
        return False, f"heap {tier['heap_mb']} MB > place max {place['max_heap_mb']} MB", None
    seeds = []
    for s in scenario.get("seed", []):
        n = s["docs"]
        if n == "tier":
            n = docs_override or tier["docs"]
        elif n == "tier.edge_docs":
            n = tier.get("edge_docs", 0)
        if n:
            seeds.append(dict(s, docs=int(n)))
    allowed = tier.get("compress")
    raw = sum(s["docs"] * AVG_DOC.get(s["profile"], 1000) for s in seeds)
    dump_bytes, copies = 0.0, 1  # the source index
    for st in scenario.get("step", []):
        c = st.get("compress", "none")
        if st["kind"] == "dump":
            if allowed and c not in allowed:
                return False, f"tier {tier_name} only allows compression {allowed} (step uses {c})", None
            dump_bytes += raw * DUMP_FACTOR[c]
        if st["kind"] in ("restore", "copy"):
            copies += 1
    need_mb = int((raw * INDEX_FACTOR * copies + dump_bytes) / 1e6) + 500
    if need_mb > place["max_disk_mb"]:
        return False, f"estimated disk {need_mb} MB > place budget {place['max_disk_mb']} MB", None
    return True, "", dict(seeds=seeds, need_disk_mb=need_mb, heap_mb=tier["heap_mb"], place=place)


def meminfo_avail_mb():
    for line in open("/proc/meminfo"):
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) // 1024
    return 0


def disk_free_mb(path):
    os.makedirs(path, exist_ok=True)
    st = os.statvfs(path)
    return st.f_bavail * st.f_frsize // (1 << 20)


def sh_env(path):
    """Read the exports of up.sh's env file without echoing them."""
    out = subprocess.run(["bash", "-c", 'set -a; source "$1"; env -0', "_", path],
                         check=True, capture_output=True).stdout
    return dict(kv.split("=", 1) for kv in out.decode().split("\0") if "=" in kv)


def engine_mapping_flavour(engine):
    if engine.startswith("os13") or engine.startswith("opensearch1"):
        return "os1"
    if engine.startswith("os") or engine.startswith("opensearch"):
        return "os"
    return "es"


def seed_index(cl, edgen, s, flavour):
    m = json.loads(subprocess.run([edgen, "mapping", "--profile", s["profile"], "--engine", flavour],
                                  check=True, capture_output=True).stdout)
    cl.req("PUT", "/" + s["index"], m)
    p = subprocess.Popen([edgen, "gen", "--profile", s["profile"], "--seed", str(s["seed"]),
                          "--count", str(s["docs"]), "--format", "bulk", "--index", s["index"]],
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    batch, n, rejected, reasons = [], 0, 0, {}
    t0 = time.time()

    def flush():
        nonlocal rejected
        if not batch:
            return
        try:
            items = [cl.req("POST", "/_bulk", b"".join(batch), "application/x-ndjson")]
        except RuntimeError as e:
            if "HTTP 400" not in str(e):
                raise
            # A request-level validation error (e.g. the >512-byte _id edge case)
            # rejects the whole batch: resend pair by pair to isolate it.
            items = []
            for i in range(0, len(batch), 2):
                try:
                    items.append(cl.req("POST", "/_bulk", b"".join(batch[i:i + 2]), "application/x-ndjson"))
                except RuntimeError as e2:
                    if "HTTP 400" not in str(e2):
                        raise
                    rejected += 1
                    reasons["request_400"] = reasons.get("request_400", 0) + 1
        for r in items:
            if not r.get("errors"):
                continue
            for it in r["items"]:
                res = next(iter(it.values()))
                if res.get("status", 200) >= 300:
                    rejected += 1
                    t = (res.get("error") or {}).get("type", str(res.get("status")))
                    reasons[t] = reasons.get(t, 0) + 1
        batch.clear()

    size = 0
    for line in p.stdout:
        batch.append(line)
        size += len(line)
        n += 1
        if n % 2 == 0 and (len(batch) >= 10000 or size > 8 << 20):
            flush()
            size = 0
    flush()
    if p.wait() != 0:
        raise RuntimeError("edgen gen failed")
    cl.req("POST", f"/{s['index']}/_refresh")
    count = cl.req("GET", f"/{s['index']}/_count")["count"]
    cl.req("POST", f"/{s['index']}/_flush")
    store = cl.req("GET", f"/{s['index']}/_stats/store")["_all"]["primaries"]["store"]["size_in_bytes"]
    return dict(index=s["index"], profile=s["profile"], seed=s["seed"], generated=s["docs"],
                stored=count, rejected=rejected, store_bytes=store,
                # same-_id edge cases overwrite each other (not an error)
                overwritten=s["docs"] - count - rejected,
                store_ratio=round(store / max(1, s["docs"] * AVG_DOC.get(s["profile"], 1000)), 3), reject_reasons=reasons,
                seconds=round(time.time() - t0, 2), mapping_engine=flavour)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("scenario")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "box"))
    ap.add_argument("--engine", default=None)
    ap.add_argument("--run-id", default=None)
    ap.add_argument("--docs", type=int, default=None, help="override the tier's doc count (smoke runs)")
    ap.add_argument("--keep", action="store_true", help="keep the cluster and work dir")
    ap.add_argument("--dry-run", action="store_true", help="only print the fit decision")
    a = ap.parse_args()

    tiers = tomllib.load(open(os.path.join(HERE, "scenarios", "tiers.toml"), "rb"))
    spath = a.scenario if os.path.exists(a.scenario) else os.path.join(HERE, "scenarios", a.scenario + ".toml")
    scenario = tomllib.load(open(spath, "rb"))
    engine = a.engine or scenario.get("engine", "es717-plain")
    if a.place not in tiers["place"]:
        sys.exit(f"load: unknown place {a.place}")
    ok, why, plan = fit(a.tier, a.place, tiers, scenario, a.docs)
    if not ok:
        print(f"LOAD-SUMMARY scenario={scenario['name']} tier={a.tier} place={a.place} REFUSED: {why}")
        return 2
    log(f"fit: tier={a.tier} place={a.place} engine={engine} est_disk={plan['need_disk_mb']}MB heap={plan['heap_mb']}MB")
    if a.dry_run:
        print(json.dumps(dict(scenario=scenario["name"], tier=a.tier, place=a.place, engine=engine,
                              seeds=plan["seeds"], need_disk_mb=plan["need_disk_mb"]), indent=1))
        return 0

    # Guards: lock, memory and disk (TST-INF-007).
    lock = open("/tmp/elasticdump-rs-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            log("waiting for the IT/load lock (/tmp/elasticdump-rs-it.lock)")
            fcntl.flock(lock, fcntl.LOCK_EX)
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-{a.tier}"
    work = os.path.join(os.environ.get("LOAD_WORK", "/var/tmp/elasticdump-rs-load"), run_id)
    need_mem = plan["heap_mb"] * 2 + 512 + plan["place"]["tool_mem_mb"] + 256
    skip = None
    if meminfo_avail_mb() < need_mem:
        skip = f"MemAvailable {meminfo_avail_mb()} MB < {need_mem} MB"
    elif disk_free_mb(os.path.dirname(work)) < plan["need_disk_mb"]:
        skip = f"free disk {disk_free_mb(os.path.dirname(work))} MB < {plan['need_disk_mb']} MB"
    if skip:
        print(f"LOAD-SUMMARY scenario={scenario['name']} tier={a.tier} SKIPPED (resource guard: {skip})")
        return 1 if os.environ.get("IT_REQUIRED") == "1" else 3

    edgen, edverify, tool = tool_path("edgen"), tool_path("edverify"), tool_path("elasticdump-rs")
    os.makedirs(work, mode=0o700, exist_ok=True)
    it_dir = os.path.join(os.environ.get("XDG_RUNTIME_DIR") or os.path.expanduser("~/.cache"), "elasticdump-rs-load")
    env = dict(os.environ, IT_DIR=it_dir, IT_LOCK_HELD="1")
    for line in subprocess.run([os.path.join(ROOT, "scripts/it/matrix.sh"), engine],
                               check=True, capture_output=True, text=True).stdout.splitlines():
        k, v = line.split("=", 1)
        env[k] = v
    up_profile = env.pop("UP_PROFILE")
    heap = f"{plan['heap_mb']}m"
    env.update(ES_HEAP=heap, OS_HEAP=heap, IT_CONTAINER_MEM=plan["place"]["container_mem"])

    result = dict(schema="edrs-load/v1", run_id=run_id, scenario=scenario["name"], tier=a.tier,
                  place=a.place, engine=engine, up_profile=up_profile,
                  image=env.get("ES_IMAGE") or env.get("OS_IMAGE"), heap=heap,
                  git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                     capture_output=True, text=True).stdout.strip(),
                  tool=subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.strip(),
                  started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
                  seeds=[], steps=[], leak={}, failures=[])
    resdir = os.path.join(ROOT, "results", "load", run_id)
    os.makedirs(resdir, exist_ok=True)
    rc = 1
    try:
        log(f"provisioning {engine} ({up_profile}, heap {heap})")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        ce = sh_env(os.path.join(it_dir, "env"))
        url, user, pw, ca = ce["IT_ES_URL"], ce.get("IT_ADMIN_USER"), ce.get("IT_ADMIN_PW"), ce.get("IT_CA")
        cl = Cluster(url, user, pw, ca)
        result["engine_version"] = cl.req("GET", "/")["version"]["number"]
        # Credentials reach the tools through env only, never argv (measure.py records argv).
        tenv = dict(os.environ)
        if user:
            tenv.update(EDV_USER=user, EDV_PASSWORD=pw,
                        ELASTICDUMP_INPUT_USERNAME=user, ELASTICDUMP_INPUT_PASSWORD=pw,
                        ELASTICDUMP_OUTPUT_USERNAME=user, ELASTICDUMP_OUTPUT_PASSWORD=pw)
        tls_tool = ["--ca", ca] if ca and url.startswith("https") else []
        tls_edv = ["--ca", ca] if ca and url.startswith("https") else []
        flavour = engine_mapping_flavour(engine)
        before = {i["index"] for i in cl.req("GET", "/_cat/indices?format=json&expand_wildcards=all")}

        for s in plan["seeds"]:
            log(f"seeding {s['index']}: {s['docs']} {s['profile']} docs (seed {s['seed']})")
            result["seeds"].append(seed_index(cl, edgen, s, flavour))
        expected = {s["index"] for s in plan["seeds"]}

        def verify(name, a_src, b_src, extra=()):
            rep = os.path.join(resdir, f"verify-{name}.json")
            p = subprocess.run([edverify, "diff", "--a", a_src, "--b", b_src, "--index-as", "x",
                                "--out", rep, *tls_edv, *extra], env=tenv, capture_output=True, text=True)
            v = json.load(open(rep)) if os.path.exists(rep) else {}
            ok = p.returncode == 0
            if not ok:
                result["failures"].append(f"verify {name}: {p.stdout.strip()[-2000:]} {p.stderr.strip()[-400:]}")
            return dict(name=name, ok=ok, report=os.path.relpath(rep, ROOT),
                        a_count=v.get("a", {}).get("count"), b_count=v.get("b", {}).get("count"))

        def run_tool(name, args, docs, outputs=()):
            mout = os.path.join(resdir, f"metrics-{name}.json")
            cmd = [os.path.join(HERE, "measure.sh"), "--run-id", f"{run_id}-{name}", "--out", mout,
                   "--cluster", url, "--docs", str(docs), "--label", f"step={name}",
                   "--label", f"tier={a.tier}", "--label", f"engine={engine}"]
            for o in outputs:
                cmd += ["--output", o]
            p = subprocess.run(cmd + ["--", tool, *args, *tls_tool], env=tenv, cwd=work)
            m = json.load(open(mout)) if os.path.exists(mout) else {}
            val = subprocess.run([os.path.join(HERE, "measure.py"), "--validate", mout], capture_output=True, text=True)
            if val.returncode != 0:  # LOAD-003: every field present, schema-valid
                result["failures"].append(f"{name}: metrics schema: {val.stdout.strip()[-400:]}")
            if p.returncode != 0:
                result["failures"].append(f"{name}: exit {p.returncode}")
            if m.get("failed_writes"):
                result["failures"].append(f"{name}: failed_writes={m['failed_writes']}")
            if m.get("total_writes") is not None and m["total_writes"] != docs:
                result["failures"].append(f"{name}: total_writes={m['total_writes']} != {docs}")
            rss = m.get("max_rss_kb") or 0
            if rss > plan["place"]["tool_mem_mb"] * 1024:
                result["failures"].append(f"{name}: max RSS {rss} KB > {plan['place']['tool_mem_mb']} MB budget")
            return dict(name=name, exit=p.returncode, metrics=os.path.relpath(mout, ROOT),
                        wall_s=m.get("wall_s"), max_rss_kb=rss, docs_per_s=m.get("docs_per_s"),
                        retries=m.get("retries"))

        for s in result["seeds"]:
            idx, src = s["index"], f"{url}/{s['index']}"
            dumped = None
            for st in scenario.get("step", []):
                kind, extra = st["kind"], st.get("args", [])
                if kind == "dump":
                    c = st.get("compress", "none")
                    f = os.path.join(work, idx + ".ndjson" + {"none": "", "gzip": ".gz", "zstd": ".zst"}[c])
                    cflags = {"none": [], "gzip": ["--compressionType=gzip"], "zstd": ["--compressionType=zstd"]}[c]
                    r = run_tool(f"dump-{idx}", ["--input", src, "--output", f, "--type=data", *cflags, *extra],
                                 s["stored"], [f])
                    r["verify"] = verify(f"dump-{idx}", src, f)
                    dumped = f
                elif kind in ("restore", "copy"):
                    dst = f"{idx}-{kind}"
                    m = json.loads(subprocess.run([edgen, "mapping", "--profile", s["profile"], "--engine", flavour],
                                                  check=True, capture_output=True).stdout)
                    cl.req("PUT", "/" + dst, m)
                    expected.add(dst)
                    inp = dumped if kind == "restore" else src
                    if inp is None:
                        raise RuntimeError("restore step needs an earlier dump step")
                    r = run_tool(f"{kind}-{idx}", ["--input", inp, "--output", f"{url}/{dst}", "--type=data", *extra],
                                 s["stored"])
                    cl.req("POST", f"/{dst}/_refresh")
                    r["verify"] = verify(f"{kind}-{idx}", src, f"{url}/{dst}")
                else:
                    raise RuntimeError(f"unknown step kind {kind}")
                result["steps"].append(r)

        # LOAD-017 leak check
        deadline, scroll, pit = time.time() + 15, None, None
        while True:
            ns = cl.req("GET", "/_nodes/stats/indices/search")["nodes"]
            scroll = sum(n["indices"]["search"].get("scroll_current", 0) for n in ns.values())
            pit = sum(n["indices"]["search"].get("point_in_time_current", 0) for n in ns.values())
            if (scroll == 0 and pit == 0) or time.time() > deadline:
                break
            time.sleep(1)
        parts = [os.path.relpath(p, work) for p in glob.glob(os.path.join(work, "**", "*.part*"), recursive=True)]
        after = {i["index"] for i in cl.req("GET", "/_cat/indices?format=json&expand_wildcards=all")}
        extra_idx = sorted(i for i in after - before - expected if not i.startswith("."))
        result["leak"] = dict(scroll_current=scroll, pit_current=pit, part_files=parts, unexpected_indices=extra_idx,
                              new_system_indices=sorted(i for i in after - before - expected if i.startswith(".")))
        if scroll or pit or parts or extra_idx:
            result["failures"].append(f"leak: {result['leak']}")
        rc = 0 if not result["failures"] else 1
    except Exception as e:  # report, then tear down
        result["failures"].append(f"error: {e}")
        rc = 1
    finally:
        result["finished"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
        result["pass"] = rc == 0
        out = os.path.join(resdir, scenario["name"] + ".json")
        with open(out, "w") as f:
            json.dump(result, f, indent=1)
        if not a.keep:
            subprocess.run([os.path.join(ROOT, "scripts/it/down.sh")], env=env)
            shutil.rmtree(work, ignore_errors=True)
            shutil.rmtree(it_dir, ignore_errors=True)
        else:
            log(f"kept: work={work} cluster env={it_dir}/env")
    steps = " ".join(f"{s['name']}={'ok' if s.get('verify', {}).get('ok') and not s['exit'] else 'FAIL'}"
                     f"({s['docs_per_s']}d/s,{s['max_rss_kb']}KB)" for s in result["steps"])
    print(f"LOAD-SUMMARY scenario={scenario['name']} tier={a.tier} engine={engine} "
          f"{'PASS' if rc == 0 else 'FAIL'} {steps} result={os.path.relpath(out, ROOT)}")
    for f in result["failures"]:
        print("  failure:", f)
    return rc


if __name__ == "__main__":
    sys.exit(main())
