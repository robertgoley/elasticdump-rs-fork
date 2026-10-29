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

cargo build --profile it --features it --tests --bins -j "${CARGO_JOBS:-4}"

run_profile() {
  local p="$1"; shift
  trap 'scripts/it/down.sh' RETURN
  scripts/it/up.sh "$p"
  # shellcheck disable=SC1091
  source "$IT_DIR/env"
  cargo test --profile it --features it --test it -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}" "$@"
  if [[ "$p" == opensearch-plain && "${IT_LEGACY:-1}" == 1 ]]; then
    cargo test --profile it --test integration_test -j "${CARGO_JOBS:-4}" -- --test-threads="${IT_THREADS:-4}" "$@"
  fi
}

case "$PROFILE" in
  all)
    run_profile opensearch-plain "$@"
    run_profile opensearch-secure "$@"
    ;;
  *) run_profile "$PROFILE" "$@" ;;
esac
