---
id: T-B7
plan_task: B7
lane: campaign
agent: runner
requirements: [TST-API-084, TST-INJ-015, TST-SUP-020, TST-SUP-022]
goal: Every cargo-fuzz target runs its long campaign (1 h; 4 h for sonic_vs_serde and zstd_in) and each gets results/B7/<target>.json; crashes are minimised
depends_on: [T-B0]
timeout_min: 60
campaign_timeout_min: 900
heavy_gb: 0
branch: card/T-B7
summary: results/cards/T-B7.md
files_allowed:
  - results/B7/*
files_readonly:
  - scripts/fuzz/smoke.sh
  - fuzz/Cargo.toml
  - target/cards/T-B7/*
forbidden:
  - edit fuzz targets, src/ or tests/ (a crash fix is lead-only)
  - commit corpora or logs (they stay in target/cards/T-B7/ and fuzz/corpus/)
campaign:
  - mkdir -p "$ART/logs" && cargo +nightly fuzz list > "$ART/targets.txt"
  - test -d fuzz/corpus || scripts/fuzz/seed.py
  - CARGO_BUILD_JOBS=8 cargo +nightly fuzz build -O --target x86_64-unknown-linux-gnu > "$ART/build.log" 2>&1
  - xargs -P "${FUZZ_JOBS:-4}" -I{} bash -c 'secs=3600; case {} in sonic_vs_serde|zstd_in) secs=14400;; esac; cargo +nightly fuzz run -O --target x86_64-unknown-linux-gnu {} fuzz/corpus/{} -- -max_total_time=$secs -rss_limit_mb=4096 -timeout=10 -max_len=65536 -print_final_stats=1 > "$ART/logs/{}.log" 2>&1; echo "{} rc=$?" >> "$ART/rc.txt"' < "$ART/targets.txt"
  - for t in $(cat "$ART/targets.txt"); do printf '%s corpus_files=%s\n' "$t" "$(ls fuzz/corpus/$t 2>/dev/null | wc -l)"; done > "$ART/corpus.txt"
  - ls -R fuzz/artifacts > "$ART/artifacts.txt" 2>&1 || true
acceptance:
  - run: test "$(ls results/B7/*.json | wc -l)" -eq "$(wc -l < target/cards/T-B7/targets.txt)"
    exit: 0
    expect: one JSON per target
  - run: for f in results/B7/*.json; do python3 -m json.tool "$f" >/dev/null || exit 1; done
    exit: 0
    expect: all valid JSON
---
## Context
HANDOFF-TASKPLAN B7: zero crashes, OOMs, timeouts or oracle violations is the pass condition. Each finding gets a
minimised reproducer (`cargo +nightly fuzz tmin <target> <crash-file>`) and is reported; the lead writes the failing unit
test and the fix. The repo has 16 targets (`cargo +nightly fuzz list`; the plan expected 19 — report the gap).
Needs nightly and a C++ compiler for libfuzzer-sys. FUZZ_JOBS = cores/4 is a safe start.

## Spec
1. For each target write `results/B7/<target>.json`: `{target, seconds, rc, execs, cov, corpus_size, crashes:[{file,
   kind, first_line}], status}` from `logs/<target>.log` (`stat::number_of_executed_units`, last `cov:`) and rc.txt.
2. rc ≠ 0 or any file under fuzz/artifacts/<target>/ → `status: "fail"`; run `cargo +nightly fuzz tmin` on each crash
   file once and record the minimised path. Quote the panic/ERROR line verbatim.
3. `results/B7/SUMMARY.md`: table target | seconds | execs | cov | status, then findings by severity.

## Output contract
One commit `T-B7: TST-API-084 TST-INJ-015 TST-SUP-020 TST-SUP-022: long fuzz campaign` with results/B7/* and
results/cards/T-B7.md. Corpora: the lead tars `fuzz/corpus/` separately (not in git).
