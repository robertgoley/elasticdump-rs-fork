#!/usr/bin/env bash
# Remove every integration-test container (label elasticdump-rs-it=1).
set -euo pipefail
# A caller with RUN_ID removes only its own containers; legacy callers keep the all-IT behaviour.
filters=(--filter label=elasticdump-rs-it=1)
[[ -n "${RUN_ID:-}" ]] && filters+=(--filter "name=-${RUN_ID}$")
ids=$(podman ps -aq "${filters[@]}")
if [[ -n "$ids" ]]; then
  podman rm -f -v $ids >/dev/null
fi
if [[ -n "${RUN_ID:-}" ]]; then
  volumes=$(podman volume ls -q --filter label=elasticdump-rs-it=1 --filter "label=elasticdump-rs-run=${RUN_ID}")
  if [[ -n "$volumes" ]]; then podman volume rm $volumes >/dev/null; fi
fi
# Data dirs of the Elasticsearch profiles are owned by UID 1001 in the userns.
IT_DIR="${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/elasticdump-rs-it}"
for d in "$IT_DIR"/es-data-*; do
  [[ -e "$d" ]] && podman unshare rm -rf "$d"
done
exit 0
