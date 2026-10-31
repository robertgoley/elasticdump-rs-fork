#!/usr/bin/env bash
# L-702 (TST-SUP-002): syscall and file-access tracing of elasticdump-rs.
#
#   scripts/sup/trace.sh [--selftest] [--cases plain,tls]
#
# Runs a command matrix under `strace -ff -y -s 0 -e trace=%file,%network,%process` and
# checks every traced syscall against scripts/sup/trace-allow.txt with trace-check.py:
#   plain (opensearch-plain from scripts/it/up.sh): dump data / mapping / settings / alias /
#         template / analyzer to files, split output (--fileSize, --maxRows), gzip output,
#         stdout output, restore settings+mapping+data from files, gzip restore, stdin
#         restore, cluster-to-cluster copy, and a failing run (missing index);
#   tls   (opensearch-secure): dump with --input-ca + --httpAuthFile, dump with a client
#         certificate (--input-cert/--input-key --tlsAuth), restore over TLS.
# Each command runs with a minimal environment (`env -i PATH HOME LANG`, HOME a directory of
# its own that the allow-list does not cover) from inside the work directory. The run fails
# when a command's exit status is not the expected one or when a case's allow-list diff is
# not empty: no execve beyond the binary, no child process, connect only to the cluster
# port on loopback, file syscalls only on allow-listed paths (input, output dir, staging
# `.part` files, cert files, system CA dirs, /proc/self, /etc/{hosts,resolv.conf,...},
# dynamic-loader paths, cgroup limits).
#
# --selftest: trace-check.py's own planted-entry tests, then live plants against the plain
# node, each of which must FAIL the check while the control passes: the binary started
# through `sh -c` (an extra execve), a connect to a port that is not the cluster's, and
# the control trace checked against the allow-list without its /proc/meminfo rule (a path the
# list does not cover).
#
# `-s 0`: strace prints pathnames in full regardless of -s, but socket buffers (request bytes,
# which may carry an Authorization header) as `""...`, so traces hold no payload.
# Needs strace on PATH (host strace 6.13, results/local/tools.json; ptrace of an own child
# needs no extra privilege). EDRS_BIN overrides the binary (default: cargo build --release).
# Logs and traces: ${TRACE_LOG_DIR:-$IT_DIR/trace}/ (trace-summary.log, observed.tsv,
# <case>/strace.*). Containers carry elasticdump-rs-it=1 and are removed by down.sh.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD
# shellcheck source=../res/guards.sh
source scripts/res/guards.sh
guard_lock

SELFTEST=0
CASES="plain,tls"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --selftest) SELFTEST=1; shift ;;
    --cases) CASES="$2"; shift 2 ;;
    *) echo "usage: $0 [--selftest] [--cases plain,tls]" >&2; exit 2 ;;
  esac
done
command -v strace >/dev/null || { echo "trace: strace not found" >&2; exit 2; }
IT_DIR="${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/elasticdump-rs-it}"
export IT_DIR
OUT="${TRACE_LOG_DIR:-$IT_DIR/trace}"
rm -rf "$OUT"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
ALLOW="$ROOT/scripts/sup/trace-allow.txt"
CHECK=(python3 "$ROOT/scripts/sup/trace-check.py")
SUMMARY="$OUT/trace-summary.log"
: > "$SUMMARY"
if [[ -n "${EDRS_BIN:-}" ]]; then
  BIN=$(readlink -f "$EDRS_BIN")
else
  cargo build --release --locked -q
  BIN="$ROOT/target/release/elasticdump-rs"
fi
WORK="$OUT/work"
TRACEHOME="$OUT/home"
mkdir -p "$WORK/out" "$WORK/in" "$TRACEHOME"
FAILED=0
trap 'scripts/it/down.sh >/dev/null 2>&1 || true' EXIT

note() { echo "$*" | tee -a "$SUMMARY"; }

