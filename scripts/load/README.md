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

## matrix.sh: the LOAD-004 scenario matrix

`matrix.sh` runs every scenario listed in `scenarios/matrix.toml` at one tier and place. It gates each cell, which is one step of one scenario.

```
LOAD_WORK=/workspace/logs/l801/work scripts/load/matrix.sh L0 --place box
scripts/load/matrix.sh L1L --place box                    # full L1-lite (1M docs, 2 GB heap)
scripts/load/matrix.sh L0 --place shared                  # scaled down while other agents use the box
scripts/load/matrix.sh L0 --place box --write-baseline    # record the perf baseline
scripts/load/matrix.sh L0 --only matrix-split --docs 5000 # smoke (never compared or recorded)
```

| Scenario | Cells |
|---|---|
| `matrix-dump` | `scroll`, `pit`, `pit-s2`, `pit-s4`, `pit-s8`, `pit-s16` |
| `matrix-restore` | `c{1,4,8}-l{1000,5000}` (`--concurrency` x `--limit`), from one verified dump (`src`) |
| `matrix-copy` | `copy` (scroll), `copy-pit-s4` (cluster -> cluster) |
| `matrix-compress` | `fscompress` (gzip), `zstd-1`, `zstd-3`, `zstd-9` (`--compressionLevel`) |
| `matrix-split` | `filesize` (`--fileSize=20mb`), `maxrows` (`--maxRows=25000`), `filesize-zstd` |
| `matrix-filter` | `full`, `sourceonly` (`--sourceOnly`), `range` (`--searchBody` range on `http.response.status_code`) |

**Correctness gate.** Every cell is checked with `edverify diff`, and the tool must exit 0. Every cell must also pass the scenario's write-count, RSS-budget and leak checks. What a cell is compared against depends on its type:

| Cell type | Compared against |
|---|---|
| Dump | The cluster digest. |
| Split dump | The cluster digest, over all parts together. The cell also needs at least `min_parts` parts. |
| `verify = "source"` | The raw `_source` of every line of the scenario's last full dump. |
| `verify = "query"` | An index built by the engine's own `_reindex` with the same query. `Total Writes` must equal that index's count, and the index is deleted afterwards. |
| Restore and copy | The source index. The target index is deleted after its check. |

**Perf baseline.** Each run's docs/s and tool max RSS are compared with `baselines/<place>-<tier>-<engine>.json`. A cell fails if it is more than `threshold_pct` (30) worse; the threshold is wide because the shared box is noisy. Baselines are only comparable within the same place, tier and engine. A cell that has no baseline entry is reported as `no-baseline`.

The run writes `results/load/<run-id>/matrix.json` (git-ignored).

### Sizes and where they run

| Tier | Docs | Heap | Where it runs |
|---|---|---|---|
| L0 | 100k | 1 GB (768 MB on `shared`) | Everywhere. Full size on this box: place `box`. |
| L1L | 1M | 2 GB | Full L1-lite. On this box with place `box`; needs about 6 GB of free RAM. Also runs on `big`. |
| L1S | 250k | 768 MB | L1-lite scaled down, for place `shared` only (when other agents need the box). |
| L1 | 10M | 4 GB | Phase B, on a dedicated load host (`big`). The memory guard needs about 10 GB free, which this box does not have. |

Committed baselines:

- `baselines/box-L0-es717-plain.json` and `baselines/box-L1L-es717-plain.json` are full size.
- `baselines/shared-L0-es717-plain.json` and `baselines/shared-L1S-es717-plain.json` are scaled down.
- `baselines/shared-L1L-es717-plain.json` has the full 1M docs, with the heap capped at 768 MB.

All 25 cells were correct in every committed baseline run. One outlier showed up: a single shared L1L run took 157 s for `zstd-9`, where the box run and the shared rerun took 10-11 s (an offline file-to-file run takes 10.5 s). It did not reproduce, so the committed value is from the rerun. If a regression shows up on one cell, rerun that scenario with `--only` before treating it as real.

A tier that a place cannot hold is refused with exit 2, and the message says what the tier needs.

### Place `shared`: the scaled-down fallback while other agents use the box

The `shared` place keeps the run to about half the box:

- **CPU.** `cpus = "0-3"` pins `run.py` to 4 of the 8 CPUs, and everything it starts inherits that affinity: the tools, `edverify`, and the engine container (through podman). `IT_CONTAINER_CPUS=2` is passed as well.
- **Memory.** The heap is capped at 768 MB and the JVM runs lean.
- **Disk.** `data_on_work = true` mounts the engine's data dir under `LOAD_WORK` (`IT_ES_DATA_MOUNT=1` in `up.sh`), so all load data lives in the run's work dir and is deleted with it; place `box` does the same. This applies to the official Elasticsearch images, which is the matrix default (`es717-plain`); other engines keep their data in podman's container storage.

Rootless podman with the cgroupfs manager accepts `--memory` and `--cpus` on this box but does not enforce them; inside the container, `memory.max` reads `max` and `nproc` reads 8. Two mechanisms therefore enforce the limits instead:

- **CPU:** the affinity above.
- **Memory:** a watchdog in `run.py` samples the engine's process-tree RSS from the host's `/proc` every 2 s. If it exceeds `node_rss_mb` (1800), the watchdog kills the container and fails the run.

The peak is recorded as `node.peak_rss_mb`.

Split file names follow Node elasticdump: `<base>.split-<n><ext>[.gz|.zst]`. An output that already ends in `.zst` therefore gets the codec suffix twice, so the matrix names split outputs `*.ndjson`.

## Memory checks (LOAD-005, `memcheck.py`)

`memcheck.py` checks that peak RSS is bounded and flat and that the README memory formula holds. It has three modes:

- `memcheck.py matrix SMALL LARGE`: compares two `matrix.sh` runs with the same flags at two sizes, for example `results/load/<L0 run>` and `results/load/<L1L run>`. Each cell passes when its peak RSS at LARGE is at most `--flat-pct` (10) above SMALL and every peak is at most `--formula-pct` (25) above the README estimate. `--formula old` checks the pre-L-802 formula (x 1.5 + 10 MB).
- `memcheck.py files`: runs the file-to-file cases in `scenarios/memcheck.toml` (doc sizes about 300 B, 1 KB, 40 KB and 1 MB) at a small and a large document count. It needs no cluster; data goes under `LOAD_WORK` and is deleted afterwards.
- `memcheck.py plateau -- CMD...` (or `--from SAMPLES`): one long run, sampled every 0.1 s. It passes when the peak after the first half of the run is at most `--flat-pct` above the peak before it, which separates bounded growth from a leak on 10M-doc runs.

Results go to `results/load/<run-id>/memcheck-*.json`; the L-802 summary is `results/local/L-802.json`. Set `MALLOC_ARENA_MAX=2` to compare with glibc limited to two malloc arenas; the files mode records the setting. On this box a 10M-doc run is limited to single cells. The full L1 tier at 10M docs needs a dedicated load host.
