#!/usr/bin/env bash
# Remove every integration-test container (label elasticdump-rs-it=1).
set -uo pipefail
ids=$(podman ps -aq --filter label=elasticdump-rs-it=1)
if [[ -n "$ids" ]]; then
  podman rm -f $ids >/dev/null
fi
# Data dirs of the Elasticsearch profiles are owned by UID 1001 in the userns.
IT_DIR="${IT_DIR:-${TMPDIR:-/tmp}/elasticdump-rs-it}"
for d in "$IT_DIR"/es-data-*; do
  [[ -e "$d" ]] && podman unshare rm -rf "$d"
done
exit 0
