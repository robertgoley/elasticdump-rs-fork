#!/usr/bin/env bash
# L-000: print the resources this box has right now and write
# results/local/env.json. Read-only: no containers are started or removed.
set -euo pipefail
cd "$(dirname "$0")/.."
out=results/local/env.json
mkdir -p "$(dirname "$out")"
nproc_v=$(nproc)
mem_total=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
mem_avail=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
swap_total=$(awk '/SwapTotal/ {print int($2/1024)}' /proc/meminfo)
disk_free=$(df -Pm . | awk 'NR==2 {print $4}')
running=$(podman ps --format '{{.Names}}' 2>/dev/null | paste -sd, - || true)
it_busy=$(podman ps --filter name=it- --format '{{.Names}}' 2>/dev/null | grep -c . || true)
images=$(podman images -q 2>/dev/null | sort -u | grep -c . || true)
rustc_v=$(rustc --version 2>/dev/null || echo none)
printf 'nproc=%s mem_total_mb=%s mem_available_mb=%s swap_mb=%s disk_free_mb=%s\n' \
  "$nproc_v" "$mem_total" "$mem_avail" "$swap_total" "$disk_free"
printf 'containers=[%s] it_containers=%s images=%s rustc="%s"\n' "$running" "$it_busy" "$images" "$rustc_v"
python3 - "$out" <<PY
import json, sys, datetime
json.dump({
  "task": "L-000",
  "time": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
  "nproc": $nproc_v, "mem_total_mb": $mem_total, "mem_available_mb": $mem_avail,
  "swap_mb": $swap_total, "disk_free_mb": $disk_free,
  "containers": [c for c in "$running".split(",") if c],
  "it_containers_running": $it_busy, "images": $images, "rustc": "$rustc_v",
}, open(sys.argv[1], "w"), indent=2)
PY
echo "wrote $out"
