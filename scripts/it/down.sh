#!/usr/bin/env bash
# Remove every integration-test container (label searchdump-it=1).
set -euo pipefail
# A caller with RUN_ID removes only its own containers; legacy callers keep the all-IT behaviour.
filters=(--filter label=searchdump-it=1)
[[ -n "${RUN_ID:-}" ]] && filters+=(--filter "name=-${RUN_ID}$")
ids=$(podman ps -aq "${filters[@]}")
if [[ -n "$ids" ]]; then
  podman rm -f -v $ids >/dev/null
fi
if [[ -n "${RUN_ID:-}" ]]; then
  volumes=$(podman volume ls -q --filter label=searchdump-it=1 --filter "label=searchdump-run=${RUN_ID}")
  if [[ -n "$volumes" ]]; then podman volume rm $volumes >/dev/null; fi
fi
# Data dirs of the Elasticsearch profiles are owned by UID 1001 in the userns.
IT_DIR="${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/searchdump-it}"
for d in "$IT_DIR"/es-data-*; do
  if [[ -e "$d" ]]; then
    if [[ $(id -u) -eq 0 ]]; then rm -rf "$d"; else podman unshare rm -rf "$d"; fi
  fi
done
exit 0
