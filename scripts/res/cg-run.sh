#!/usr/bin/env bash
# cg-run.sh <mem> <cmd...>
# Run <cmd> under a memory cap.
#
# CG_MODE=cgroup (default when passwordless sudo and cgroup v2 are available):
#   a real cgroup v2 child /sys/fs/cgroup/edrs-test/run-<pid> with memory.max=<mem>,
#   memory.swap.max=0, pids.max=${CG_PIDS:-4096}. sudo is used ONLY to create the
#   cgroup, write its limits, move the command's pid into it, and remove it afterwards
#   (owner-approved 2026-10-29). <cmd> itself runs as the calling user. When the cap is
#   hit, the kernel OOM-kills the command (exit 137). The script prints
#   "cg-run: oom_kill=N" from memory.events, and the cgroup is removed on every exit path.
# CG_MODE=rlimit: an address-space rlimit fallback (ulimit -v; allocations fail instead of
#   an OOM kill: Rust aborts with SIGABRT/134, Python raises MemoryError).
# CG_MODE=cgroup with no sudo or cgroup v2 available: exit 77 (skip).
set -euo pipefail
mem="${1:?usage: cg-run.sh <mem> <cmd...>}"; shift
to_kb() {
  local v="$1" n="${1%[KkMmGg]}"
  case "$v" in
    *[Kk]) echo "$n" ;; *[Mm]) echo $((n * 1024)) ;; *[Gg]) echo $((n * 1024 * 1024)) ;;
    *) echo $((v / 1024)) ;;
  esac
}
have_cgroup() {
  [[ "$(stat -fc %T /sys/fs/cgroup 2>/dev/null)" == cgroup2fs ]] && sudo -n true 2>/dev/null &&
    grep -qw memory /sys/fs/cgroup/cgroup.controllers
}
mode="${CG_MODE:-}"
if [[ -z "$mode" ]]; then have_cgroup && mode=cgroup || mode=rlimit; fi

if [[ "$mode" == rlimit ]]; then
  export MALLOC_ARENA_MAX=2
  ulimit -v "$(to_kb "$mem")"
  ulimit -u "${CG_PIDS:-4096}" 2>/dev/null || true
  exec "$@"
fi

if ! have_cgroup; then
  echo "cg-run: cgroup v2 + sudo not available; skipping" >&2
  exit 77
fi
PARENT=/sys/fs/cgroup/edrs-test
CG="$PARENT/run-$$"
cleanup() {
  [[ -d "$CG" ]] || return 0
  # kill anything left, then remove (a populated cgroup cannot be removed)
  [[ -e "$CG/cgroup.kill" ]] && echo 1 | sudo tee "$CG/cgroup.kill" >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do sudo rmdir "$CG" 2>/dev/null && break; sleep 0.2; done
  # remove the parent when no other run uses it
  sudo rmdir "$PARENT" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM
sudo mkdir -p "$PARENT"
echo "+memory +pids" | sudo tee "$PARENT/cgroup.subtree_control" >/dev/null
sudo mkdir "$CG"
echo $(( $(to_kb "$mem") * 1024 )) | sudo tee "$CG/memory.max" >/dev/null
[[ -e "$CG/memory.swap.max" ]] && echo 0 | sudo tee "$CG/memory.swap.max" >/dev/null
echo "${CG_PIDS:-4096}" | sudo tee "$CG/pids.max" >/dev/null
rc=0
# the child moves itself into the cgroup before exec'ing the command
bash -c 'echo $$ | sudo tee "$1/cgroup.procs" >/dev/null && shift && exec "$@"' _ "$CG" "$@" || rc=$?
oom=$(awk '$1=="oom_kill"{print $2}' "$CG/memory.events" 2>/dev/null || echo 0)
peak=$(cat "$CG/memory.peak" 2>/dev/null || echo "?")
echo "cg-run: mode=cgroup mem=$mem rc=$rc oom_kill=${oom:-0} memory.peak=$peak" >&2
exit "$rc"
