#!/usr/bin/env bash
# Remove every integration-test container (label elasticdump-rs-it=1).
set -uo pipefail
ids=$(podman ps -aq --filter label=elasticdump-rs-it=1)
if [[ -n "$ids" ]]; then
  podman rm -f $ids >/dev/null
fi
