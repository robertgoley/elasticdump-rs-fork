#!/usr/bin/env bash
# L-701 (TST-SUP-001): egress-denied test runs.
#
#   scripts/sup/egress.sh [--tiers T1,T2,T7] [--profile PROFILE] [--image IMAGE] [--selftest]
#
# Everything runs inside one rootless pod created with `podman pod create --network none`
# (label searchdump-it=1): its network namespace has loopback only, so nothing can
# leave the box, and nothing on the host (firewall, routes, other containers) is touched.
# Inside it:
#   * a logging DNS sink (egress-sink.py) on 127.0.0.1:53 / [::1]:53 answers every query with
#     NXDOMAIN and logs the name; test processes and containers get a resolv.conf pointing at it;
#   * the kernel counters Ip:OutNoRoutes / Ip6OutNoRoutes of that namespace count every attempt
#     to reach a non-loopback address (TCP, UDP, a DNS query to an outside resolver).
# Phases (each counted on its own):
#   T1  unit + contract nextest profiles, plus the egress plant (tests/egress_plant.rs)
#   T7  chaos nextest profile (mock-based fault injection)
#   T2  the IT suite against PROFILE (default opensearch-plain), the node running in the pod
#       (scripts/it/up.sh with IT_POD), then the built image (IMAGE, default
#       localhost/searchdump-rs:<version>) dumping and restoring an index from the pod
# The run fails when a phase fails, or when any phase made an unexpected connection attempt
# or DNS query. Expected names: localhost, and the reserved *.invalid / *.test / *.example /
# *.localhost names tests use to provoke resolution failures (EGRESS_ALLOW_DNS overrides the
# regex). The node's own start-up is reported as phase `node-start` and counts too.
#
# EDRS_EGRESS_PLANT=dns|tcp plants a probe (resolve example.com / connect to 192.0.2.1) in T1;
# the run must then fail. --selftest runs the plants and a clean control (T1 plant phase only)
# and passes only if each plant is caught and the control is clean.
# Logs: ${EGRESS_LOG_DIR:-$IT_DIR/egress}/ (dns.log, phase logs, egress-summary.log).
set -euo pipefail
cd "$(dirname "$0")/../.."
# one IT/load/egress run at a time (TST-INF-007); down.sh removes every searchdump-it=1 container
# shellcheck source=../res/guards.sh
source scripts/res/guards.sh
guard_lock

TIERS="T1,T2,T7"
PROFILE="opensearch-plain"
IMAGE=""
SELFTEST=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tiers) TIERS="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --image) IMAGE="$2"; shift 2 ;;
    --selftest) SELFTEST=1; shift ;;
    *) echo "usage: $0 [--tiers T1,T2,T7] [--profile PROFILE] [--image IMAGE] [--selftest]" >&2; exit 2 ;;
  esac
done
ALLOW="${EGRESS_ALLOW_DNS:-^(localhost|.*\.localhost|.*\.invalid|.*\.test|.*\.example|invalid|test|example)\.?$}"
IT_DIR="${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/searchdump-it}"
export IT_DIR
OUT="${EGRESS_LOG_DIR:-$IT_DIR/egress}"
mkdir -p "$OUT"
POD="searchdump-egress-$$"
export EGRESS_DIR="$OUT/ns"
mkdir -p "$EGRESS_DIR"
printf 'nameserver 127.0.0.1\noptions timeout:1 attempts:1\n' > "$EGRESS_DIR/resolv.conf"
# The pod's hostname is its name; the node looks itself up at start. `--add-host` is ignored
# with --network none, so the node gets this hosts file as a volume instead.
printf '127.0.0.1\tlocalhost %s\n::1\tlocalhost ip6-localhost ip6-loopback %s\n' "$POD" "$POD" > "$EGRESS_DIR/hosts"
: > "$OUT/dns.log"
: > "$OUT/egress-summary.log"
NS=(scripts/sup/netns-exec.sh "$POD")
# Fill the registry cache (all features, incl. optional crates nextest's `cargo metadata
# --all-features` resolves) while the host still has network; everything after this is offline.
# A fresh CI runner has an empty cache (release.yml v1.0.0-beta1 failed here on `lipsum`).
if ! cargo fetch --locked >/dev/null 2>&1; then
  echo "egress: warning: cargo fetch --locked failed; continuing with the cached registry" >&2
