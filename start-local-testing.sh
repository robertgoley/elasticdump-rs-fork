#!/usr/bin/env bash
# start-local-testing.sh: prep, start, load and test elasticdump-rs on one local
# machine against podman-hosted Elasticsearch/OpenSearch, one engine at a time.
#
#   ./start-local-testing.sh [options]
#
# Steps (in order; pick with --steps):
#   prep    check/install host tools, Rust 1.91.1, pinned Node elasticdump 6.125.1
#   images  pull every pinned image in scripts/it/images.lock, check the pins
#   build   cargo +1.91.1 build --release --workspace --bins
#   it      integration suite on every pinned profile (scripts/it/run.sh matrix)
#   load    load matrix with the sample data sets on every engine, plain + secure,
#           plus the L1L (1M doc) tier on es717-plain
#   extra   dump-restore scenario, Node comparison (nodecmp), multielasticdump round trip
#   perf    sequential speed suite + Node comparison (scripts/perf/run-suite.sh)
#
# Options:
#   --steps LIST         comma list of steps (default: all of the above)
#   --engines LIST       load/perf engines (default: all; e.g. es717,os13)
#   --tier L0|L1         load/perf tier (default L0; L1L is always the extra es717 tier)
#   --reps N             perf repetitions (default 3)
#   --place PLACE        scripts/load/scenarios/tiers.toml place (default big)
#   --update-baselines   let the perf run record baselines (first run on a machine)
#   --perf-host-tools    run the perf controller on the host (when apt fails in containers)
#   --install-deps       apt-get install missing host packages (uses sudo)
#   --no-pull            don't `git pull --ff-only` first
#   --dry-run            print what would run
#
# Run as your normal user: the suites expect rootless podman. Root works
# (ALLOW_ROOT=1) but uses root's separate image store.
# Credentials: none are needed or stored. The secure profiles generate throwaway
# certs/passwords per run. If an image registry needs a login, run `podman login`
# yourself first; this script never reads or prints auth files.
#
# Output: results/local-testing/<timestamp>/{<step>.log,summary.txt}. A failing
# step is recorded and the next one still runs; the exit status is non-zero if any failed.
set -uo pipefail

cd "$(dirname "$(readlink -f "$0")")"
ROOT=$PWD
STEPS="prep,images,build,it,load,extra,perf"
ENGINES="es710,es717,es819,elasticsearch8,elasticsearch,os13,opensearch"
TIER=L0 REPS=3 PLACE=big UPDATE_BASELINES=0 HOST_TOOLS=0 INSTALL_DEPS=0 PULL=1 DRY=0
TOOLCHAIN=1.91.1
NODE_DIR="${NODE_DIR:-$HOME/.cache/elasticdump-rs/node-ed}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --steps) STEPS=$2; shift 2 ;;
    --engines) ENGINES=$2; shift 2 ;;
    --tier) TIER=$2; shift 2 ;;
    --reps) REPS=$2; shift 2 ;;
    --place) PLACE=$2; shift 2 ;;
    --update-baselines) UPDATE_BASELINES=1; shift ;;
    --perf-host-tools) HOST_TOOLS=1; shift ;;
    --install-deps) INSTALL_DEPS=1; shift ;;
    --no-pull) PULL=0; shift ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done

if [[ $(id -u) -eq 0 && "${ALLOW_ROOT:-0}" != 1 ]]; then
  echo "Run this as your normal user (rootless podman). To run as root anyway: ALLOW_ROOT=1 $0" >&2
  exit 2
fi

TS=$(date +%Y%m%d-%H%M%S)
OUT="$ROOT/results/local-testing/$TS"
mkdir -p "$OUT"
SUMMARY="$OUT/summary.txt"
FAILED=()
export PATH="$HOME/.cargo/bin:$PATH" RUSTUP_TOOLCHAIN=$TOOLCHAIN
export NODE_ED_BIN="$NODE_DIR/node_modules/.bin" NODE_ED="$NODE_DIR/node_modules/.bin/elasticdump"

has_step() { [[ ",$STEPS," == *",$1,"* ]]; }
log() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$SUMMARY"; }

# run NAME CMD...: log to $OUT/NAME.log (also shown live), record pass/fail, keep going.
run() {
  local name=$1; shift
  if [[ $DRY -eq 1 ]]; then echo "DRY $name: $*"; return 0; fi
  log "START $name: $*"
  local s=$SECONDS
  "$@" 2>&1 | tee "$OUT/$name.log"
  local rc=${PIPESTATUS[0]}
  if [[ $rc -eq 0 ]]; then log "PASS  $name ($((SECONDS - s))s)"
  else log "FAIL  $name rc=$rc ($((SECONDS - s))s), see $OUT/$name.log"; FAILED+=("$name"); fi
  # never leave an engine running between steps
  scripts/it/down.sh >/dev/null 2>&1 || true
  return 0
}

