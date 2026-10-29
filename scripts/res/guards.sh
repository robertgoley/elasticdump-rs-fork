# shellcheck shell=bash
# Resource guards for the shared box (TST-INF-007, TST-LOAD-012). Source it.
#   guard_lock                 serialise IT/load runs (flock; re-entrant via IT_LOCK_HELD)
#   guard_mem NEED_MB WHAT     0 if MemAvailable >= NEED_MB, else print SKIP and return 75
#   guard_disk PATH NEED_MB WHAT
#   mb_of 512m|2g|1500         size string -> MiB
#   skip_or_fail REQUIRED_VAR  after a failed guard: return 0 (SKIP) or 1 when $REQUIRED_VAR=1

mem_available_mb() { awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo; }

mb_of() {
  local v="${1,,}"
  case "$v" in
    *g) echo $(( ${v%g} * 1024 ));;
    *m) echo "${v%m}";;
    *) echo "$v";;
  esac
}

guard_lock() {
  [[ "${IT_LOCK_HELD:-0}" == 1 ]] && return 0
  local lock="${IT_LOCK:-/tmp/elasticdump-rs-it.lock}"
  exec 8>>"$lock"
  if ! flock -n 8; then
    echo "guard: another IT/load run holds $lock; waiting (max ${IT_LOCK_WAIT:-3600}s)" >&2
    flock -w "${IT_LOCK_WAIT:-3600}" 8 || { echo "guard: lock wait timed out" >&2; return 75; }
  fi
  export IT_LOCK_HELD=1
}

guard_mem() {
  local need="$1" what="$2" have
  have=$(mem_available_mb)
  if (( have < need )); then
    echo "SKIP $what: needs ${need} MB available, have ${have} MB" >&2
    return 75
  fi
}

guard_disk() {
  local path="$1" need="$2" what="$3" have
  mkdir -p "$path"
  have=$(df -Pm "$path" | awk 'NR==2 {print $4}')
  if (( have < need )); then
    echo "SKIP $what: needs ${need} MB free disk at $path, have ${have} MB" >&2
    return 75
  fi
}

skip_or_fail() {
  local var="$1"
  if [[ "${!var:-0}" == 1 ]]; then
    echo "guard: $var=1, so the skip is a failure" >&2
    return 1
  fi
  return 0
}
