#!/usr/bin/env bash
# Build first (light `it` profile), then start OpenSearch, run the suites, and
# always tear the container down.
#
#   scripts/it/run.sh PROFILE [extra cargo test args]
#
# PROFILE: opensearch-plain | opensearch-secure | elasticsearch-plain |
#          elasticsearch-secure | elasticsearch8-plain | elasticsearch8-secure |
#          cross | opensearch | elasticsearch | elasticsearch8 | all |
#          es717-{plain,secure} | es710-{plain,secure} | es819-{plain,secure} |
#          os13-{plain,secure} | es717 | es710 | es819 | os13 | matrix
#   opensearch/elasticsearch/elasticsearch8 = both variants of that engine;
#   elasticsearch = latest bitnamilegacy tag (ES_TAG, 9.1.2), elasticsearch8 =
#   the pinned regression tag (ES8_TAG, 8.6.1). all = every single-engine
#   profile (OS_TAG as given), then the last cross-engine file leg, then cross.
#   Single-engine profiles run one container at a time.
#
# Cross-engine coverage:
#   * file dump -> restore runs sequentially: each plain profile exports to
#     $IT_CROSS_DIR/<engine>/ and restores what the other engine exported
#     earlier; `all` restarts opensearch-plain at the end (cross:: tests only)
#     so both directions are covered with one container at a time.
#   * cluster -> cluster (`cross`) needs one plain node of each engine at once
#     (heaps ${CROSS_HEAP:-256m}, IT_LEAN_JVM=1). It only starts after checking
#     MemAvailable >= CROSS_MIN_MB (default 2200, measured: ES 9 ~700 MB RSS
#     idle at a 256m heap plus its launcher, OpenSearch ~600 MB, plus headroom
#     for load); otherwise it prints SKIP and exits 0 (CROSS_REQUIRED=1 → 1).
#
# Building before starting containers matters: a fat-LTO build next to a
# running OpenSearch node has exceeded a 16 GB memory budget.
set -euo pipefail
cd "$(dirname "$0")/../.."
PROFILE="${1:-all}"
shift || true
IT_DIR="${IT_DIR:-${TMPDIR:-/tmp}/elasticdump-rs-it}"
export IT_DIR
IT_CROSS_DIR="${IT_CROSS_DIR:-$IT_DIR-cross}"
export IT_CROSS_DIR
ES8_TAG="${ES8_TAG:-8.6.1-debian-11-r0}"
# Regression matrix (exact tags verified against the registries 2026-10-29;
# see TESTING-MATRIX.md for sources and digests):
OFFICIAL_ES=docker.elastic.co/elasticsearch/elasticsearch
ES717_IMAGE="${ES717_IMAGE:-$OFFICIAL_ES:7.17.29}"      # bitnamilegacy stops at 7.17.26
ES710_PLAIN_IMAGE="${ES710_PLAIN_IMAGE:-docker.io/bitnamilegacy/elasticsearch:7.10.2}"  # OSS build
ES710_SECURE_IMAGE="${ES710_SECURE_IMAGE:-$OFFICIAL_ES:7.10.2}"  # bitnami 7.10.2 has no X-Pack security
ES819_IMAGE="${ES819_IMAGE:-$OFFICIAL_ES:8.19.23}"      # bitnamilegacy stops at 8.18.0
OS13_TAG="${OS13_TAG:-1.3.20}"

# Map a matrix profile (es717-plain, os13-secure, ...) onto up.sh's engine
# profile plus image env. Prints the up.sh profile.
matrix_env() {
  local p="$1" variant="${1##*-}"
  case "$p" in
    elasticsearch8-*) export ES_TAG="$ES8_TAG" ES_IMAGE="docker.io/bitnamilegacy/elasticsearch:$ES8_TAG"; echo "elasticsearch-$variant" ;;
    es717-*) export ES_TAG=7.17.29 ES_IMAGE="$ES717_IMAGE"; echo "elasticsearch-$variant" ;;
    es819-*) export ES_TAG=8.19.23 ES_IMAGE="$ES819_IMAGE"; echo "elasticsearch-$variant" ;;
    es710-plain) export ES_TAG=7.10.2 ES_IMAGE="$ES710_PLAIN_IMAGE"; echo elasticsearch-plain ;;
    es710-secure) export ES_TAG=7.10.2 ES_IMAGE="$ES710_SECURE_IMAGE"; echo elasticsearch-secure ;;
    os13-*) export OS_TAG="$OS13_TAG" OS_IMAGE="docker.io/opensearchproject/opensearch:$OS13_TAG"; echo "opensearch-$variant" ;;
    *) echo "$p" ;;
  esac
}

