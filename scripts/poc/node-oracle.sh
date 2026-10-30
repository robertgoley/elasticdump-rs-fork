#!/usr/bin/env bash
# node-oracle.sh <es-url> <workdir>
# TST-INJ-017 / L-513: run Node elasticdump and elasticdump-rs on the injection PoC files
# against one live (throwaway test) engine and print one RESULT line per tool and case.
#   INJ-F1: scripts/poc/inj-f1.sh variants 1 and 2 (pretty-printed _source -> NDJSON framing)
#   INJ-F2: analyzer `{"*":...}` (closes every index?), alias `{"*":{"aliases":...}}` (alias on
#           every index?), data `_index: "_all"` / `"*"`
# NODE_ED (default /tmp/ed/node_modules/.bin/elasticdump), RS (default target/release/elasticdump-rs).
set -uo pipefail
U="${1:?url}"; W="${2:?workdir}"
cd "$(dirname "$0")/../.."
NODE_ED="${NODE_ED:-/tmp/ed/node_modules/.bin/elasticdump}"; RS="${RS:-target/release/elasticdump-rs}"
mkdir -p "$W"
j() { curl -s -H 'Content-Type: application/json' "$@"; }
for tool in rs node; do
  bin=$RS; [[ $tool == node ]] && bin=$NODE_ED
  for v in 1 2; do
    out=$(VARIANT=$v scripts/poc/inj-f1.sh "$U" "$bin" "$W/f1-$tool-$v" 2>&1)
    echo "$out" | grep -E '^RESULT|^OVERALL' | sed "s/^/$tool F1v$v /"
  done
  # INJ-F2 setup: two ordinary indices
  reset2() {
    j -XDELETE "$U/keep1,keep2,victim,_all-ish?ignore_unavailable=true" >/dev/null
    j -XPUT "$U/keep1" -d '{"settings":{"number_of_replicas":0}}' >/dev/null
    j -XPUT "$U/keep2" -d '{"settings":{"number_of_replicas":0}}' >/dev/null
  }
  closed() { j "$U/_cat/indices/keep1,keep2?h=status" | grep -c close; }
  aliased() { j "$U/_alias/logs-read" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(len([k for k in d if k in ("keep1","keep2")]) if isinstance(d,dict) and "error" not in d else 0)'; }
  reset2
  printf '{"*":{"settings":{"index":{"analysis":{"analyzer":{"evil":{"type":"standard"}}}}}}}\n' > "$W/an.json"
  extra=(); [[ $tool == rs ]] && extra=(--closeIndex)
  timeout 120 "$bin" --input="$W/an.json" --output="$U" --type=analyzer "${extra[@]}" > "$W/an-$tool.log" 2>&1; rc=$?
  c=$(closed); j -XPOST "$U/keep1,keep2/_open" >/dev/null
  echo "$tool F2-analyzer: exit=$rc indices-closed=$c $([[ $c == 0 ]] && echo OK || echo 'CLOSED OTHER INDICES')"
  reset2
  printf '{"*":{"aliases":{"logs-read":{}}}}\n' > "$W/al.json"
  timeout 120 "$bin" --input="$W/al.json" --output="$U" --type=alias > "$W/al-$tool.log" 2>&1; rc=$?
  a=$(aliased)
  echo "$tool F2-alias: exit=$rc keep-indices-with-alias=$a $([[ $a == 0 ]] && echo OK || echo 'ALIAS ON EVERY INDEX')"
  j -XDELETE "$U/*/_alias/logs-read" >/dev/null
  reset2
  printf '{"_index":"_all","_id":"z","_source":{"x":1}}\n{"_index":"*","_id":"y","_source":{"x":2}}\n' > "$W/da.json"
  timeout 120 "$bin" --input="$W/da.json" --output="$U" --type=data > "$W/da-$tool.log" 2>&1; rc=$?
  n=$(j "$U/keep1,keep2/_count" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("count"))')
  echo "$tool F2-data-_index: exit=$rc docs-in-keep-indices=$n $(tail -1 "$W/da-$tool.log" | cut -c1-140)"
done
j -XDELETE "$U/keep1,keep2?ignore_unavailable=true" >/dev/null