# run_case NAME WANT_RC [<STDIN_FILE] -- ARGS...: trace one binary run, keep its output in
# $OUT/NAME/{stdout,stderr}, record the exit status. STDIN/STDOUT redirection via TRACE_IN /
# TRACE_STDOUT.
run_case() {
  local name="$1" want="$2"; shift 2
  local dir="$OUT/$name" rc=0
  mkdir -p "$dir"
  (cd "$WORK" && env -i PATH=/usr/bin:/bin HOME="$TRACEHOME" LANG=C.UTF-8 \
     strace -ff -y -s 0 -e trace=%file,%network,%process -o "$dir/strace" \
     "${TRACE_PREFIX_CMD[@]}" "$BIN" "$@" <"${TRACE_IN:-/dev/null}" >"${TRACE_STDOUT:-$dir/stdout}" 2>"$dir/stderr") || rc=$?
  if [[ "$want" == any || "$rc" == "$want" || ( "$want" == nonzero && "$rc" != 0 ) ]]; then
    note "RUN case=$name rc=$rc want=$want OK"
  else
    note "RUN case=$name rc=$rc want=$want FAIL ($(tail -c 300 "$dir/stderr" | tr '\n' ' '))"
    FAILED=1
  fi
}
TRACE_PREFIX_CMD=()

# check_case NAME [ALLOW_FILE]: allow-list diff of NAME's trace; returns the checker status
check_case() {
  local name="$1" allow="${2:-$ALLOW}" rc=0
  "${CHECK[@]}" --allow "$allow" --case "$name" --cwd "$WORK" --observed "$OUT/observed.tsv" \
    --var "BIN=$BIN" --var "WORK=$WORK" --var "HOME=$TRACEHOME" --var "PORT=$PORT" \
    --var "CA=${CA:-}" --var "CERT=${CERT:-}" --var "KEY=${KEY:-}" --var "KEYENC=${KEYENC:-}" \
    "$OUT/$name/strace" >"$OUT/$name/check.log" 2>&1 || rc=$?
  cat "$OUT/$name/check.log" >> "$SUMMARY"
  tail -n1 "$OUT/$name/check.log"
  return $rc
}

up() {
  scripts/it/down.sh >/dev/null 2>&1 || true
  scripts/it/up.sh "$1" > "$OUT/up-$1.log" 2>&1 || { note "UP $1 FAIL (see $OUT/up-$1.log)"; exit 1; }
  # shellcheck disable=SC1091
  source "$IT_DIR/env"
  U="$IT_ES_URL"
  PORT="${U##*:}"
}

seed() { # seed INDEX N via curl (not traced)
  local idx="$1" n="$2" i
  for ((i = 1; i <= n; i++)); do
    printf '{"index":{"_index":"%s","_id":"%d"}}\n{"n":%d,"s":"%s"}\n' "$idx" "$i" "$i" "$(printf 'x%.0s' $(seq 1 40))"
  done > "$WORK/in/seed.ndjson"
  "${CURL[@]}" -H 'content-type: application/x-ndjson' -XPOST "$U/_bulk?refresh=true" \
    --data-binary @"$WORK/in/seed.ndjson" >/dev/null
}

plain_cases() {
  up opensearch-plain
  CURL=(curl -fsS)
  "${CURL[@]}" -XPUT "$U/tr" -H 'content-type: application/json' \
    -d '{"settings":{"analysis":{"analyzer":{"a":{"type":"custom","tokenizer":"whitespace"}}}},"aliases":{"tr-alias":{}}}' >/dev/null
  "${CURL[@]}" -XPUT "$U/_template/tr-tpl" -H 'content-type: application/json' \
    -d '{"index_patterns":["trx-*"],"settings":{"number_of_shards":1}}' >/dev/null
  seed tr 200
  run_case dump-data 0 --input="$U/tr" --output=out/tr.json --type=data
  local t
  for t in mapping settings alias analyzer; do
    run_case "dump-$t" 0 --input="$U/tr" --output="out/tr.$t.json" --type="$t"
  done
  run_case dump-template 0 --input="$U" --output=out/templates.json --type=template
  run_case split-filesize 0 --input="$U/tr" --output=out/split.json --fileSize=4kb
  run_case split-maxrows 0 --input="$U/tr" --output=out/rows.json --maxRows=50
  run_case dump-gzip 0 --input="$U/tr" --output=out/tr.json.gz --fsCompress
  TRACE_STDOUT="$WORK/out/stdout.json" run_case dump-stdout 0 --input="$U/tr" --output='$'
  run_case restore-settings 0 --input=out/tr.settings.json --output="$U/tr2" --type=settings
  run_case restore-mapping 0 --input=out/tr.mapping.json --output="$U/tr2" --type=mapping
  run_case restore-data 0 --input=out/tr.json --output="$U/tr2" --type=data
  run_case restore-gzip 0 --input=out/tr.json.gz --output="$U/tr3" --fsCompress
  TRACE_IN="$WORK/out/tr.json" run_case restore-stdin 0 --input='$' --output="$U/tr4"
  run_case copy-cluster 0 --input="$U/tr" --output="$U/tr5"
  run_case missing-index nonzero --input="$U/no-such-index" --output=out/missing.json
  local c
  for c in dump-data dump-mapping dump-settings dump-alias dump-analyzer dump-template \
           split-filesize split-maxrows dump-gzip dump-stdout restore-settings restore-mapping \
           restore-data restore-gzip restore-stdin copy-cluster missing-index; do
    check_case "$c" || FAILED=1
  done
  local parts
  parts=$(find "$WORK/out" -name 'split*' | wc -l)
  [[ $parts -ge 2 ]] && note "SPLIT parts=$parts OK" || { note "SPLIT parts=$parts FAIL (want >= 2)"; FAILED=1; }
}

