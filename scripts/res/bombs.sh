#!/usr/bin/env bash
# TST-SUP-020 decompression bombs under a real memory cap (cg-run.sh, 256 MB cgroup):
#   zst-value   2 GiB single value in RLE blocks (~70 KB on disk, ~30000:1) -> exit 1, --maxDocMb error
#   zst-space   2 GiB of whitespace in RLE blocks, then one document           -> exit 0, 1 document
#   gz-space    2 GiB of whitespace, gzip -9 (~2 MB, ~1000:1), then one doc    -> exit 0, 1 document
# Every case must finish without an OOM kill (exit 137). Reduced scale (A); GB-scale campaigns are B.
#
#   scripts/res/bombs.sh [path/to/elasticdump-rs]   (default target/release/elasticdump-rs)
# Exit 0 = pass, 1 = fail, 77 = skipped (no cgroup v2 / sudo).
set -uo pipefail
cd "$(dirname "$0")/../.."
bin="${1:-target/release/elasticdump-rs}"
[[ -x "$bin" ]] || { echo "missing $bin (cargo build --release first)" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 - "$work" <<'PY'
import gzip, struct, sys, zlib
w = sys.argv[1]
def frame(blocks):
    f = bytearray(b"\x28\xb5\x2f\xfd\x00\x38")          # window 2^17
    for i, (ty, size, body) in enumerate(blocks):
        f += struct.pack("<I", int(i + 1 == len(blocks)) | (ty << 1) | (size << 3))[:3] + body
    return bytes(f)
B = 128 << 10
n = (2 << 30) // B
doc = b'{"_id":"d","_source":{"a":1}}\n'
with open(f"{w}/zst-value.zst", "wb") as f:
    head = doc + b'{"_id":"x","_source":{"v":"'
    f.write(frame([(0, len(head), head)]))
    f.write(frame([(1, B, b"x")] * n + [(0, 4, b'"}}\n')]))
with open(f"{w}/zst-space.zst", "wb") as f:
    f.write(frame([(1, B, b" ")] * n + [(0, len(doc), doc)]))
c = zlib.compressobj(9, zlib.DEFLATED, 31)
with open(f"{w}/gz-space.json.gz", "wb") as f:
    chunk = b" " * (1 << 20)
    for _ in range(2048):
        f.write(c.compress(chunk))
    f.write(c.compress(doc)); f.write(c.flush())
PY
ls -l "$work" | awk 'NR>1{print "  " $5, $9}'
fail=0
for c in zst-value.zst zst-space.zst gz-space.json.gz; do
  scripts/res/cg-run.sh 256M "$bin" --input "$work/$c" --output "$work/$c.out.json" --maxDocMb 64 \
    > "$work/stdout" 2> "$work/stderr"
  rc=$?
  [[ $rc == 77 ]] && exit 77
  oom=$(grep -o 'oom_kill=[0-9]*' "$work/stderr" | tail -1)
  case "$c" in
    zst-value.zst) ok=$([[ $rc == 1 ]] && grep -q -- "--maxDocMb" "$work/stderr" && echo y) ;;
    *) ok=$([[ $rc == 0 ]] && [[ $(wc -l < "$work/$c.out.json") == 1 ]] && echo y) ;;
  esac
  echo "$c: exit $rc ${oom:-} $(grep -o 'memory.peak=[^ ]*' "$work/stderr" | tail -1) -> $([[ -n "$ok" ]] && echo PASS || echo FAIL)"
  [[ -z "$ok" ]] && { tail -3 "$work/stderr" | cut -c1-300; fail=1; }
  rm -f "$work/$c.out.json"
done
exit $fail
