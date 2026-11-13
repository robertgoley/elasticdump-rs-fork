#!/usr/bin/env bash
# Restore from s3://. Set DATA_INPUT to the data object (or one split part).
# Prefer searchdump-multi --direction=load --input=s3://bucket/prefix to load
# a whole tree (it stages locally first).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_common.sh
source "${ROOT}/_common.sh"

: "${DATA_INPUT:=${S3_URI}/${INDEX}.data.json}"
: "${LIMIT:=1000}"
: "${CONCURRENCY:=4}"

if [[ -z "${INPUT_STORAGE_CONFIG}" && -z "${SEARCHDUMP_S3_SECRET_ACCESS_KEY:-}" && -z "${AWS_SECRET_ACCESS_KEY:-}" ]]; then
  echo "error: set INPUT_STORAGE_CONFIG or SEARCHDUMP_S3_* / AWS_* credentials" >&2
  exit 1
fi

run_searchdump output \
  --input="${S3_URI}/${INDEX}.settings.json" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=settings
run_searchdump output \
  --input="${S3_URI}/${INDEX}.mapping.json" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=mapping
run_searchdump output \
  --input="${DATA_INPUT}" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=data --limit="${LIMIT}" --concurrency="${CONCURRENCY}"
echo "restored ${DATA_INPUT} -> ${DEST_ES_URL}/${DEST_INDEX}"
