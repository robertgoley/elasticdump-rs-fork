#!/usr/bin/env bash
# Dump a large index in parts (--fileSize, default 1gb) with a longer scroll
# keep-alive. searchdump has no --resume; if this run fails mid-index, check
# the dump directory and usually start the index again (--overwrite replaces
# existing parts). For many indices, use the searchdump-multi examples with
# --resume instead.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_common.sh
source "${ROOT}/_common.sh"

: "${FILE_SIZE:=1gb}"
: "${SCROLL_TIME:=30m}"
: "${LIMIT:=1000}"
: "${WORKERS:=4}"

ensure_dump_dir
dump_settings_and_mapping
input_cluster_flags
"${SEARCHDUMP_BIN}" \
  --input="${ES_URL}/${INDEX}" \
  --output="$(data_stem)" \
  --type=data \
  --overwrite \
  --compressionType=zstd \
  --compressionLevel=1 \
  --fileSize="${FILE_SIZE}" \
  --scrollTime="${SCROLL_TIME}" \
  --limit="${LIMIT}" \
  --workers="${WORKERS}" \
  --maxResponseMb=256 \
  "${INPUT_CLUSTER_FLAGS[@]}"
echo "wrote split parts next to $(data_stem) (fileSize=${FILE_SIZE}, scrollTime=${SCROLL_TIME})"
