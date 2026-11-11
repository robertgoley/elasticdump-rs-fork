#!/usr/bin/env bash
# L-809a: b2:// against REAL Backblaze B2. Not run in CI or by run-all.sh: it needs the owner's
# B2 account (pending). There is no usable local B2 emulator: OpenDAL's B2 service always
# authorizes at https://api.backblazeb2.com, so a mock cannot be pointed at.
# Required environment (never commit values; blank references only):
#   B2_APPLICATION_KEY_ID=   B2_APPLICATION_KEY=   B2_BUCKET=   B2_BUCKET_ID=
# Optional: B2_PREFIX (default edrs-odt/<timestamp>). The objects written stay under that
# prefix; remove them by hand afterwards (B2 web UI or the b2 CLI).
set -euo pipefail
: "${B2_APPLICATION_KEY_ID:?set B2_APPLICATION_KEY_ID}" "${B2_APPLICATION_KEY:?set B2_APPLICATION_KEY}"
: "${B2_BUCKET:?set B2_BUCKET}" "${B2_BUCKET_ID:?set B2_BUCKET_ID}"
. "$(dirname "$0")/common.sh"
MULTI=${EDRS_MULTI_BIN:-$(dirname "$BIN")/multielasticdump}
DOCS=${ODT_DOCS:-2000}
PREFIX=${B2_PREFIX:-edrs-odt/$(date +%Y%m%dT%H%M%S)}
B2=b2://$B2_BUCKET/$PREFIX
Q="?bucket_id=$B2_BUCKET_ID"
IN=$WORK/in.json
make_input "$IN"
ed() { "$BIN" --quiet "$@"; }
check "local -> b2 (gzip)" ed --input "$IN" --output "$B2/d.json.gz$Q" --fsCompress
check "b2 -> local" ed --input "$B2/d.json.gz$Q" --output "$WORK/back.json"
check "round trip keeps every document" same_docs "$IN" "$WORK/back.json"
check "local -> b2 split parts" ed --input "$IN" --output "$B2/s/d.json$Q" --maxRows 700
check_fails "existing object refused" "already exists" ed --input "$IN" --output "$B2/d.json.gz$Q"
mkdir -p "$WORK/tree/_cluster"; cp "$IN" "$WORK/tree/a.data.json"; echo '{}' >"$WORK/tree/_cluster/policy.json"
check "multielasticdump copy local -> b2" "$MULTI" --direction copy --input "$WORK/tree" --output "$B2/tree$Q"
check "multielasticdump copy b2 -> local" "$MULTI" --direction copy --input "$B2/tree$Q" --output "$WORK/tree.back"
check "tree identical" diff -r "$WORK/tree" "$WORK/tree.back"
msg=$(B2_APPLICATION_KEY=FAKE-odt-b2-wrong-0001 ed --input "$IN" --output "$B2/denied.json$Q" 2>&1 || true)
if [ -n "$msg" ] && ! grep -q "FAKE-odt-b2-wrong-0001" <<<"$msg"; then ok "wrong key fails without printing it"; else bad "wrong key"; fi
echo "objects left under $B2 (remove with the B2 web UI or b2 CLI)"
summary "b2 (real)"
