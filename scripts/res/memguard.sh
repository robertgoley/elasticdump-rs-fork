#!/usr/bin/env bash
# memguard.sh <GB> [max_wait_s]
# Wait until MemAvailable >= <GB> GiB (polling every 15 s, default up to
# 30 min). Exit 0 when there is enough memory, 75 (EX_TEMPFAIL) otherwise so
# the caller can retry the task later.
set -euo pipefail
need_gb="${1:?usage: memguard.sh <GB> [max_wait_s]}"
max_wait="${2:-1800}"
need_mb=$(awk -v g="$need_gb" 'BEGIN {print int(g*1024)}')
waited=0
while :; do
  have=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
  if (( have >= need_mb )); then
    echo "memguard: ${have} MB available >= ${need_mb} MB" >&2
    exit 0
  fi
  if (( waited >= max_wait )); then
    echo "memguard: only ${have} MB available after ${waited}s (need ${need_mb} MB); retry later" >&2
    exit 75
  fi
  sleep 15; waited=$((waited + 15))
done