fi
export CARGO_NET_OFFLINE=true

cleanup() {
  # the sink is a host process inside the pod's namespace; removing the pod does not stop it
  pkill -f "egress-sink.py $OUT/dns.log" 2>/dev/null || true
  scripts/it/down.sh >/dev/null 2>&1 || true
  podman pod rm -f "$POD" >/dev/null 2>&1 || true
}
trap cleanup EXIT

podman pod create --name "$POD" --network none --label searchdump-it=1 >/dev/null
podman pod start "$POD" >/dev/null
rm -f "$OUT/sink.ready"
"${NS[@]}" python3 scripts/sup/egress-sink.py "$OUT/dns.log" "$OUT/sink.ready" > "$OUT/sink.out" 2>&1 8>&- &
for _ in $(seq 1 50); do [[ -e "$OUT/sink.ready" ]] && break; sleep 0.2; done
[[ -e "$OUT/sink.ready" ]] || { echo "egress: DNS sink did not start" >&2; cat "$OUT/sink.out" >&2; exit 1; }

noroutes() {
  "${NS[@]}" sh -c "awk '/^Ip: [0-9]/ {print \$13}' /proc/net/snmp; awk '\$1==\"Ip6OutNoRoutes\" {print \$2}' /proc/net/snmp6" \
    | awk '{s+=$1} END {print s+0}'
}

FAILED=0
# phase NAME CMD... : run CMD (already wrapped as needed), then count what it tried to reach
phase() {
  local name="$1"; shift
  local before lines rc=0 after dns_all dns_bad
  before=$(noroutes)
  lines=$(wc -l < "$OUT/dns.log")
  echo "== egress phase $name: $*" >&2
  "$@" > "$OUT/phase-$name.log" 2>&1 || rc=$?
  sleep 1 # let the sink log a last query
  after=$(noroutes)
  dns_all=$(tail -n +"$((lines + 1))" "$OUT/dns.log" | awk '{print $3}')
  dns_bad=$(printf '%s\n' "$dns_all" | grep -v '^$' | grep -cvE "$ALLOW" || true)
  local nr=$((after - before))
  local names
  names=$(printf '%s\n' "$dns_all" | grep -v '^$' | grep -vE "$ALLOW" | sort -u | tr '\n' ' ' || true)
  local verdict=OK
  if [[ $rc -ne 0 || $nr -ne 0 || $dns_bad -ne 0 ]]; then verdict=FAIL; FAILED=1; fi
  echo "EGRESS phase=$name rc=$rc unexpected_dns=$dns_bad noroute=$nr names=[${names% }] $verdict" \
    | tee -a "$OUT/egress-summary.log"
  [[ $rc -eq 0 ]] || tail -n 30 "$OUT/phase-$name.log" >&2
  return 0
}

has() { [[ ",$TIERS," == *",$1,"* ]]; }

if [[ $SELFTEST -eq 1 ]]; then
  cargo test -q --test egress_plant --no-run
  phase plant-dns "${NS[@]}" env EDRS_EGRESS_PLANT=dns cargo test -q --test egress_plant
  phase plant-tcp "${NS[@]}" env EDRS_EGRESS_PLANT=tcp cargo test -q --test egress_plant
  phase control "${NS[@]}" env -u EDRS_EGRESS_PLANT cargo test -q --test egress_plant
  want="plant-dns:FAIL plant-tcp:FAIL control:OK"
  got=$(awk '{split($2,p,"="); print p[2] ":" $NF}' "$OUT/egress-summary.log" | tr '\n' ' ')
  if [[ "${got% }" == "$want" ]] \
    && grep -q '^EGRESS phase=plant-dns .*names=\[example.com\]' "$OUT/egress-summary.log" \
    && grep -qE '^EGRESS phase=plant-tcp .*noroute=[1-9]' "$OUT/egress-summary.log"; then
    echo "EGRESS-SELFTEST OK (plants caught, control clean)" | tee -a "$OUT/egress-summary.log"
    exit 0
  fi
  echo "EGRESS-SELFTEST FAIL: expected [$want], got [${got% }]" | tee -a "$OUT/egress-summary.log"
  exit 1
