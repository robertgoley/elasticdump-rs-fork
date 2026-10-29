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
