#!/usr/bin/env python3
"""L-811 self-test of run.leak_check (LOAD-017), the teardown assertion every load script calls.

Offline part (always): a stub cluster without the PIT stat, `.part` detection incl. hidden staging
files, the `it`/`it-*` skip and `allow_part`. With --engine (default es717-plain; opensearch-plain checks the PIT stat): provisions one small
container and checks that an open scroll, an open PIT, a stray index and a hidden index are each
reported, and that the check passes again once they are gone. No load; about a minute.

  scripts/load/leakcheck_selftest.py [--offline] [--engine es717-plain] [--keep]
"""
import argparse, datetime, fcntl, json, os, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
import run as R  # noqa: E402

FAILS = []


def check(name, cond, detail=""):
    print(f"{'ok  ' if cond else 'FAIL'} {name} {detail}".rstrip())
    if not cond:
        FAILS.append(name)


class Stub:
    def __init__(self, search):
        self.search = search

    def req(self, method, path, *a, **k):
        return {"nodes": {"n1": {"indices": {"search": self.search}}}}


def offline():
    w = tempfile.mkdtemp(prefix="leakcheck-")
    try:
        leak, f = R.leak_check(Stub({"scroll_current": 0}), w, wait_s=0)
        check("stub: no PIT stat -> pit_current None, pass", leak["pit_current"] is None and not f, leak)
        leak, f = R.leak_check(Stub({"scroll_current": 1, "open_contexts": 3}), w, wait_s=0)
        check("stub: ES open_contexts beyond scrolls fails", leak["other_open_contexts"] == 2 and f, leak)
        leak, f = R.leak_check(Stub({"scroll_current": 2, "point_in_time_current": 0}), w, wait_s=0)
        check("stub: open scroll fails", any("scroll_current=2" in x for x in f), f)
        leak, f = R.leak_check(Stub({"scroll_current": 2}), w, wait_s=0, contexts=False)
        check("stub: contexts=False records only", leak["scroll_current"] == 2 and not f, f)
        for d in ("it", "it-storm", "sub/kill", "sub/ok"):
            os.makedirs(os.path.join(w, d))
        for p in ("it/.x.part-1", "it-storm/y.part", "sub/kill/.d.ndjson.part-000001"):
            open(os.path.join(w, p), "w").close()
        leak, f = R.leak_check(None, w, allow_part=lambda rel, _f: rel.endswith("kill"))
        check("files: cluster dirs skipped, allow_part honoured", not f and leak["part_files_n"] == 0, leak)
        open(os.path.join(w, "sub/ok/.dump.ndjson.part-000003"), "w").close()
        leak, f = R.leak_check(None, w, allow_part=lambda rel, _f: rel.endswith("kill"))
        check("files: hidden staging part found (glob would miss it)",
              leak["part_files"] == ["sub/ok/.dump.ndjson.part-000003"] and f, leak)
    finally:
        shutil.rmtree(w, ignore_errors=True)


def online(a):
    lock = open("/tmp/elasticdump-rs-it.lock", "w")
    fcntl.flock(lock, fcntl.LOCK_EX)
    run_id = datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + "-leakcheck"
    work = os.path.join(os.environ.get("LOAD_WORK", "/var/tmp/elasticdump-rs-load"), run_id)
    os.makedirs(work, mode=0o700, exist_ok=True)
    base = dict(line.split("=", 1) for line in subprocess.run(
        [os.path.join(ROOT, "scripts/it/matrix.sh"), a.engine], check=True, capture_output=True,
        text=True).stdout.splitlines())
    up = base.pop("UP_PROFILE")
    env = dict(os.environ, **base, IT_DIR=os.path.join(work, "it"), IT_LOCK_HELD="1",
               RUN_ID=f"load{os.getpid()}leak", ES_HEAP="512m", OS_HEAP="512m", PORT="19202",
               IT_CONTAINER_MEM="2g")
    try:
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up], env=env, check=True, stdout=subprocess.DEVNULL)
        cl = R.Cluster(R.sh_env(os.path.join(env["IT_DIR"], "env"))["IT_ES_URL"])
        ver = cl.req("GET", "/")["version"]["number"]
        print(f"engine {a.engine} {ver}")
        cl.req("PUT", "/lk", {"settings": {"number_of_shards": 1, "number_of_replicas": 0}})
        cl.req("POST", "/lk/_bulk?refresh=true", b'{"index":{}}\n{"a":1}\n{"index":{}}\n{"a":2}\n',
               ctype="application/x-ndjson")
        before = {i["index"] for i in cl.req("GET", "/_cat/indices?format=json&expand_wildcards=all")}
        leak, f = R.leak_check(cl, work, before=before, wait_s=2)
        check("clean cluster passes", not f, leak)
        sid = cl.req("POST", "/lk/_search?scroll=5m", {"size": 1})["_scroll_id"]
        leak, f = R.leak_check(cl, work, before=before, wait_s=2)
        check("open scroll is reported", leak["scroll_current"] >= 1 and f, leak)
        cl.req("DELETE", "/_search/scroll", {"scroll_id": sid})
        leak, f = R.leak_check(cl, work, before=before, wait_s=5)
        check("cleared scroll passes", not f, leak)
        # PIT: OpenSearch point_in_time_current; Elasticsearch other_open_contexts
        osd = cl.req("GET", "/")["version"].get("distribution") == "opensearch"
        if osd:
            pid = cl.req("POST", "/lk/_search/point_in_time?keep_alive=5m")["pit_id"]
        else:
            pid = cl.req("POST", "/lk/_pit?keep_alive=5m")["id"]
        leak, f = R.leak_check(cl, work, before=before, wait_s=2)
        check("open PIT is reported", ((leak["pit_current"] or 0) >= 1 or (leak["other_open_contexts"] or 0) >= 1)
              and f, leak)
        if osd:
            cl.req("DELETE", "/_search/point_in_time", {"pit_id": [pid]})
        else:
            cl.req("DELETE", "/_pit", {"id": pid})
        leak, f = R.leak_check(cl, work, before=before, wait_s=5)
        check("closed PIT passes", not f, leak)
        cl.req("PUT", "/stray", {"settings": {"number_of_shards": 1, "number_of_replicas": 0}})
        cl.req("PUT", "/.stray-staging", {"settings": {"number_of_shards": 1, "number_of_replicas": 0}})
        leak, f = R.leak_check(cl, work, before=before, wait_s=0)
        check("stray and hidden indices are reported",
              leak["unexpected_indices"] == ["stray"] and leak["new_hidden_indices"] == [".stray-staging"] and f,
              leak)
        leak, f = R.leak_check(cl, work, before=before, expected={"stray", ".stray-staging"}, wait_s=0)
        check("expected indices pass", not f, leak)
    finally:
        if not a.keep:
            # only this run's container (down.sh would remove every IT container on the box)
            subprocess.run(["podman", "rm", "-f", f"it-es-plain-{env['RUN_ID']}", f"it-os-plain-{env['RUN_ID']}"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run((["rm", "-rf"] if os.geteuid() == 0 else ["podman", "unshare", "rm", "-rf"]) + [work])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--offline", action="store_true")
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    offline()
    if not a.offline:
        online(a)
    print(f"LEAKCHECK-SELFTEST {'PASS' if not FAILS else 'FAIL ' + json.dumps(FAILS)}")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