tls_cases() {
  up opensearch-secure
  CA="$IT_CA"; CERT="$IT_CLIENT_CERT"; KEY="$IT_CLIENT_KEY"; KEYENC="$IT_CLIENT_KEY_ENC"
  # the generated test password goes to a 0600 INI file and a curl config, never to a log
  (umask 077; printf 'user=%s\npassword=%s\n' "$IT_ADMIN_USER" "$IT_ADMIN_PW" > "$WORK/in/auth.ini"
   printf 'user = "%s:%s"\n' "$IT_ADMIN_USER" "$IT_ADMIN_PW" > "$OUT/curl.cfg")
  CURL=(curl -fsS --cacert "$CA" -K "$OUT/curl.cfg")
  seed tr 50
  run_case tls-dump 0 --input="$U/tr" --input-ca="$CA" --httpAuthFile=in/auth.ini --output=out/tls.json
  run_case tls-client-cert 0 --input="$U/tr" --input-ca="$CA" --input-cert="$CERT" --input-key="$KEY" --tlsAuth --output=out/tls-cert.json
  run_case tls-restore 0 --input=out/tls.json --output="$U/tr2" --output-ca="$CA" --httpAuthFile=in/auth.ini
  local c
  for c in tls-dump tls-client-cert tls-restore; do check_case "$c" || FAILED=1; done
  rm -f "$OUT/curl.cfg" "$WORK/in/auth.ini"
  unset CA CERT KEY KEYENC
}

selftest() {
  "${CHECK[@]}" --selftest | tee -a "$SUMMARY" | tail -n1
  grep -q '^TRACE_SELFTEST OK' "$SUMMARY" || FAILED=1
  up opensearch-plain
  CURL=(curl -fsS)
  seed tr 20
  local free=1
  # a port nothing listens on (the cluster port + 7 is outside the IT range in use)
  free=$((PORT + 7))
  run_case st-control 0 --input="$U/tr" --output=out/st.json
  TRACE_PREFIX_CMD=(/bin/sh -c '"$0" "$@"')
  run_case st-plant-exec 0 --input="$U/tr" --output=out/st-exec.json
  TRACE_PREFIX_CMD=()
  run_case st-plant-connect nonzero --input="http://127.0.0.1:$free/tr" --output=out/st-conn.json --noImplicitRetry
  grep -v '^path */proc/meminfo ' "$ALLOW" > "$OUT/allow-planted.txt"
  local c want
  for c in st-control st-plant-exec st-plant-connect st-plant-path; do
    case "$c" in
      st-plant-path) mkdir -p "$OUT/$c"; cp "$OUT"/st-control/strace.* "$OUT/$c/"
                     if check_case "$c" "$OUT/allow-planted.txt"; then want=FAIL; else want=OK; fi ;;
      st-control) if check_case "$c"; then want=OK; else want=FAIL; fi ;;
      *) if check_case "$c"; then want=FAIL; else want=OK; fi ;;
    esac
    note "SELFTEST $c $want"
    [[ $want == OK ]] || FAILED=1
  done
}

if [[ $SELFTEST == 1 ]]; then
  selftest
else
  [[ ",$CASES," == *,plain,* ]] && plain_cases
  [[ ",$CASES," == *,tls,* ]] && tls_cases
fi
note "TRACE_DONE $([[ $FAILED == 0 ]] && echo OK || echo FAIL) (summary: $SUMMARY)"
exit $FAILED
