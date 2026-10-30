#!/usr/bin/env bash
# TST-BRK-RES-002: disk full while writing a split set (and a single file) on a small tmpfs.
# The run must fail (exit 1) naming the error, leave the existing destination untouched and
# leave no staging (".part-") or moved-aside (".edrs-replaced-") files behind.
#
#   scripts/res/disk-full.sh [path/to/elasticdump-rs]   (default target/debug/elasticdump-rs)
#
# sudo is used ONLY to mount and unmount a 2 MB tmpfs on a private temporary directory
# (owner-approved scope: small tmpfs/loop mounts for disk-full tests); it is unmounted on
# every exit path. Exit 0 = pass, 1 = fail, 77 = skipped (no passwordless sudo).
set -uo pipefail
cd "$(dirname "$0")/../.."
bin="${1:-target/debug/elasticdump-rs}"
[[ -x "$bin" ]] || { echo "missing $bin (cargo build first)" >&2; exit 1; }
sudo -n true 2>/dev/null || { echo "disk-full: no passwordless sudo; skipping" >&2; exit 77; }
work=$(mktemp -d)
mnt="$work/mnt"
mkdir "$mnt"
cleanup() {
  mountpoint -q "$mnt" && sudo -n umount "$mnt"
  rm -rf "$work"
}
trap cleanup EXIT
sudo -n mount -t tmpfs -o size=2m,mode=0700,uid="$(id -u)",gid="$(id -g)" edrs-diskfull "$mnt" || exit 1
python3 - "$work/in.json" <<'PY'
import json, sys
with open(sys.argv[1], "w") as f:
    for i in range(20000):
        f.write(json.dumps({"_index": "i", "_id": str(i), "_source": {"n": i, "pad": "p" * 200}}) + "\n")
PY
fail=0
# 1) split set with --overwrite over an older set
for n in 0 1; do echo "old$n" > "$mnt/out.split-$n.json"; done
"$bin" --input "$work/in.json" --output "$mnt/out.json" --fileSize 256kb --overwrite \
  > "$work/o1" 2> "$work/e1"; rc=$?
echo "split: exit $rc: $(grep -m1 -io 'no space left[^"]*' "$work/e1" | head -c 120)"
[[ $rc == 1 ]] && grep -qi "no space left" "$work/e1" || { echo "  FAIL: expected exit 1 with ENOSPC"; tail -3 "$work/e1"; fail=1; }
left=$(ls -A "$mnt" | tr '\n' ' ')
[[ "$left" == "out.split-0.json out.split-1.json " ]] || { echo "  FAIL: left [$left]"; fail=1; }
for n in 0 1; do [[ "$(cat "$mnt/out.split-$n.json")" == "old$n" ]] || { echo "  FAIL: part $n changed"; fail=1; }; done
# 2) single file over an existing one
rm -f "$mnt"/*; echo old > "$mnt/one.json"
"$bin" --input "$work/in.json" --output "$mnt/one.json" --overwrite > "$work/o2" 2> "$work/e2"; rc=$?
echo "single: exit $rc"
[[ $rc == 1 ]] && grep -qi "no space left" "$work/e2" || { echo "  FAIL: expected exit 1 with ENOSPC"; tail -3 "$work/e2"; fail=1; }
left=$(ls -A "$mnt" | tr '\n' ' ')
[[ "$left" == "one.json " && "$(cat "$mnt/one.json")" == old ]] || { echo "  FAIL: left [$left]"; fail=1; }
# 3) control: the same split dump fits once the input is small enough
head -n 1000 "$work/in.json" > "$work/small.json"; rm -f "$mnt"/*
"$bin" --input "$work/small.json" --output "$mnt/out.json" --fileSize 64kb > "$work/o3" 2> "$work/e3"; rc=$?
echo "control: exit $rc, $(ls "$mnt" | wc -l) parts, $(cat "$mnt"/out.split-*.json | wc -l) lines"
[[ $rc == 0 && "$(cat "$mnt"/out.split-*.json | wc -l)" == 1000 ]] || { echo "  FAIL: control"; tail -3 "$work/e3"; fail=1; }
[[ $fail == 0 ]] && { echo "disk-full: PASS"; exit 0; }
echo "disk-full: FAIL"; exit 1
