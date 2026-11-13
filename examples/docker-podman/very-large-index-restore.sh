#!/usr/bin/env bash
# Restore a split dump: one searchdump invocation per part, in order, run
# inside the image so the glob sees /data.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./_common.sh
source "${ROOT}/_common.sh"

: "${LIMIT:=1000}"
: "${CONCURRENCY:=4}"

run_searchdump output \
  --input="/data/${INDEX}.settings.json" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=settings
run_searchdump output \
  --input="/data/${INDEX}.mapping.json" \
  --output="${DEST_ES_URL}/${DEST_INDEX}" \
  --type=mapping

mounted_flags output
if ((${#MOUNTED_FLAGS[@]})); then
  FLAG_Q="$(quoted_flags "${MOUNTED_FLAGS[@]}")"
else
  FLAG_Q=""
fi
# The loop runs inside the image (dhi-debian has /bin/sh).
run_sh "
  set -- /data/${INDEX}.data.split-*.json /data/${INDEX}.data.split-*.json.gz /data/${INDEX}.data.split-*.json.zst
  found=0
  for f in \"\$@\"; do
    [ -f \"\$f\" ] || continue
    found=1
    echo \"restoring \$f\"
    searchdump --input=\"\$f\" --output=\"${DEST_ES_URL}/${DEST_INDEX}\" --type=data \
      --limit=${LIMIT} --concurrency=${CONCURRENCY} --workers=4 ${FLAG_Q}
  done
  if [ \"\$found\" -eq 0 ]; then
    echo \"error: no split parts for ${INDEX} in /data\" >&2
    exit 1
  fi
"
echo "restored split dump of ${INDEX} -> ${DEST_ES_URL}/${DEST_INDEX}"