# Every test binary is compiled up front with ONE feature set; a later
# `cargo test` with different features would recompile next to the running
# node and risk the OOM killer.
cargo build --profile it --features it --tests --bins -j "${CARGO_JOBS:-4}"
cargo test --profile it --features it --tests --no-run -j "${CARGO_JOBS:-4}"

# Note: a RETURN trap would fire when `source` below finishes, so cleanup is
# an EXIT trap plus an explicit down.sh per profile.
trap 'scripts/it/down.sh' EXIT

run_profile() {
  local p="$1"; shift
  # matrix profiles only change images for this profile (subshell)
  if [[ "$(matrix_env "$p")" != "$p" ]]; then
    ( local up; up=$(matrix_env "$p"); matrix_env "$p" >/dev/null; run_profile "$up" "$@" )
    return
  fi
  scripts/it/up.sh "$p"
  # shellcheck disable=SC1091
  source "$IT_DIR/env"
  # --show-output keeps the "SKIP: <reason>" lines of tests that do not apply
  # to this version; summarised after the run
  cargo test --profile it --features it --test it -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}" --show-output "$@" 2>&1 \
    | tee "$IT_DIR/last-it.log" | grep -vE '^\[[0-9TZ:-]+ (DEBUG|INFO|TRACE)' || true
  local skips
  skips=$(grep -cE '^SKIP' "$IT_DIR/last-it.log" || true)
  echo "IT-SUMMARY profile=$p image=${ES_IMAGE:-}${OS_IMAGE:+ os=$OS_IMAGE} $(grep -E '^test result' "$IT_DIR/last-it.log" | tail -1) skipped-by-reason=${skips}"
  grep -E '^SKIP' "$IT_DIR/last-it.log" | sort | uniq -c || true
  grep -qE '^test result: ok' "$IT_DIR/last-it.log"
  if [[ "$p" == *-plain && "${IT_LEGACY:-1}" == 1 ]]; then
    cargo test --profile it --features it --test integration_test -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}" "$@"
  fi
  scripts/it/down.sh
}

mem_available_mb() { awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo; }

run_cross() {
  local heap="${CROSS_HEAP:-256m}" need="${CROSS_MIN_MB:-2200}" have
  have=$(mem_available_mb)
  if (( have < need )); then
    echo "SKIP cross: two nodes with ${heap} heaps need ~${need} MB, only ${have} MB available" >&2
    [[ "${CROSS_REQUIRED:-0}" == 1 ]] && return 1
    return 0
  fi
  IT_LEAN_JVM=1 OS_HEAP="$heap" scripts/it/up.sh opensearch-plain
  have=$(mem_available_mb)
  if (( have < need * 6 / 10 )); then
    echo "SKIP cross: only ${have} MB left after the first node" >&2
    scripts/it/down.sh
    [[ "${CROSS_REQUIRED:-0}" == 1 ]] && return 1
    return 0
  fi
  mkdir -p "$IT_DIR/peer"
  IT_LEAN_JVM=1 IT_DIR="$IT_DIR/peer" OS_HEAP="$heap" scripts/it/up.sh elasticsearch-plain
  # shellcheck disable=SC1091
  source "$IT_DIR/env"
  export IT_PEER_URL IT_PEER_ENGINE
  IT_PEER_URL=$(sed -n 's/^export IT_ES_URL=//p' "$IT_DIR/peer/env")
  IT_PEER_ENGINE=elasticsearch
  cargo test --profile it --features it --test it -j "${CARGO_JOBS:-4}" -- --test-threads=1 cross::cluster_to_cluster "$@"
  unset IT_PEER_URL IT_PEER_ENGINE
  IT_DIR="$IT_DIR/peer" scripts/it/down.sh
  scripts/it/down.sh
}

case "$PROFILE" in
  all)
    for p in opensearch-plain opensearch-secure elasticsearch-plain elasticsearch-secure; do
      run_profile "$p" "$@"
    done
    for p in elasticsearch8-plain elasticsearch8-secure; do
      run_profile "$p" "$@"
    done
    # last file leg: OpenSearch restores the Elasticsearch export
    IT_LEGACY=0 run_profile opensearch-plain cross::file_dump_from "$@"
    run_cross "$@"
    rm -rf "$IT_CROSS_DIR"
    ;;
  opensearch|elasticsearch|elasticsearch8|es717|es710|es819|os13)
    run_profile "$PROFILE-plain" "$@"
    run_profile "$PROFILE-secure" "$@"
    ;;
  matrix)
    # the older/extra versions, one container at a time
    for p in es717 es710 es819 os13; do
      run_profile "$p-plain" "$@"
      run_profile "$p-secure" "$@"
    done
    ;;
  cross) run_cross "$@" ;;
  *) run_profile "$PROFILE" "$@" ;;
esac
