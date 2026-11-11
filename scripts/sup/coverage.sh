#!/usr/bin/env bash
# TST-SUP-008 / L-205: coverage across T0 (unit), T1 (contract) and T2 (it), then a never-executed
# function list for the per-release review (docs/UNCOVERED-FUNCTIONS.md).
#   scripts/sup/coverage.sh [it-profile ...]      default: es717-plain opensearch-secure
# Needs cargo-llvm-cov (+ llvm-tools-preview) and rustfilt; T2 needs podman (one engine at a time).
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
profiles=("$@"); [[ ${#profiles[@]} -gt 0 ]] || profiles=(es717-plain opensearch-secure)
OUT="${COV_OUT:-target/coverage}"; mkdir -p "$OUT"
source scripts/res/guards.sh; guard_lock
cargo llvm-cov clean --workspace -q
cargo llvm-cov --no-report nextest -p searchdump-rs --features load --profile unit
cargo llvm-cov --no-report nextest -p searchdump-rs --features load --profile contract
trap 'scripts/it/down.sh' EXIT
for p in "${profiles[@]}"; do
  eval "$(scripts/it/matrix.sh "$p" | sed 's/^/export /')"
  scripts/it/up.sh "$UP_PROFILE"
  ( set -a; source "${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/searchdump-it}/env"; set +a
    # the legacy integration_test suite is plain-only (as in scripts/it/run.sh)
    filt=(); [[ "$p" == *-secure ]] && filt=(-E 'binary(it)')
    cargo llvm-cov --no-report nextest -p searchdump-rs --features it --profile it --test-threads "${IT_THREADS:-2}" "${filt[@]}" )
  scripts/it/down.sh
  unset ES_IMAGE ES_TAG OS_IMAGE OS_TAG
done
cargo llvm-cov report -p searchdump-rs --summary-only | tail -1
cargo llvm-cov report -p searchdump-rs --json --output-path "$OUT/cov.json"
scripts/sup/uncovered.py "$OUT/cov.json" --root "$PWD" > "$OUT/uncovered.md"
echo "report: $OUT/uncovered.md (review it into docs/UNCOVERED-FUNCTIONS.md)"
