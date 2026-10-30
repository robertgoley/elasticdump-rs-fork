#!/usr/bin/env bash
# cargo-fuzz smoke runs (TST-API-084 / L-505): every target for FUZZ_SECS seconds (default 60;
# the nightly B campaign uses 3600) on a corpus seeded by scripts/fuzz/seed.py. One target at
# a time, -rss_limit_mb 1024, so it fits beside the other agents on this box.
#
#   scripts/fuzz/smoke.sh [target...]          (default: all targets)
#
# Crashes land in fuzz/artifacts/<target>/; minimise with `cargo +nightly fuzz tmin` and
# commit the input as a regression unit test. Exit 0 = no crash.
#
# Needs nightly + a C++ compiler for libfuzzer-sys (CXX=...; this box has no g++, so the L-505
# runs used an unpacked g++-14 via CXX/LIBRARY_PATH). Corpus growth is capped at -max_len=65536.
set -uo pipefail
cd "$(dirname "$0")/../.."
secs="${FUZZ_SECS:-60}"
[[ -d fuzz/corpus ]] || scripts/fuzz/seed.py >/dev/null
targets=("$@")
[[ ${#targets[@]} -gt 0 ]] || mapfile -t targets < <(cargo +nightly fuzz list)
fail=0
for t in "${targets[@]}"; do
  log=$(mktemp)
  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo +nightly fuzz run -O --target "${FUZZ_TARGET_TRIPLE:-x86_64-unknown-linux-gnu}" "$t" "fuzz/corpus/$t" -- \
    -max_total_time="$secs" -rss_limit_mb=1024 -max_len=65536 -print_final_stats=1 > "$log" 2>&1
  rc=$?
  execs=$(grep -m1 -o 'stat::number_of_executed_units: *[0-9]*' "$log" | grep -o '[0-9]*$')
  cov=$(grep -o 'cov: [0-9]*' "$log" | tail -1)
  if [[ $rc == 0 ]]; then
    echo "$t: ok (${execs:-?} execs, ${cov:-cov ?}, ${secs}s)"
  else
    echo "$t: CRASH rc=$rc"; grep -E 'panicked|ERROR|SUMMARY|Test unit written' "$log" | head -5
    fail=1
  fi
  rm -f "$log"
done
exit $fail
