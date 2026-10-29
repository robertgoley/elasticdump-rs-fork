#!/usr/bin/env bash
# Build first (light `it` profile), then start OpenSearch, run the suites, and
# always tear the container down.
#
#   scripts/it/run.sh [opensearch-plain|opensearch-secure|all] [extra cargo test args]
#
# Building before starting containers matters: a fat-LTO build next to a
# running OpenSearch node has exceeded a 16 GB memory budget.
set -euo pipefail
cd "$(dirname "$0")/../.."
PROFILE="${1:-all}"
shift || true
IT_DIR="${IT_DIR:-${TMPDIR:-/tmp}/elasticdump-rs-it}"
export IT_DIR

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
  if [[ "$p" == opensearch-plain && "${IT_LEGACY:-1}" == 1 ]]; then
    cargo test --profile it --features it --test integration_test -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}" "$@"
  fi
  scripts/it/down.sh
}

case "$PROFILE" in
  all)
    run_profile opensearch-plain "$@"
    run_profile opensearch-secure "$@"
    ;;
  *) run_profile "$PROFILE" "$@" ;;
esac
