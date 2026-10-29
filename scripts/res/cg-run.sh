#!/usr/bin/env bash
# cg-run.sh <mem> <cmd...>
# Run <cmd> under a memory cap.
#
# The plan's version creates a root cgroup v2 child (memory.max,
# memory.swap.max=0, pids.max). The owner has NOT approved sudo/root cgroups
# on this box, and rootless podman here uses the cgroupfs manager without
# delegation, so `podman run --memory` is silently NOT enforced (verified:
# memory.max reads "max" inside the container). Until the owner approves
# root cgroups this wrapper uses an address-space rlimit instead:
#   ulimit -v <mem>   (RLIMIT_AS; allocation fails instead of an OOM kill)
#   MALLOC_ARENA_MAX=2 so glibc arenas do not eat the budget.
# A Rust binary that hits the cap aborts with "memory allocation of N bytes
# failed" (SIGABRT, exit 134) rather than being OOM-killed (137); tests that
# need the real cgroup behaviour (BRK-RES-001) are DEFERRED.
# Set CG_MODE=cgroup to require a real cgroup (exits 77 when unavailable).
set -euo pipefail
mem="${1:?usage: cg-run.sh <mem> <cmd...>}"; shift
to_kb() {
  local v="$1" n="${1%[KkMmGg]}"
  case "$v" in
    *[Kk]) echo "$n" ;; *[Mm]) echo $((n * 1024)) ;; *[Gg]) echo $((n * 1024 * 1024)) ;;
    *) echo $((v / 1024)) ;;
  esac
}
if [[ "${CG_MODE:-rlimit}" == cgroup ]]; then
  echo "cg-run: root cgroups are not approved on this box; skipping" >&2
  exit 77
fi
kb=$(to_kb "$mem")
export MALLOC_ARENA_MAX=2
ulimit -v "$kb"
ulimit -u "${CG_PIDS:-4096}" 2>/dev/null || true
exec "$@"
