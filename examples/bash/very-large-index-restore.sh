#!/usr/bin/env bash
# Restore a split dump. searchdump takes one invocation per part, in order
# (as in Node). searchdump-multi --direction=load finds the parts itself; see
# multi-index-restore.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_common.sh
source "${ROOT}/_common.sh"

: "${LIMIT:=1000}"
: "${CONCURRENCY:=4}"

restore_settings_and_mapping
restore_data_parts --limit="${LIMIT}" --concurrency="${CONCURRENCY}" --workers=4
echo "restored split dump of ${INDEX} -> ${DEST_ES_URL}/${DEST_INDEX}"
