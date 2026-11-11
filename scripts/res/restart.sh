#!/usr/bin/env bash
# TST-BRK-NET-009 (L-606): single-node restart (`podman restart`) mid-dump and mid-restore,
# against the throwaway engine started by scripts/it/up.sh (never any other container).
#   scripts/res/restart.sh [bin]       (needs $IT_ES_URL and $IT_CONTAINER from $IT_DIR/env)
# Cases:
#   dump-scroll / dump-pit  restart while paging -> exit 1 with a clear "context lost / rerun"
#                           message; nothing committed; a rerun gives the clean digest
#   restore-covered         --retryAttempts covers the downtime -> exit 0, every doc written
#   restore-short           --retryAttempts 1 -> exit 1, clear message; the documented rerun
#                           (explicit _ids make it idempotent) gives every doc
# Prints RESULT lines; exit 0 = every case as expected.
set -uo pipefail
cd "$(dirname "$0")/../.."
BIN="${1:-target/release/searchdump}"
: "${IT_ES_URL:?source \$IT_DIR/env first}" "${IT_CONTAINER:?}"
podman inspect -f '{{index .Config.Labels "searchdump-it"}}' "$IT_CONTAINER" | grep -qx 1 \
  || { echo "refusing: $IT_CONTAINER is not an searchdump-it container" >&2; exit 2; }
