#!/usr/bin/env bash
# Load a searchdump-multi dump directory, including split parts, in order.
# --skipExisting skips an index that already exists on the target.
# On a small-heap target, keep PARALLEL low and pass a modest --limit after --.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_common.sh
source "${ROOT}/_common.sh"

need_bin "${SEARCHDUMP_MULTI_BIN}"

: "${PARALLEL:=2}"
: "${LIMIT:=500}"

MULTI_FLAGS=(
  --direction=load
  --input="${DUMP_DIR}"
  --output="${DEST_ES_URL}"
  --parallel="${PARALLEL}"
)
if [[ -n "${HTTP_AUTH_FILE}" ]]; then
  MULTI_FLAGS+=(--httpAuthFile="${HTTP_AUTH_FILE}")
fi
if [[ -n "${SKIP_EXISTING:-}" ]]; then
  MULTI_FLAGS+=(--skipExisting)
fi
if [[ -n "${PREFIX:-}" ]]; then
  MULTI_FLAGS+=(--prefix="${PREFIX}")
fi
if [[ -n "${SUFFIX:-}" ]]; then
  MULTI_FLAGS+=(--suffix="${SUFFIX}")
fi
"${SEARCHDUMP_MULTI_BIN}" "${MULTI_FLAGS[@]}" -- \
  --limit="${LIMIT}" \
  --concurrency="${CONCURRENCY:-2}" \
  --maxResponseMb="${MAX_RESPONSE_MB:-64}"
echo "loaded ${DUMP_DIR} -> ${DEST_ES_URL}"
