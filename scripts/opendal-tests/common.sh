#!/usr/bin/env bash
# Shared helpers for the OpenDAL storage tests (L-809a). Source this file.
# Containers are named edrs-odt-* and run with --network=host (box quirk); only those are
# started or removed here.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BIN=${EDRS_BIN:-$ROOT/target/debug/elasticdump-rs}
WORK=${ODT_WORK:-$(mktemp -d /tmp/edrs-odt.XXXXXX)}
DOCS=${ODT_DOCS:-5000}
PASS=0
FAIL=0
RESULTS=()

[ -x "$BIN" ] || { echo "build first: cargo build (or set EDRS_BIN)" >&2; exit 2; }
mkdir -p "$WORK"
export NO_PROXY="127.0.0.1,localhost${NO_PROXY:+,$NO_PROXY}"
export no_proxy="$NO_PROXY"

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }

ok() { PASS=$((PASS + 1)); RESULTS+=("PASS $*"); log "PASS $*"; }
bad() { FAIL=$((FAIL + 1)); RESULTS+=("FAIL $*"); log "FAIL $*"; }

# check NAME CMD...: PASS when CMD exits 0
check() { local n=$1; shift; if "$@" >>"$WORK/test.log" 2>&1; then ok "$n"; else bad "$n"; fi; }
# check_fails NAME PATTERN CMD...: PASS when CMD fails and its output matches PATTERN
check_fails() {
  local n=$1 pat=$2; shift 2
  local out
  if out=$("$@" 2>&1); then bad "$n (unexpected success)"; return; fi
  echo "$out" >>"$WORK/test.log"
  if grep -Eq "$pat" <<<"$out"; then ok "$n"; else bad "$n (output did not match /$pat/: $(tail -c 300 <<<"$out"))"; fi
}

summary() {
  echo
  if [ "$FAIL" -eq 0 ] && [ "${KEEP:-0}" != 1 ]; then
    echo "== $1: $PASS passed, $FAIL failed"
    rm -rf "$WORK"
  else
    echo "== $1: $PASS passed, $FAIL failed (log and files kept: $WORK)"
  fi
  printf '  %s\n' "${RESULTS[@]}"
  [ "$FAIL" -eq 0 ]
}

# NDJSON hits like a dump of an index: $DOCS documents with some unicode and big numbers.
make_input() {
  python3 - "$1" "$DOCS" <<'PY'
import json, sys
path, n = sys.argv[1], int(sys.argv[2])
with open(path, "w") as f:
    for i in range(n):
        src = {"n": i, "big": 12345678901234567890123 + i, "t": f"doc {i} \u00e9\u4e2d\U0001F600",
               "pad": "x" * (i % 300)}
        f.write(json.dumps({"_index": "odt", "_id": str(i), "_source": src}, ensure_ascii=False) + "\n")
PY
}

# same_docs A B: both files hold the same documents (order-insensitive)
same_docs() { cmp -s <(sort "$1") <(sort "$2"); }

# Self-signed CA + server certificate for 127.0.0.1/localhost.
make_tls() {
  local d=$1
  mkdir -p "$d"
  openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=edrs-odt-ca" \
    -keyout "$d/ca.key" -out "$d/ca.crt" >/dev/null 2>&1
  openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
    -keyout "$d/server.key" -out "$d/server.csr" >/dev/null 2>&1
  printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\n' >"$d/ext.cnf"
  openssl x509 -req -in "$d/server.csr" -CA "$d/ca.crt" -CAkey "$d/ca.key" -CAcreateserial \
    -days 2 -extfile "$d/ext.cnf" -out "$d/server.crt" >/dev/null 2>&1
}

wait_port() {
  local port=$1 i
  for i in $(seq 1 60); do
    if (exec 3<>/dev/tcp/127.0.0.1/"$port") 2>/dev/null; then return 0; fi
    sleep 1
  done
  return 1
}

stop_container() { podman rm -f "$1" >/dev/null 2>&1 || true; }