U="${IT_ES_URL%/}"; N="${N:-20000}"; W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
fail=0
j() { curl -s -H 'Content-Type: application/json' "$@"; }
ready() { for _ in $(seq 1 120); do j "$U/_cluster/health?wait_for_status=yellow&timeout=1s" 2>/dev/null | grep -q '"status"' && return 0; sleep 1; done; return 1; }
# stop, wait until rootless podman's port forwarder released the port, start (a plain
# `podman restart` races the forwarder: "pasta failed ... Address already in use")
restart_after() {
  sleep "$1"; local t=$(date +%s) port=${U##*:}
  podman stop -t 5 "$IT_CONTAINER" >/dev/null
  for _ in $(seq 1 60); do ss -ltn "sport = :$port" | grep -q LISTEN || break; sleep 1; done
  for _ in 1 2 3 4 5; do podman start "$IT_CONTAINER" >/dev/null 2>&1 && break; sleep 2; done
  ready; echo $(( $(date +%s) - t )) > "$W/down"
}
digest() { python3 -c 'import sys,json,hashlib; L=sorted(json.dumps(json.loads(l)["_source"],sort_keys=True)+json.loads(l)["_id"] for l in open(sys.argv[1])); print(hashlib.sha256("\n".join(L).encode()).hexdigest(), len(L))' "$1"; }
res() { if [[ "$2" == ok ]]; then echo "RESULT $1: OK ($3)"; else echo "RESULT $1: FAIL ($3)"; fail=1; fi; }
last_err() { grep -E '^Error' "$1" | tail -1 | cut -c1-400; }

j -XDELETE "$U/rs,rr?ignore_unavailable=true" >/dev/null
j -XPUT "$U/rs" -d '{"settings":{"number_of_replicas":0,"number_of_shards":2}}' >/dev/null
python3 - "$N" > "$W/seed.ndjson" <<'PY'
import sys
for i in range(int(sys.argv[1])):
    print('{"index":{"_id":"%d"}}' % i); print('{"n":%d,"s":"%s"}' % (i, "v" * 64))
PY
split -l 4000 "$W/seed.ndjson" "$W/seed."
for f in "$W"/seed.??; do curl -s -XPOST "$U/rs/_bulk" -H 'Content-Type: application/x-ndjson' --data-binary @"$f" | grep -q '"errors":false' || { echo "seed failed"; exit 1; }; done
j -XPOST "$U/rs/_refresh" >/dev/null
"$BIN" --input="$U/rs" --output="$W/clean.json" --limit 1000 --quiet || { echo "clean dump failed"; exit 1; }
clean=$(digest "$W/clean.json"); echo "clean digest: $clean"

# *-long: retries outlast the downtime, so the run reaches the restarted node and finds the
# context gone (404 search_context_missing)
for mode in scroll pit scroll-long pit-long; do
  extra=(); [[ $mode == pit* ]] && extra=(--searchType pit)
  [[ $mode == *-long ]] && extra+=(--retryAttempts 40 --retryDelay 3000) || extra+=(--retryDelay 2000)
  rm -f "$W/d.json"
  "$BIN" --input="$U/rs" --output="$W/d.json" --limit 100 --throttleInterval 150 "${extra[@]}" > "$W/d-$mode.log" 2>&1 & p=$!
  restart_after 3; wait $p; rc=$?
  msg=$(last_err "$W/d-$mode.log")
  echo "dump-$mode: rc=$rc down=$(cat "$W/down")s msg: $msg"
  # pit-long must reach the restarted node (PIT pages are idempotent, so the cut page is
  # retried). A scroll continuation cut in flight is never retried (the cursor may have moved),
  # so scroll-long ends with either message, depending on where the restart lands.
  ok=1; [[ $mode == pit-long ]] && { grep -q 'context no longer exists' <<<"$msg" || ok=0; }
  [[ $ok == 1 && $rc == 1 && ! -e "$W/d.json" ]] && grep -qiE 'rerun|re-run' <<<"$msg" && grep -qiE 'context|restart' <<<"$msg" \
    && res "dump-$mode" ok "exit 1, nothing committed, context-lost/rerun message" \
    || res "dump-$mode" bad "rc=$rc committed=$([[ -e $W/d.json ]] && echo yes || echo no)"
  "$BIN" --input="$U/rs" --output="$W/d.json" --overwrite --limit 1000 "${extra[@]}" --quiet
  [[ "$(digest "$W/d.json")" == "$clean" ]] && res "dump-$mode-rerun" ok "digest equal" || res "dump-$mode-rerun" bad "digest differs"
done

for case in covered short; do
  j -XDELETE "$U/rr?ignore_unavailable=true" >/dev/null
  j -XPUT "$U/rr" -d '{"settings":{"number_of_replicas":0}}' >/dev/null
  if [[ $case == covered ]]; then r=(--retryAttempts 40 --retryDelay 3000); else r=(--retryAttempts 1 --retryDelay 1000); fi
  "$BIN" --input="$W/clean.json" --output="$U/rr" --limit 200 --throttleInterval 150 "${r[@]}" > "$W/r-$case.log" 2>&1 & p=$!
  restart_after 3; wait $p; rc=$?
  ready; j -XPOST "$U/rr/_refresh" >/dev/null
  cnt=$(j "$U/rr/_count" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("count"))')
  msg=$(last_err "$W/r-$case.log")
  echo "restore-$case: rc=$rc down=$(cat "$W/down")s count=$cnt msg: $msg"
  if [[ $case == covered ]]; then
    [[ $rc == 0 && $cnt == "$N" ]] && res restore-covered ok "survived the restart, $cnt docs" || res restore-covered bad "rc=$rc count=$cnt"
  else
    [[ $rc == 1 && -n "$msg" ]] && res restore-short ok "exit 1: clear error" || res restore-short bad "rc=$rc"
    "$BIN" --input="$W/clean.json" --output="$U/rr" --limit 1000 --quiet; j -XPOST "$U/rr/_refresh" >/dev/null
    cnt=$(j "$U/rr/_count" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("count"))')
    [[ $cnt == "$N" ]] && res restore-short-rerun ok "rerun restored $cnt docs (explicit _ids: idempotent)" || res restore-short-rerun bad "count=$cnt"
  fi
done
j -XDELETE "$U/rs,rr?ignore_unavailable=true" >/dev/null
echo "OVERALL: $([[ $fail == 0 ]] && echo PASS || echo FAIL)"
exit $fail
