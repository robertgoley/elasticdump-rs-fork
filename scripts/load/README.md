# Load harness (TST-INF-006, TST-LOAD-001…003, LOAD-012)

## measure.sh

`measure.sh` runs one command and records its metrics.

```
scripts/load/measure.sh [--run-id ID] [--out FILE] [--cluster URL] [--docs N] [--input P]... [--output P]...
                        [--uncompressed-bytes N] [--label k=v]... -- CMD...
scripts/load/measure.py --validate FILE...      # check a file against metrics.schema.json
scripts/load/compare.py BASE.json NEW.json [--threshold 10]   # exit 1 on a regression
```

It records:

- wall time, user and system CPU;
- max RSS from rusage, and peak RSS, FDs and threads sampled from `/proc` every 100 ms;
- voluntary and involuntary context switches;
- bytes read and written;
- docs/s, and MB/s in and out;
- compression ratio;
- `Total Writes` / `Failed Writes`, and retries by kind, parsed from the tool's stderr.

With `--cluster`, it also samples `_nodes/stats` before and after the run. From that it reports:

- indexing and search rates;
- rejected writes and searches;
- open scroll and PIT contexts afterwards.

The output goes to `results/<run-id>.json`, which is git-ignored; schema `edrs-metrics/v1`. A copy of stderr is kept next to it.

**Deviation from the spec:** the spec asks for `/usr/bin/time -v` and a cgroup per command. The box has no `/usr/bin/time`, and per-command cgroups need root, or systemd user scopes that the box does not have. RSS therefore comes from `wait4` rusage, which includes waited-for children, plus `/proc` sampling.

## run.sh

`run.sh` is the load runner. See the header of `scripts/load/run.sh` and `scenarios/*.toml`.

### Running a scenario

```
scripts/load/run.sh L0 dump-restore-logs                  # box, ES 7.17.29 (scenario default)
scripts/load/run.sh L0 copy-logs --engine os13-plain
scripts/load/run.sh L1 dump-restore-logs --dry-run        # print the fit decision only
LOAD_PLACE=runner scripts/load/run.sh L1R dump-restore-logs
```

**Tiers and places.** These are defined in `scenarios/tiers.toml`. The runner refuses, with exit code 2, a tier that is not allowed at the place, or one whose heap or estimated disk exceeds the place budget. On the box, L2 is never allowed.

**Resource guards.** Before starting, the runner takes the shared lock `/tmp/elasticdump-rs-it.lock`. It then checks memory (MemAvailable ≥ 2×heap + 512 MB + tool budget) and disk.

**Seeding.** The runner seeds each index from `edgen` through its own `_bulk` client. Edge-case documents that the engine rejects are recorded per reason in `seeds[].reject_reasons`, together with `seeds[].overwritten`; they are not counted as failures. The reference for verification is always the digest of what the engine stored.

**Correctness (LOAD-002).** After every step, `edverify diff --index-as x` compares the source index with the step's output (a dump file or a target index). Any mismatch fails the run, and the failure names the first 10 differing ids.

**Metrics (LOAD-003).** Each tool step runs under `measure.sh` and is schema-validated. The tool's max RSS must stay under the place's `tool_mem_mb`. `Total Writes` must equal the source count, and `Failed Writes` must be 0.

**Leak check (LOAD-017).** At the end of a run there must be:

- no open scroll or PIT contexts;
- no `*.part*` files in the work directory;
- no unexpected non-system indices.

**Results.** Output is written to `results/load/<run-id>/<scenario>.json` (schema `edrs-load/v1`), alongside the metrics and verify reports. Credentials reach the tools through environment variables only, never through argv.
