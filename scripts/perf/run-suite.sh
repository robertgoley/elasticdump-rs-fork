#!/usr/bin/env bash
# One-machine entrypoint. Only podman + a POSIX/Linux host with bash are needed.
set -euo pipefail
umask 077
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
# Offline plan/help never builds, pulls, starts a service, or creates result directories.
for arg in "$@"; do
  if [[ "$arg" == --dry-run || "$arg" == --help || "$arg" == -h ]]; then
    if command -v python3 >/dev/null; then exec python3 scripts/perf/suite.py "$@"; fi
    echo 'Offline plan needs Python 3.11+; otherwise run --help/--dry-run in a prepared controller image.' >&2
    exit 2
  fi
done
command -v podman >/dev/null || { echo 'podman is required (rootless; no sudo).' >&2; exit 2; }
[[ $(podman info --format '{{.Host.Security.Rootless}}') == true ]] || { echo 'Use rootless podman.' >&2; exit 2; }
image="${PERF_CONTROLLER_IMAGE:-localhost/elasticdump-rs-perf:1}"
if ! podman image exists "$image"; then
  podman build --network=host -f scripts/perf/Containerfile -t "$image" scripts/perf
fi
# The remote API lets the controller reuse the HOST image store and create sibling engines.
# Mount paths identically: engine bind mounts are resolved by the host service, not the client.
runtime="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
[[ -d "$runtime" && -w "$runtime" ]] || { echo 'Set XDG_RUNTIME_DIR to your private writable runtime directory.' >&2; exit 2; }
socket_dir=$(mktemp -d "$runtime/edrs-perf.XXXXXX")
service_pid=''
cleanup() {
  [[ -z "$service_pid" ]] || { kill "$service_pid" 2>/dev/null || true; wait "$service_pid" 2>/dev/null || true; }
  rm -rf "$socket_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
podman system service --time=0 "unix://$socket_dir/podman.sock" >"$socket_dir/service.log" 2>&1 &
service_pid=$!
# Bounded readiness (not a fixed sleep); service starts normally in <1 second.
for ((i=0; i<100; i++)); do
  [[ -S "$socket_dir/podman.sock" ]] && break
  kill -0 "$service_pid" 2>/dev/null || { echo 'podman service failed to start.' >&2; exit 2; }
  sleep 0.1
done
[[ -S "$socket_dir/podman.sock" ]] || { echo 'podman socket readiness timed out.' >&2; exit 2; }
touch /tmp/elasticdump-rs-it.lock
args=(--rm --network=host --security-opt label=disable
  -v "$socket_dir:$socket_dir" -v "$root:$root" -w "$root"
  -v /tmp/elasticdump-rs-it.lock:/tmp/elasticdump-rs-it.lock
  -e "CONTAINER_HOST=unix://$socket_dir/podman.sock"
  -e "PERF_HOST_ID=${PERF_HOST_ID:-$(hostname)}"
  -e "PERF_CONTROLLER_ID=$(podman image inspect --format '{{.Id}}' "$image")")
# No --userns=keep-id with --network=host. Container root maps to this rootless host user.
# Optional local Node installation and prebuilt RELEASE binaries (each mounted read-only).
for var in NODE_ED_BIN PERF_BIN_DIR; do
  if [[ -n "${!var:-}" ]]; then
    dir="${!var}"
    [[ "$dir" == /* && -d "$dir" ]] || { echo "$var must be an absolute directory." >&2; exit 2; }
    # Mount parent for npm's .bin symlinks and its sibling elasticdump package.
    mount_dir="$dir"; [[ "$var" == NODE_ED_BIN ]] && mount_dir="$(dirname "$dir")"
    args+=(-v "$mount_dir:$mount_dir:ro" -e "$var=$dir")
  fi
done
for var in LOAD_PLACE PERF_BUILD_JOBS; do
  [[ -z "${!var:-}" ]] || args+=(-e "$var=${!var}")
done
podman run "${args[@]}" "$image" python3 scripts/perf/suite.py "$@"