fi

if has T1; then
  cargo nextest run --profile unit --no-run
  cargo nextest run --profile contract --no-run
  cargo test -q --test egress_plant --no-run
  phase T1-unit "${NS[@]}" cargo nextest run --profile unit
  phase T1-contract "${NS[@]}" cargo nextest run --profile contract
  phase T1-plant "${NS[@]}" cargo test -q --test egress_plant
fi
if has T7; then
  cargo nextest run --profile chaos --no-run
  phase T7-chaos "${NS[@]}" cargo nextest run --profile chaos
fi
if has T2; then
  cargo build --profile it --features it --tests --bins -j "${CARGO_JOBS:-4}"
  cargo test --profile it --features it --tests --no-run -j "${CARGO_JOBS:-4}"
  phase node-start env IT_POD="$POD" IT_POD_RESOLV="$EGRESS_DIR/resolv.conf" IT_POD_HOSTS="$EGRESS_DIR/hosts" \
    scripts/it/up.sh "$PROFILE"
  # shellcheck disable=SC1091
  source "$IT_DIR/env"
  phase T2-it "${NS[@]}" env IT_ES_URL="$IT_ES_URL" \
    cargo test --profile it --features it --test it -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}"
  if [[ -z "$IMAGE" ]]; then
    IMAGE="localhost/searchdump-rs:$(awk -F'"' '/^version/ {print $2; exit}' Cargo.toml)"
  fi
  if podman image exists "$IMAGE"; then
    data="$OUT/image-data"
    rm -rf "$data"; mkdir -p "$data"; chmod 0777 "$data"
    run_image() {
      podman run --rm --pod "$POD" -v "$EGRESS_DIR/resolv.conf:/etc/resolv.conf:ro" \
        -v "$EGRESS_DIR/hosts:/etc/hosts:ro" \
        -v "$data:/data:Z" "$IMAGE" "$@"
    }
    image_round_trip() {
      local u="$IT_ES_URL" c=(curl -fsS -k)
      [[ -n "${IT_ADMIN_USER:-}" ]] && c+=(-u "${IT_ADMIN_USER}:${IT_ADMIN_PW}")
      "${NS[@]}" "${c[@]}" -XPUT "$u/egress_probe/_doc/1?refresh=true" \
        -H 'Content-Type: application/json' -d '{"n":1}' >/dev/null
      run_image --input="$u/egress_probe" --output=/data/probe.json --type=data
      run_image --input=/data/probe.json --output="$u/egress_probe_copy" --type=data
      "${NS[@]}" "${c[@]}" "$u/egress_probe_copy/_refresh" >/dev/null
      "${NS[@]}" "${c[@]}" "$u/egress_probe_copy/_count" | grep -q '"count":1'
    }
    if [[ -z "${IT_ADMIN_USER:-}" ]]; then
      phase T2-image image_round_trip
    else
      echo "EGRESS phase=T2-image SKIPPED (image round trip runs on plain profiles)" | tee -a "$OUT/egress-summary.log"
    fi
  else
    echo "EGRESS phase=T2-image rc=1 image $IMAGE not built (make image) FAIL" | tee -a "$OUT/egress-summary.log"
    FAILED=1
  fi
fi

if [[ $FAILED -ne 0 ]]; then
  echo "EGRESS FAIL (see $OUT/egress-summary.log)" | tee -a "$OUT/egress-summary.log"
  exit 1
fi
echo "EGRESS OK: 0 unexpected connections or DNS queries" | tee -a "$OUT/egress-summary.log"
