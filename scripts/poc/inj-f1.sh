#!/usr/bin/env bash
# inj-f1.sh <es-url> <binary> <workdir>
# Live reproduction of INJ-F1 (TEST-REQUIREMENTS.md §8.1): a pretty-printed
# _source breaks the NDJSON framing of a cluster->cluster copy and of dump
# files. Prints one RESULT line per check; exit 0 = no injection observed.
set -uo pipefail
U="${1:?es url}"; BIN="${2:?binary}"; W="${3:?workdir}"
# VARIANT=1: the PoC from §8.1 (closing braces on their own lines).
# VARIANT=2: closing braces on the injected line, so the trailing fragment
#            does not abort the batch (the injected line still parses
#            because Top::parse keeps members read before an error).
VARIANT="${VARIANT:-1}"
U2="${U/localhost/127.0.0.1}"   # a second "host" string for host-only copies
mkdir -p "$W"; rm -f "$W"/*
fail=0
j() { curl -s -H 'Content-Type: application/json' "$@"; }
reset() {
  j -XDELETE "$U/src,victim,dst?ignore_unavailable=true" >/dev/null
  if [[ "$VARIANT" == 2 ]]; then
    printf '{"a":\n{"_index":"victim","_id":"admin","_source":{"role":"superuser"}}}' > "$W/poc.json"
  else
    printf '{"a":\n{"_index":"victim","_id":"admin","_source":{"role":"superuser"}}\n}\n' > "$W/poc.json"
  fi
  j -XPUT "$U/src/_doc/x?refresh=true" --data-binary @"$W/poc.json" >/dev/null
  j -XPUT "$U/dst/_doc/admin?refresh=true" -d '{"role":"user"}' >/dev/null
}
check() { # name cond-desc ok?
  if [[ "$3" == ok ]]; then echo "RESULT $1: OK ($2)"; else echo "RESULT $1: INJECTED/BROKEN ($2)"; fail=1; fi
}
src_x() { j "$U/src/_doc/x" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(json.dumps(d.get("_source"),sort_keys=True))'; }
want_x='{"a": {"_id": "admin", "_index": "victim", "_source": {"role": "superuser"}}}'
doc() { j "$U/$1/_doc/$2" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(json.dumps(d.get("_source")) if d.get("found") else "MISSING")'; }

echo "== raw _source as returned by the cluster"
j "$U/src/_search?size=1" >/dev/null; reset
j "$U/src/_search" | python3 -c 'import sys; b=sys.stdin.read(); print("newlines inside hits:", b.count("\n"))'

echo "== A: host-only copy (all indices) $U2 -> $U2"
reset
"$BIN" --input="$U" --output="$U2" --type=data --quiet > "$W/A.log" 2>&1; rcA=$?
j -XPOST "$U/_refresh" >/dev/null
echo "exit=$rcA victim/admin=$(doc victim admin) src/x=$(src_x)"
[[ "$(doc victim admin)" == MISSING ]] && check A-victim "victim/admin not created" ok || check A-victim "victim/admin written by a document body" bad
[[ "$(src_x)" == "$want_x" ]] && check A-x "src/x source preserved" ok || check A-x "src/x overwritten: $(src_x)" bad

echo "== B: --output-index (src -> dst)"
reset
"$BIN" --input="$U/src" --output="$U2" --output-index=dst --type=data --quiet > "$W/B.log" 2>&1; rcB=$?
j -XPOST "$U/_refresh" >/dev/null
echo "exit=$rcB dst/admin=$(doc dst admin) dst/x=$(doc dst x)"
[[ "$(doc dst admin)" == '{"role": "user"}' ]] && check B-dst-admin "dst/admin unchanged" ok || check B-dst-admin "dst/admin now $(doc dst admin)" bad

echo "== C: --bulkAction delete (src -> dst)"
reset
"$BIN" --input="$U/src" --output="$U2" --output-index=dst --bulkAction=delete --type=data --quiet > "$W/C.log" 2>&1; rcC=$?
j -XPOST "$U/_refresh" >/dev/null
echo "exit=$rcC dst/admin=$(doc dst admin)"
[[ "$(doc dst admin)" == '{"role": "user"}' ]] && check C-dst-admin "dst/admin not deleted" ok || check C-dst-admin "dst/admin now $(doc dst admin)" bad

echo "== D: file dump, --sourceOnly, --fileSize"
reset
"$BIN" --input="$U/src" --output="$W/dump.json" --type=data --quiet > "$W/D.log" 2>&1
lines=$(wc -l < "$W/dump.json"); echo "dump lines=$lines (hits=1)"
python3 - "$W/dump.json" <<'PY' && check D-lines "1 line, parses alone" ok || check D-lines "$lines lines for 1 hit" bad
import json,sys
ls=[l for l in open(sys.argv[1]).read().split("\n") if l]
assert len(ls)==1, ls
json.loads(ls[0])
PY
"$BIN" --input="$U/src" --output="$W/src-only.json" --sourceOnly --type=data --quiet > "$W/D2.log" 2>&1
python3 - "$W/src-only.json" <<'PY' && check D-sourceOnly "1 line" ok || check D-sourceOnly "multi-line" bad
import json,sys
ls=[l for l in open(sys.argv[1]).read().split("\n") if l]
assert len(ls)==1, ls
json.loads(ls[0])
PY
# restore the dump into a fresh cluster index: the injected fragment must not appear
j -XDELETE "$U/victim?ignore_unavailable=true" >/dev/null
"$BIN" --input="$W/dump.json" --output="$U2" --type=data --quiet > "$W/D3.log" 2>&1; rcD=$?
j -XPOST "$U/_refresh" >/dev/null
echo "restore exit=$rcD victim/admin=$(doc victim admin)"
[[ "$(doc victim admin)" == MISSING ]] && check D-restore "restore did not write victim" ok || check D-restore "restore wrote victim/admin" bad
j -XDELETE "$U/src,victim,dst?ignore_unavailable=true" >/dev/null
echo "OVERALL: $([[ $fail == 0 ]] && echo NO-INJECTION || echo INJECTION-REPRODUCED)"
exit $fail
