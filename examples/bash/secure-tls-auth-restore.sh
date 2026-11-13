#!/usr/bin/env bash
# Restore over HTTPS with a private CA and optional client certificates.
# Set SEARCHDUMP_OUTPUT_USERNAME / SEARCHDUMP_OUTPUT_PASSWORD when the
# destination user is not the same as the one in HTTP_AUTH_FILE.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_common.sh
source "${ROOT}/_common.sh"

if [[ -z "${HTTP_AUTH_FILE}" && -z "${SEARCHDUMP_OUTPUT_USERNAME:-}" && -z "${SEARCHDUMP_INPUT_USERNAME:-}" ]]; then
  echo "error: set HTTP_AUTH_FILE or SEARCHDUMP_OUTPUT_USERNAME / SEARCHDUMP_OUTPUT_PASSWORD" >&2
  exit 1
fi
if [[ -z "${CA_FILE}" && -z "${OUTPUT_CA_FILE}" ]]; then
  echo "error: set CA_FILE or OUTPUT_CA_FILE to your cluster CA (PEM)" >&2
  exit 1
fi

restore_settings_and_mapping
restore_data_parts --limit=500 --concurrency=2
echo "restored ${INDEX} -> ${DEST_ES_URL}/${DEST_INDEX}"
