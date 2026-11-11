#!/usr/bin/env bash
# L-701 (TST-SUP-001): run a command inside a pod's network namespace.
#
#   scripts/sup/netns-exec.sh POD CMD [ARGS...]
#
# POD is one of this project's egress-test pods (`podman pod create --network none`,
# scripts/sup/egress.sh): its namespace has loopback only, so nothing can leave the box.
# The command sees /etc/resolv.conf pointing at 127.0.0.1 (the run's logging DNS sink)
# through a private mount namespace; nothing on the host changes. Rootless: `podman
# unshare` enters the user namespace that owns the pod's network namespace.
set -euo pipefail
pod="$1"; shift
inf=$(podman pod inspect "$pod" --format '{{.InfraContainerID}}')
pid=$(podman inspect --format '{{.State.Pid}}' "$inf")
[[ "$pid" =~ ^[1-9][0-9]*$ ]] || { echo "netns-exec: pod $pod is not running" >&2; exit 2; }
dir="${EGRESS_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/searchdump-egress/$pod}"
mkdir -p "$dir"
[[ -f "$dir/resolv.conf" ]] || printf 'nameserver 127.0.0.1\noptions timeout:1 attempts:1\n' > "$dir/resolv.conf"
# shellcheck disable=SC2016
exec podman unshare nsenter --net="/proc/$pid/ns/net" unshare --mount -- \
  sh -c 'mount --bind "$0" /etc/resolv.conf && exec "$@"' "$dir/resolv.conf" "$@"
