#!/usr/bin/env bash
# Restore settings, mapping and data from s3://. If the dump was split, this
# script expects you to set DATA_INPUT to one part (or the unsplit object);
# pass further parts by running again or by listing keys yourself. Prefer
# searchdump-multi --direction=load --input=s3://bucket/prefix for a whole
# tree (it stages locally first).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_common.sh
source "${ROOT}/_common.sh"

: "${DATA_INPUT:=${S3_URI}/${INDEX}.data.json}"

if [[ -z "${INPUT_STORAGE_CONFIG}" && -z "${SEARCHDUMP_S3_SECRET_ACCESS_KEY:-}" && -z "${AWS_SECRET_ACCESS_KEY:-}" ]]; then
  echo "error: set INPUT_STORAGE_CONFIG or SEARCHDUMP_S3_* / AWS_* credentials" >&2
  exit 1
fi

output_cluster_flags
STORAGE_FLAGS=()
if [[ -n "${INPUT_STORAGE_CONFIG}" ]]; then
  STORAGE_FLAGS+=(--inputStorageConfig="${INPUT_STORAGE_CONFIG}")
fi

"${SEARCHDUMP_BIN}" \
  --input="${S3_URI}/${INDEX}.settings.json" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=settings \
  "${OUTPUT_CLUSTER_FLAGS[@]}" \
  "${STORAGE_FLAGS[@]}"
"${SEARCHDUMP_BIN}" \
  --input="${S3_URI}/${INDEX}.mapping.json" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=mapping \
  "${OUTPUT_CLUSTER_FLAGS[@]}" \
  "${STORAGE_FLAGS[@]}"
"${SEARCHDUMP_BIN}" \
  --input="${DATA_INPUT}" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=data \
  --limit="${LIMIT:-1000}" \
  --concurrency="${CONCURRENCY:-4}" \
  "${OUTPUT_CLUSTER_FLAGS[@]}" \
  "${STORAGE_FLAGS[@]}"
echo "restored ${DATA_INPUT} -> ${DEST_ES_URL}/${DEST_INDEX}"