cleanup() { [[ $DRY -eq 1 ]] || scripts/it/down.sh >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'log "interrupted"; exit 130' INT TERM

# ---------------------------------------------------------------- prep
step_prep() {
  local missing=() t
  for t in podman git curl openssl python3 node npm; do command -v "$t" >/dev/null || missing+=("$t"); done
  if [[ ${#missing[@]} -gt 0 ]]; then
    if [[ $INSTALL_DEPS -eq 1 ]]; then
      local pkgs=() m
      for m in "${missing[@]}"; do case $m in node) pkgs+=(nodejs) ;; *) pkgs+=("$m") ;; esac; done
      sudo apt-get update && sudo apt-get install -y "${pkgs[@]}" uidmap slirp4netns passt fuse-overlayfs || return 1
    else
      echo "missing host tools: ${missing[*]} (re-run with --install-deps, or install them)"; return 1
    fi
  fi
  python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' \
    || { echo "python3 3.11+ required"; return 1; }
  [[ $(podman info --format '{{.Host.Security.Rootless}}' 2>/dev/null) == true || $(id -u) -eq 0 ]] \
    || { echo "podman is not usable rootless for $USER (check /etc/subuid and /etc/subgid)"; return 1; }
  if ! command -v rustup >/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none || return 1
  fi
  rustup toolchain install "$TOOLCHAIN" --profile minimal --component rustfmt,clippy || return 1
  if [[ "$("$NODE_ED" --version 2>/dev/null)" != *6.125.1* ]]; then
    mkdir -p "$NODE_DIR"
    cp scripts/perf/node-package.json "$NODE_DIR/package.json"
    cp scripts/perf/node-package-lock.json "$NODE_DIR/package-lock.json"
    (cd "$NODE_DIR" && npm ci --ignore-scripts --no-audit --no-fund) || return 1
  fi
  "$NODE_ED" --version
  echo "prep ok: podman $(podman --version | awk '{print $3}'), rust $TOOLCHAIN, node elasticdump at $NODE_ED_BIN"
}

# ---------------------------------------------------------------- images
step_images() {
  local img dig rc=0
  while read -r img dig; do
    [[ -z "$img" || "$img" == \#* ]] && continue
    if podman image exists "${img%%:*}@$dig"; then echo "have $img"; continue; fi
    echo "pull $img@$dig"
    podman pull "${img%%:*}@$dig" || rc=1
  done < scripts/it/images.lock
  scripts/it/check-pins.sh || rc=1
  return $rc
}

# ---------------------------------------------------------------- build
step_build() {
  cargo +"$TOOLCHAIN" build --locked --release --workspace --bins
}

# ---------------------------------------------------------------- load
step_load_engine() {   # $1 = engine-variant
  scripts/load/matrix.sh "$TIER" --place "$PLACE" --engine "$1" --run-id "lt-$TS-$TIER-$1"
}

# ---------------------------------------------------------------- main
log "start-local-testing: steps=$STEPS engines=$ENGINES tier=$TIER place=$PLACE reps=$REPS out=$OUT"
if [[ $PULL -eq 1 && $DRY -eq 0 ]]; then
  git pull --ff-only || log "git pull --ff-only failed; continuing with the current checkout"
fi
log "commit $(git rev-parse --short HEAD)"

has_step prep   && run prep step_prep
has_step images && run images step_images
has_step build  && run build step_build
has_step it     && run it scripts/it/run.sh matrix

if has_step load; then
  IFS=, read -ra ENG <<< "$ENGINES"
  for e in "${ENG[@]}"; do
    for v in plain secure; do run "load-$TIER-$e-$v" step_load_engine "$e-$v"; done
  done
  run load-L1L-es717-plain scripts/load/matrix.sh L1L --place "$PLACE" --engine es717-plain --run-id "lt-$TS-L1L-es717-plain"
fi

if has_step extra; then
  run dump-restore scripts/load/run.sh "$TIER" dump-restore-logs --place "$PLACE" --engine es717-plain --run-id "lt-$TS-drl"
  run nodecmp python3 scripts/load/nodecmp.py "$TIER" --place "$PLACE" --reps 3 --node "$NODE_ED"
  run multielasticdump-roundtrip bash scripts/it/multielasticdump-roundtrip.sh target/release
fi

if has_step perf; then
  perf_args=(--tier "$TIER" --place "$PLACE" --reps "$REPS")
  if [[ "$ENGINES" != "es710,es717,es819,elasticsearch8,elasticsearch,os13,opensearch" ]]; then
    pe=(); IFS=, read -ra ENG <<< "$ENGINES"
    for e in "${ENG[@]}"; do
      # The speed suite tests OpenSearch 2 and 3 separately (os2, os3); the
      # load and integration suites call the current OpenSearch "opensearch".
      case $e in
        opensearch) pe+=(os2-plain os2-secure os3-plain os3-secure) ;;
        *) pe+=("$e-plain" "$e-secure") ;;
      esac
    done
    perf_args+=(--engines "$(IFS=,; echo "${pe[*]}")")
  fi
  [[ $UPDATE_BASELINES -eq 1 ]] && perf_args+=(--update-baselines)
  [[ $HOST_TOOLS -eq 1 ]] && perf_args+=(--host-tools)
  PERF_HOST_ID="${PERF_HOST_ID:-$(hostname -s)}" run perf scripts/perf/run-suite.sh "${perf_args[@]}"
fi

[[ $DRY -eq 1 ]] && exit 0
log "done. ${#FAILED[@]} failed step(s)${FAILED:+: ${FAILED[*]}}"
log "summary: $SUMMARY; load results: results/load/lt-$TS-*; perf: results/perf/*/summary.md"
[[ ${#FAILED[@]} -eq 0 ]]
