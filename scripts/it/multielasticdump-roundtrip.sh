#!/usr/bin/env bash
# multielasticdump dump -> load round trip against two in-memory fake nodes
# (scripts/it/fake_es.py). No containers, no network beyond 127.0.0.1.
# Usage: scripts/it/multielasticdump-roundtrip.sh [BIN_DIR]   (default target/debug; builds it)
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=${1:-target/debug}
[[ $# -ge 1 ]] || cargo build --quiet --bin elasticdump-rs --bin multielasticdump
M=$BIN/multielasticdump
W=$(mktemp -d); pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true; rm -rf "$W"' EXIT
free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
SRC=$(free_port); DST=$(free_port); OSP=$(free_port)
python3 scripts/it/fake_es.py "$SRC" seed 2>"$W/src.log" & pids+=($!)
python3 scripts/it/fake_es.py "$DST" 2>"$W/dst.log" & pids+=($!)
python3 scripts/it/fake_es.py "$OSP" os 2>"$W/os.log" & pids+=($!)
for p in $SRC $DST $OSP; do for _ in $(seq 50); do curl -sf "http://127.0.0.1:$p/" >/dev/null && break; sleep 0.1; done; done
fail=0
ok()  { echo "ok    $*"; }
bad() { echo "FAIL  $*"; fail=1; }
check() { local d=$1; shift; if "$@" >"$W/last.out" 2>&1; then ok "$d"; else bad "$d"; sed 's/^/      /' "$W/last.out"; fi; }
count() { curl -sf "http://127.0.0.1:$1/$2/_count" | python3 -c 'import json,sys;print(json.load(sys.stdin)["count"])'; }
idx() { curl -sf "http://127.0.0.1:$1/$2"; }

# 1. regex + group break-out, zstd3, alias
check "dump group layout (zstd3, alias)" "$M" --input="http://127.0.0.1:$SRC" --output="$W/g" \
  --match='^logs-(?P<group>[a-z]+)-.*$' --layout=group --types=settings,mapping,alias,data --compression=zstd3 --logDir="$W/l1"
check "group dirs app/ and web/" test -s "$W/g/app/logs-app-2026.10.01.data.json.zst" -a -s "$W/g/web/logs-web-2026.10.01.data.json.zst"
check "metrics-1 and .kibana_1 not selected" test ! -e "$W/g/metrics-1.settings.json" -a ! -e "$W/g/app/.kibana_1.settings.json"
check "manifest has 3 done" python3 -c "import json,sys;m=json.load(open('$W/g/multielasticdump.json'));sys.exit(0 if sum(e['status']=='done' for e in m['indices'].values())==3 else 1)"
# 2. load it back with a suffix, lifecycle dropped
check "load with --suffix -r --lifecycle drop" "$M" --direction=load --input="$W/g" --output="http://127.0.0.1:$DST" --suffix=-r --logDir="$W/l2"
check "doc counts 7/3/5" test "$(count $DST logs-app-2026.10.01-r)/$(count $DST logs-app-2026.10.02-r)/$(count $DST logs-web-2026.10.01-r)" = 7/3/5
check "alias restored" sh -c "curl -sf http://127.0.0.1:$DST/logs-app-2026.10.01-r | grep -q '\"logs-app\"'"
check "index.lifecycle dropped" sh -c "! curl -sf http://127.0.0.1:$DST/logs-app-2026.10.01-r | grep -q lifecycle"
check "mapping restored" sh -c "curl -sf http://127.0.0.1:$DST/logs-app-2026.10.01-r/_mapping | grep -q '\"msg\"'"
# 3. existing target: refuse, then --skipExisting
check "reload refuses existing (exit 1)" sh -c "! '$M' --direction=load --input='$W/g' --output=http://127.0.0.1:$DST --suffix=-r --logDir='$W/l3'"
check "reload --skipExisting exits 0" "$M" --direction=load --input="$W/g" --output="http://127.0.0.1:$DST" --suffix=-r --skipExisting --logDir="$W/l4"
# 4. flat gzip, hidden, exclude, empty index; oslogs.py-compatible names
check "dump flat gzip --includeHidden --exclude" "$M" --input="http://127.0.0.1:$SRC" --output="$W/f" --includeHidden --exclude='^metrics' --logDir="$W/l5"
check "oslogs.py file names" test -s "$W/f/.kibana_1.settings.json" -a -s "$W/f/logs-app-2026.10.02.mapping.json" -a -s "$W/f/logs-app-2026.10.02.data.json.gz"
check "load empty and hidden index" "$M" --direction=load --input="$W/f" --output="http://127.0.0.1:$DST" --match='^(\.kibana|empty)' --includeHidden --logDir="$W/l6"
check "empty index created, .kibana_1 has 1 doc" test "$(count $DST empty-logs-x)/$(count $DST .kibana_1)" = 0/1
# 5. load without a manifest (plain oslogs.py directory)
rm "$W/f/multielasticdump.json"
check "load without manifest" "$M" --direction=load --input="$W/f" --output="http://127.0.0.1:$DST" --match='^logs-web' --prefix=nm- --logDir="$W/l7"
check "no-manifest count 5" test "$(count $DST nm-logs-web-2026.10.01)" = 5
# 6. OpenSearch target, drop lifecycle (the default)
check "load into OpenSearch-flavoured node" "$M" --direction=load --input="$W/g" --output="http://127.0.0.1:$OSP" --match='app-2026.10.02' --prefix=os- --logDir="$W/l8"
check "dry run lists, writes nothing" sh -c "'$M' --input=http://127.0.0.1:$SRC --output='$W/dry' --dryRun | grep -q logs-web && test ! -e '$W/dry/multielasticdump.json'"
if grep -h UNHANDLED "$W"/*.log; then bad "fake node saw requests it does not handle"; fi
[[ $fail == 0 ]] && echo "multielasticdump round trip: all checks passed" || { echo "multielasticdump round trip: FAILED"; exit 1; }
