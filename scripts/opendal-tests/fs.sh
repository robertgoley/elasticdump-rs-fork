#!/usr/bin/env bash
# L-809a: OpenDAL fs:// / file:// tests in temp dirs (no containers).
set -euo pipefail
. "$(dirname "$0")/common.sh"
MULTI=${EDRS_MULTI_BIN:-$(dirname "$BIN")/multielasticdump}
IN=$WORK/in.json
make_input "$IN"
ed() { "$BIN" --quiet "$@"; }
check "local -> fs:// (zstd, split)" ed --input "$IN" --output "fs://$WORK/o/d.json" --compressionType zstd --maxRows 2000
check "parts in place, no temp files left" sh -c "ls $WORK/o | tr '\n' ' ' | grep -qx 'd.split-0.json.zst d.split-1.json.zst d.split-2.json.zst '"
: >"$WORK/back.json"
for n in 0 1 2; do ed --input "file://$WORK/o/d.split-$n.json.zst" --output "$WORK/p$n.json" && cat "$WORK/p$n.json" >>"$WORK/back.json"; done
check "file:// read back keeps every document" same_docs "$IN" "$WORK/back.json"
check_fails "existing file refused without --overwrite" "already exists" ed --input "$IN" --output "fs://$WORK/o/d.split-0.json.zst"
check_fails "a directory is not an output" "name an object or file" ed --input "$IN" --output "fs://$WORK/o/"
check "multielasticdump copy local -> fs://" "$MULTI" --direction copy --input "$WORK/o" --output "fs://$WORK/o2"
check "copied tree identical" diff -r "$WORK/o" "$WORK/o2"
summary "fs"
