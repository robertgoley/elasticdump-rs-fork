#!/usr/bin/env bash
# L-635 (TST-BRK-RES-001): restores under real cgroup v2 memory caps (scripts/res/cg-run.sh).
# For each cap (64/128/256 MB) and flag set: the README estimate, whether the start-of-run
# memory warning fired, exit code, oom_kill and memory.peak. Runs inside the formula's bound
# must succeed with memory.peak <= the estimate; runs outside it must warn.
# usage: low-memory.sh <binary> <out.json>
set -euo pipefail
BIN="${1:?binary}"; OUT="${2:?out.json}"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d /tmp/lowmem.XXXXXX)"
srv_pid=""
cleanup() { [[ -n "$srv_pid" ]] && kill "$srv_pid" 2>/dev/null; rm -rf "$work"; }
trap cleanup EXIT
# 1000 docs x ~40 KB = ~40 MB input
python3 - "$work/in.json" <<'PY'
import json, sys
pad = "x" * 40000
with open(sys.argv[1], "w") as f:
    for i in range(1000):
        f.write(json.dumps({"_index": "x", "_id": str(i), "_source": {"n": i, "pad": pad}}) + "\n")
PY
port=$((20000 + RANDOM % 20000))
python3 - "$port" > /dev/null 2>&1 <<'PY' &
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def reply(s, b):
        s.send_response(200); s.send_header("content-type", "application/json")
        s.send_header("content-length", str(len(b))); s.end_headers(); s.wfile.write(b)
    def do_GET(s): s.reply(b'{"version":{"number":"8.19.0"}}')
    def do_POST(s):
        body = s.rfile.read(int(s.headers.get("content-length", 0)))
        n = body.count(b"\n") // 2
        s.reply(('{"errors":false,"items":[' + ",".join(['{"index":{"status":201}}'] * n) + "]}").encode())
    def log_message(s, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
srv_pid=$!
sleep 1
avg=$(( $(stat -c %s "$work/in.json") / 1000 ))
echo "[" > "$OUT"
first=1
for mem in 64 128 256; do
  for flags in "--limit 10 --bufferSize 2" "--limit 50 --bufferSize 4" "" "--limit 100 --concurrency 4" "--limit 10000"; do
    read -r limit buf conc <<<"$(python3 -c "
import sys; a=sys.argv[1].split(); g=lambda k,d: int(a[a.index(k)+1]) if k in a else d
print(g('--limit',100), g('--bufferSize',16), g('--concurrency',1))" "$flags")"
    est=$(( ( (buf + conc) * limit * avg * 3 / 2 + 10485760 ) / 1048576 ))
    rc=0
    # shellcheck disable=SC2086
    "$here/cg-run.sh" "${mem}M" "$BIN" --input "$work/in.json" --output "http://127.0.0.1:$port/x" --noRefresh $flags \
      > "$work/out.txt" 2>&1 || rc=$?
    warned=$(grep -c "memory: this run may need" "$work/out.txt" || true)
    oom=$(sed -n 's/.*oom_kill=\([0-9]*\).*/\1/p' "$work/out.txt" | tail -1)
    peak=$(sed -n 's/.*memory.peak=\([0-9?]*\).*/\1/p' "$work/out.txt" | tail -1)
    peak_mb=$([[ "$peak" =~ ^[0-9]+$ ]] && echo $((peak / 1048576)) || echo null)
    [[ $first == 1 ]] || echo "," >> "$OUT"; first=0
    printf '{"cap_mb":%s,"flags":"%s","estimate_mb":%s,"warned":%s,"rc":%s,"oom_kill":%s,"peak_mb":%s}' \
      "$mem" "$flags" "$est" "$warned" "$rc" "${oom:-0}" "$peak_mb" >> "$OUT"
    echo "cap=${mem}M flags='$flags' est=${est}MB warned=$warned rc=$rc oom=${oom:-0} peak=${peak_mb}MB"
  done
done
echo "]" >> "$OUT"
