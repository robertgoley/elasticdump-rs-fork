#!/usr/bin/env bash
# TST-BRK-IN-007 under a real memory cap: a file input (a FIFO fed by a generator) whose
# second document is a single 2 GB value. Under a 512 MB cgroup (cg-run.sh) the run must
# stop with the --maxDocMb error (exit 1) naming document 2, not be OOM-killed (137).
#
#   scripts/res/giant-value.sh [path/to/elasticdump-rs]   (default target/debug/elasticdump-rs)
#
# GIANT_EXTRA="--maxDocMb 0" shows the uncapped behaviour (OOM kill, 137).
# Exit 0 = pass, 1 = fail, 77 = skipped (no cgroup v2 / sudo).
set -uo pipefail
cd "$(dirname "$0")/../.."
bin="${1:-target/debug/elasticdump-rs}"
[[ -x "$bin" ]] || { echo "missing $bin (cargo build first)" >&2; exit 1; }
work=$(mktemp -d)
gen=""
trap '[[ -n "$gen" ]] && kill "$gen" 2>/dev/null; rm -rf "$work"' EXIT
mkfifo "$work/in.json"
python3 - "$work/in.json" <<'PY' &
import sys
try:
    with open(sys.argv[1], "wb") as f:
        f.write(b'{"_id":"g1","_source":{"a":1}}\n{"_id":"big","_source":{"blob":"')
        chunk = b"x" * (1 << 20)
        for _ in range(2048):
            f.write(chunk)
        f.write(b'"}}\n{"_id":"g2","_source":{"a":2}}\n')
except BrokenPipeError:
    pass
PY
gen=$!
scripts/res/cg-run.sh 512M "$bin" --input "$work/in.json" --output "$work/out.json" \
  ${GIANT_EXTRA:-} > "$work/stdout" 2> "$work/stderr"
rc=$?
tail -5 "$work/stderr" | cut -c1-400
echo "giant-value: exit $rc"
[[ $rc == 77 ]] && exit 77
if [[ $rc == 1 ]] && grep -q -- "--maxDocMb" "$work/stderr" && grep -q "document 2 of the input" "$work/stderr" \
  && [[ ! -e "$work/out.json" ]]; then
  echo "giant-value: PASS (clear error under 512 MB, nothing committed)"; exit 0
fi
echo "giant-value: FAIL"; exit 1
