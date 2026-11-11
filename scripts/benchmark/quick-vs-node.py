#!/usr/bin/env python3
"""Quick Rust-vs-Node comparison against a running OpenSearch (no auth).

  URL=http://127.0.0.1:19200 N=100000 NODE_BIN=/path/to/elasticdump \
    python3 scripts/benchmark/quick-vs-node.py

Seeds index `bsrc` (N docs, 2 shards) and times dump / restore / copy for both
tools. Results go to $BENCH_DIR/results.json. Deletes `bsrc` and `bdst`.

For repeated runs at 100k/1M with digest checks and the PLAN.md §8 scenarios, use
scripts/load/nodecmp.py (LOAD-011).
"""
import json, os, resource, subprocess, sys, time, urllib.request, random
URL = os.environ.get("URL", "http://127.0.0.1:19200")
RUST = os.environ.get("RUST_BIN", "target/it/searchdump")
NODE = os.environ.get("NODE_BIN", "elasticdump")
N = int(os.environ.get("N", "200000"))
D = os.environ.get("BENCH_DIR", "/tmp/edbench")
os.makedirs(D, exist_ok=True)

def http(method, path, body=None):
    req = urllib.request.Request(URL + path, method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Content-Type": "application/json"})
    try:
        return json.loads(urllib.request.urlopen(req).read() or b"{}")
    except urllib.error.HTTPError as e:
        return json.loads(e.read() or b"{}")

def timed(cmd):
    r0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    t = time.time()
    p = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    el = time.time() - t
    r1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    if p.returncode != 0:
        print("FAILED", cmd, p.stderr.decode()[-2000:]); sys.exit(1)
    cpu = (r1.ru_utime - r0.ru_utime) + (r1.ru_stime - r0.ru_stime)
    return el, cpu, r1.ru_maxrss / 1024  # maxrss of largest child so far (MB)

def fresh(idx, shards=2):
    http("DELETE", "/" + idx)
    http("PUT", "/" + idx, {"settings": {"number_of_shards": shards, "number_of_replicas": 0, "refresh_interval": "-1"}})

def count(idx):
    http("POST", f"/{idx}/_refresh")
    return http("GET", f"/{idx}/_count")["count"]

def seed():
    path = f"{D}/seed.ndjson"
    rnd = random.Random(1)
    with open(path, "w") as f:
        for i in range(N):
            f.write(json.dumps({"_index": "bsrc", "_id": str(i), "_source": {
                "k": f"key{i}", "n": i, "t": " ".join(rnd.choice(["lorem","ipsum","dolor","sit","amet","consectetur","adipiscing","elit"]) for _ in range(40)),
                "d": "2026-01-01T00:00:00Z", "o": {"x": i % 100, "y": [1, 2, 3]}}}) + "\n")
    fresh("bsrc")
    timed([RUST, "--input", path, "--output", URL + "/bsrc", "--limit", "5000", "--concurrency", "4", "--noRefresh", "--quiet"])
    assert count("bsrc") == N, count("bsrc")
    return os.path.getsize(path)

results = []
def rec(name, tool, el, cpu, rss):
    results.append({"scenario": name, "tool": tool, "seconds": round(el, 2), "docs_per_s": int(N / el), "cpu_s": round(cpu, 2), "max_rss_mb_cumulative": int(rss)})
    print(json.dumps(results[-1]), flush=True)

size = seed()
print("dataset bytes", size, flush=True)
out = f"{D}/out.json"
def rm(): 
    for f in os.listdir(D):
        if f.startswith("out"): os.remove(os.path.join(D, f))

# dump, defaults (limit 100)
for tool, cmd in [("node", [NODE, "--input", URL + "/bsrc", "--output", out]),
                  ("rust", [RUST, "--input", URL + "/bsrc", "--output", out, "--quiet"])]:
    rm(); rec("dump limit=100", tool, *timed(cmd))
# dump, limit 5000
for tool, cmd in [("node", [NODE, "--input", URL + "/bsrc", "--output", out, "--limit", "5000"]),
                  ("rust", [RUST, "--input", URL + "/bsrc", "--output", out, "--limit", "5000", "--quiet"])]:
    rm(); rec("dump limit=5000", tool, *timed(cmd))
rm(); rec("dump limit=5000 sliced PIT x4", "rust", *timed([RUST, "--input", URL + "/bsrc", "--output", out, "--limit", "5000", "--searchAfter", "--pit", "--slices", "4", "--quiet"]))
rm(); rec("dump limit=5000 gzip", "node", *timed([NODE, "--input", URL + "/bsrc", "--output", out, "--limit", "5000", "--fsCompress"]))
rm(); rec("dump limit=5000 gzip", "rust", *timed([RUST, "--input", URL + "/bsrc", "--output", out, "--limit", "5000", "--fsCompress", "--quiet"]))
rm(); rec("dump limit=5000 zstd", "rust", *timed([RUST, "--input", URL + "/bsrc", "--output", out, "--limit", "5000", "--compressionType", "zstd", "--quiet"]))
# restore
seedf = f"{D}/seed.ndjson"
for tool, base in [("node", [NODE]), ("rust", [RUST, "--quiet"])]:
    fresh("bdst")
    rec("restore limit=5000 conc=4 noRefresh", tool, *timed(base + ["--input", seedf, "--output", URL + "/bdst", "--limit", "5000", "--concurrency", "4", "--noRefresh"]))
    assert count("bdst") == N, (tool, count("bdst"))
# copy
for tool, base in [("node", [NODE]), ("rust", [RUST, "--quiet"])]:
    fresh("bdst")
    rec("copy limit=5000 conc=4", tool, *timed(base + ["--input", URL + "/bsrc", "--output", URL + "/bdst", "--limit", "5000", "--concurrency", "4", "--noRefresh"]))
    assert count("bdst") == N, (tool, count("bdst"))
json.dump({"n_docs": N, "dataset_bytes": size, "results": results}, open(f"{D}/results.json", "w"), indent=1)
