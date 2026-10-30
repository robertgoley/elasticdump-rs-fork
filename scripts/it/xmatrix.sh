#!/usr/bin/env bash
# Cross-version matrix "export once, import everywhere" (TST-API-070/071), one
# container at a time:
#   pass 1: every image exports the fixture set to $IT_XMATRIX_DIR/<label>/ (zstd)
#   pass 2: every image imports every other image's export and compares the
#           cells with tests/golden/cross-expectations.toml
#
#   scripts/it/xmatrix.sh [export|import|all]   (default all)
#
# IT_XMATRIX_RECORD=1 reports table differences without failing; the actual
# cells are always written to $IT_XMATRIX_DIR/actual-<label>.toml.
# XMATRIX_PROFILES overrides the image list ("profile:OS_TAG ...").
set -uo pipefail
cd "$(dirname "$0")/../.."
what="${1:-all}"
export IT_XMATRIX_DIR="${IT_XMATRIX_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/elasticdump-rs-xmatrix}"
mkdir -p "$IT_XMATRIX_DIR"
profiles="${XMATRIX_PROFILES:-opensearch-plain:2 opensearch-plain:3 os13-plain: es710-plain: es710-secure: es717-plain: elasticsearch8-plain: es819-plain: elasticsearch-plain:}"
rc=0
pass() {
  local filter="$1" p prof tag
  for p in $profiles; do
    prof=${p%%:*}; tag=${p#*:}
    if [[ -n $tag ]]; then export OS_TAG=$tag; else unset OS_TAG; fi
    if IT_LEGACY=0 CARGO_JOBS=${CARGO_JOBS:-2} IT_THREADS=${IT_THREADS:-2} scripts/it/run.sh "$prof" "$filter" --nocapture; then
      echo "XMATRIX $filter $prof$tag ok"
    else
      echo "XMATRIX $filter $prof$tag FAILED"; rc=1
    fi
  done
}
case "$what" in
  export) pass xmatrix::xmatrix_export ;;
  import) pass xmatrix::xmatrix_import ;;
  all) pass xmatrix::xmatrix_export; pass xmatrix::xmatrix_import ;;
  *) echo "usage: $0 [export|import|all]" >&2; exit 2 ;;
esac
exit $rc
