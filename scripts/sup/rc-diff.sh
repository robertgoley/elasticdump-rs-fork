#!/usr/bin/env bash
# L-705 (TST-SUP-016): release-candidate behaviour diff.
#
#   scripts/sup/rc-diff.sh [--baseline PROFILE.json] [--release VER]
#                          [--trace-dir DIR --egress-dir DIR] [--write-baseline]
#
# Runs SUP-002 (scripts/sup/trace.sh) and SUP-001 (scripts/sup/egress.sh --tiers
# ${RC_EGRESS_TIERS:-T1,T7,T2}) for the checked-out commit, or reuses the logs of runs that
# already happened (--trace-dir/--egress-dir, e.g. downloaded CI artefacts), condenses them
# into a behaviour profile (scripts/sup/rc-profile.py build) and diffs it against the previous
# release's profile: --baseline, or else the highest-versioned results/profiles/*.json.
# Every added syscall class, path, socket kind, connect target, exec, DNS name, or egress
# phase with new no-route attempts or unexpected names needs a line in
# scripts/sup/rc-signoff.txt (kind<TAB>glob<TAB>reason), or the run fails. Removals pass.
#
# Artefacts in ${RC_DIFF_DIR:-$IT_DIR/rc-diff}/: profile.json, diff.md, diff.json,
# rc-diff-summary.log (and trace/ and egress/ when the runs happen here).
# --write-baseline: after the diff, copy profile.json to results/profiles/<release>.json, the
# baseline for the next candidate; commit it with the release.
# Exit 0 no unsigned change; 1 unsigned change or a failed trace/egress run; 2 usage/input error.
set -euo pipefail
cd "$(dirname "$0")/../.."
BASELINE=""; RELEASE=""; TDIR=""; EDIR=""; WRITE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --baseline) BASELINE="$2"; shift 2 ;;
    --release) RELEASE="$2"; shift 2 ;;
    --trace-dir) TDIR="$2"; shift 2 ;;
    --egress-dir) EDIR="$2"; shift 2 ;;
    --write-baseline) WRITE=1; shift ;;
    *) echo "usage: $0 [--baseline F] [--release V] [--trace-dir D --egress-dir D] [--write-baseline]" >&2; exit 2 ;;
  esac
done
[[ -n "$TDIR" && -z "$EDIR" || -z "$TDIR" && -n "$EDIR" ]] && { echo "rc-diff: give both --trace-dir and --egress-dir, or neither" >&2; exit 2; }
IT_DIR="${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/searchdump-it}"
export IT_DIR
OUT="${RC_DIFF_DIR:-$IT_DIR/rc-diff}"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
SUM="$OUT/rc-diff-summary.log"
: > "$SUM"
note() { echo "$*" | tee -a "$SUM"; }
RELEASE="${RELEASE:-v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
COMMIT="$(git rev-parse --short=12 HEAD)"
if [[ -z "$BASELINE" ]]; then
  BASELINE=$(find results/profiles -maxdepth 1 -name '*.json' 2>/dev/null | sort -V | tail -n1)
fi
RUNS_OK=1
if [[ -z "$TDIR" ]]; then
  TDIR="$OUT/trace"; EDIR="$OUT/egress"
  note "rc-diff: trace run (logs $TDIR)"
  TRACE_LOG_DIR="$TDIR" scripts/sup/trace.sh > "$OUT/trace.log" 2>&1 || RUNS_OK=0
  tail -n1 "$OUT/trace.log" | tee -a "$SUM"
  note "rc-diff: egress run --tiers ${RC_EGRESS_TIERS:-T1,T7,T2} (logs $EDIR)"
  EGRESS_LOG_DIR="$EDIR" scripts/sup/egress.sh --tiers "${RC_EGRESS_TIERS:-T1,T7,T2}" > "$OUT/egress.log" 2>&1 || RUNS_OK=0
  tail -n1 "$OUT/egress.log" | tee -a "$SUM"
fi
python3 scripts/sup/rc-profile.py build --trace "$TDIR" --egress "$EDIR" --release "$RELEASE" \
  --commit "$COMMIT" -o "$OUT/profile.json" | tee -a "$SUM"
DIFF_RC=0
if [[ -z "$BASELINE" ]]; then
  note "rc-diff: no baseline profile (results/profiles/ is empty); nothing to compare"
else
  note "rc-diff: baseline $BASELINE"
  set +e
  python3 scripts/sup/rc-profile.py diff "$BASELINE" "$OUT/profile.json" \
    --signoff scripts/sup/rc-signoff.txt --md "$OUT/diff.md" --json "$OUT/diff.json" | tee -a "$SUM"
  DIFF_RC=${PIPESTATUS[0]}
  set -e
fi
if [[ $WRITE == 1 ]]; then
  cp "$OUT/profile.json" "results/profiles/$RELEASE.json"
  note "rc-diff: wrote results/profiles/$RELEASE.json"
fi
[[ $DIFF_RC == 2 ]] && exit 2
if [[ $RUNS_OK == 1 && $DIFF_RC == 0 ]]; then note "RC_DIFF_DONE OK"; exit 0; fi
note "RC_DIFF_DONE FAIL (runs ok=$RUNS_OK, diff rc=$DIFF_RC; see $OUT)"
exit 1
