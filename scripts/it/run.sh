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
IT_DIR="${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/searchdump-it}"
export IT_DIR
IT_CROSS_DIR="${IT_CROSS_DIR:-$IT_DIR-cross}"
export IT_CROSS_DIR
# TST-INF-007 guards: one IT/load run at a time on the box; memory and disk
# preflight per profile (SKIP + exit 0, or exit 1 with IT_REQUIRED=1).
# shellcheck source=../res/guards.sh
source scripts/res/guards.sh
guard_lock
# shellcheck source=matrix.sh
source scripts/it/matrix.sh

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
  local heap need
  heap=$(mb_of "${ES_HEAP:-${OS_HEAP:-512m}}")
  need="${IT_MIN_MB:-$(( heap * 2 + 600 ))}"
  if ! guard_mem "$need" "profile $p" || ! guard_disk "$IT_DIR" "${IT_MIN_DISK_MB:-1500}" "profile $p"; then
    echo "IT-SUMMARY profile=$p SKIPPED (resource guard)" | tee -a "$IT_DIR/it-summary.log"
    skip_or_fail IT_REQUIRED
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
  echo "IT-SUMMARY profile=$p image=${ES_IMAGE:-}${OS_IMAGE:+ os=$OS_IMAGE} $(grep -E '^test result' "$IT_DIR/last-it.log" | tail -1) skipped-by-reason=${skips}" \
    | tee -a "$IT_DIR/it-summary.log"  # INF-011: scripts/it/matrix-md.py --summary
  grep -E '^SKIP' "$IT_DIR/last-it.log" | sort | uniq -c || true
  grep -qE '^test result: ok' "$IT_DIR/last-it.log"
  if [[ "$p" == *-plain && "${IT_LEGACY:-1}" == 1 ]]; then
    cargo test --profile it --features it --test integration_test -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}" "$@"
  fi
  scripts/it/down.sh
}

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
