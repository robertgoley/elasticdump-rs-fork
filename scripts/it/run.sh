#!/usr/bin/env bash
# Build first (light `it` profile), then start OpenSearch, run the suites, and
# always tear the container down.
#
#   scripts/it/run.sh PROFILE [extra cargo test args]
#
# PROFILE: opensearch-plain | opensearch-secure | elasticsearch-plain |
#          elasticsearch-secure | elasticsearch8-plain | elasticsearch8-secure |
#          cross | opensearch | elasticsearch | elasticsearch8 | all
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
if [[ "$PROFILE" == elasticsearch8* ]]; then
  export ES_TAG="$ES8_TAG"
  PROFILE="${PROFILE/elasticsearch8/elasticsearch}"
fi

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
  scripts/it/up.sh "$p"
  # shellcheck disable=SC1091
  source "$IT_DIR/env"
  cargo test --profile it --features it --test it -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}" "$@"
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
    for p in elasticsearch-plain elasticsearch-secure; do
      ES_TAG="$ES8_TAG" run_profile "$p" "$@"
    done
    # last file leg: OpenSearch restores the Elasticsearch export
    IT_LEGACY=0 run_profile opensearch-plain cross::file_dump_from "$@"
    run_cross "$@"
    rm -rf "$IT_CROSS_DIR"
    ;;
  opensearch|elasticsearch)
    run_profile "$PROFILE-plain" "$@"
    run_profile "$PROFILE-secure" "$@"
    ;;
  cross) run_cross "$@" ;;
  *) run_profile "$PROFILE" "$@" ;;
esac
