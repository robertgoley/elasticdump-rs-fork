#!/usr/bin/env bash
# TST-INF-003: record golden exchanges against a live engine.
#   scripts/golden/record.sh <profile>           record into tests/golden/<engine>-<version>/
#   scripts/golden/record.sh <profile> --check   record into a temp dir and require a zero diff
#                                                against the committed files (drift check, API-004)
# Profiles as scripts/it/run.sh (one engine at a time; same resource guards).
set -euo pipefail
cd "$(dirname "$0")/../.."
profile="${1:?usage: record.sh <profile> [--check]}"
mode="${2:-}"
if [[ "$mode" == --check ]]; then
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/edrs-golden-XXXXXX")
  trap 'rm -rf "$tmp"' EXIT
  export IT_GOLDEN_DIR="$tmp"
fi
IT_RECORD=1 IT_LEGACY=0 IT_THREADS=1 scripts/it/run.sh "$profile" golden::record --ignored --exact
if [[ "$mode" == --check ]]; then
  rc=0
  for d in "$tmp"/*/; do
    ev=$(basename "$d")
    mkdir -p target/golden-diff
    if ! diff -ru "tests/golden/$ev" "$d" > "target/golden-diff/$ev.diff"; then
      echo "golden drift for $ev: target/golden-diff/$ev.diff" >&2; rc=1
    else
      echo "golden: $ev re-recorded with zero diff"; rm -f "target/golden-diff/$ev.diff"
    fi
  done
  exit $rc
fi
scripts/it/matrix-md.py >/dev/null
echo "golden: recorded; review with git diff tests/golden TESTING-MATRIX.md"
