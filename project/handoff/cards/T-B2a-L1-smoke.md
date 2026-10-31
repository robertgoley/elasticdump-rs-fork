---
id: T-B2a
plan_task: B2
lane: campaign
agent: runner
requirements: [TST-LOAD-004, TST-LOAD-005, TST-LOAD-017]
goal: The two existing load scenarios pass at L1 (10M docs) on ES 7.17 and OpenSearch 2, with metrics and digests in results/B2/<cell>/result.json
depends_on: [T-B0]
timeout_min: 40
campaign_timeout_min: 480
heavy_gb: 12
branch: card/T-B2a
summary: results/cards/T-B2a.md
files_allowed:
  - results/B2/*
  - results/load/*
files_readonly:
  - scripts/load/README.md
  - scripts/load/scenarios/dump-restore-logs.toml
  - scripts/load/scenarios/copy-logs.toml
  - scripts/load/metrics.schema.json
  - target/cards/T-B2a/*
forbidden:
  - rerun a failed cell (a mismatch is a finding, not a retry)
  - edit scenarios or scripts
campaign:
  - scripts/load/run.sh L1 dump-restore-logs --place big --dry-run > "$ART/fit.txt" 2>&1
  - scripts/load/run.sh L1 dump-restore-logs --place big --engine es717-plain --run-id B2a-drl-es717 > "$ART/drl-es717.log" 2>&1
  - scripts/load/run.sh L1 copy-logs --place big --engine es717-plain --run-id B2a-copy-es717 > "$ART/copy-es717.log" 2>&1
  - OS_TAG=2 scripts/load/run.sh L1 dump-restore-logs --place big --engine opensearch-plain --run-id B2a-drl-os2 > "$ART/drl-os2.log" 2>&1
  - OS_TAG=2 scripts/load/run.sh L1 copy-logs --place big --engine opensearch-plain --run-id B2a-copy-os2 > "$ART/copy-os2.log" 2>&1
  - podman ps -a --filter name=it- --format '{{.Names}}' > "$ART/leftover-containers.txt"
acceptance:
  - run: test "$(ls results/B2/*/result.json | wc -l)" -ge 4
    exit: 0
    expect: four cells reported
  - run: for f in results/B2/*/result.json; do jq -e '.schema=="edrs-result/1" and (.status|type=="string")' "$f" >/dev/null || exit 1; done
    exit: 0
    expect: schema and status present in every cell
---
## Context
B2 runs the L1 matrix (10M docs) on a big host. This card is the first slice: the two scenarios that exist today, on
two engines. `scripts/load/run.sh <tier> <scenario> --place big` provisions one engine (via scripts/it/up.sh), seeds with
edgen, runs dump/restore/copy, verifies with edverify, does the LOAD-017 leak check (scroll/PIT contexts = 0) and writes
`results/load/<run-id>/<scenario>.json` (git-ignored metrics in `edrs-metrics/v1`). It refuses tiers that do not fit.
Pass per cell (B2 acceptance): count and digest equal (source, dump, destination); Failed Writes = 0; leak check clean;
peak RSS within the tier's tool_mem_mb.

## Spec
1. One `results/B2/<run-id>/result.json` per cell in the HANDOFF §9 schema (`task: "B2"`, `cell: <run-id>`, metrics,
   correctness with expected/actual count and digest, findings). Copy numbers only from the run's JSON and log.
2. A cell whose log shows a verify mismatch, leak, non-zero exit or "refused" is `fail`/`blocked` with the log line.
3. `results/B2/SUMMARY.md`: cell | status | docs/s dump | docs/s restore | peak RSS MB | digest equal.
4. `leftover-containers.txt` non-empty → finding (teardown leak).

## Output contract
One commit `T-B2a: TST-LOAD-004 TST-LOAD-005 TST-LOAD-017: L1 smoke cells` with results/B2/** (and results/load/** if
the runner wrote tracked files there) and results/cards/T-B2a.md.
