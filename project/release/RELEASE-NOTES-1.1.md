# searchdump-rs 1.1 release notes (draft)

Status: draft on the local `release/1.1` branch. Not tagged, not published, and not yet
performance-measured. These notes describe the changes already committed for 1.1. Fill in the
[Performance](#performance) section after the point-in-time runs finish.

The same material is summarised in [`CHANGELOG.md`](../../CHANGELOG.md) under **Unreleased (1.1)**.
Known, deliberate differences from Node elasticdump are also in
[`NODE-DIFFERENCES.md`](../../NODE-DIFFERENCES.md) (#25–#27).

## Disclaimer

searchdump-rs is provided **as is, without warranty of any kind**, as set out in its
[MIT license](../../LICENSE) (the same warranty disclaimer as the elasticdump-rs code it derives
from; Node elasticdump, whose behaviour it follows, has the equivalent disclaimer in Apache-2.0,
sections 7 and 8). The authors accept no liability for any damage from its use, including data
loss, data corruption, downtime or service disruption, in production or anywhere else.

It has been stable and fast in our testing, but no test suite can cover every edge case across
cluster versions, configurations, plugins and storage backends. Before you rely on it:

- try it in a non-production environment first;
- verify every backup independently: restore it somewhere, then compare document counts (and
  spot-check documents, mappings and settings) against the source;
- keep your existing backup method until those checks pass for your own data.

See [Disclaimer](../../README.md#disclaimer).

## Highlights

- **Restores and copies spend less time refreshing.** The writer refreshes at most every 5 seconds
  (plus once at the end of the run) instead of after every bulk batch. With no output index, it
  refreshes only the indices it wrote, never the whole cluster.
- **Scroll `--slices` is capped to the shard count**, so a dump no longer asks the engine to filter
  every document of a shard once per extra slice. `--sliceField` can slice on a numeric or date
  field instead of `_id`.
- **`--scrollDocSort` is available as an opt-in** when you want Elasticsearch's cheap `_doc` scroll
  order. It stays off by default so dump lines still match Node and 1.0.
- **gzip and zstd no longer stall the writer** between batches. The compressed bytes stay identical
  to 1.0.
- **At `--concurrency 1`, the next bulk batch is prepared while one is sending.** Sends never
  overlap, so the cluster still sees the same requests in the same order.
- **`searchdump-multi` takes record counts from its children**, so a large compressed dump is no
  longer read back just to fill the manifest. Ctrl-C now stops running children. `--verifyCounts`
  keeps the old full re-read when you want it.

## New flags and defaults

| Flag | Default | Tool | Meaning |
|---|---|---|---|
| `--refreshInterval=<seconds>` | `5` | `searchdump` | Refresh written indices at most this often while writing, plus once at the end of the run. Accepts seconds, optional decimals, and an optional trailing `s` (`0` to `86400`). `0` refreshes after every batch, as in 1.0 and Node. |
| `--noRefresh` | off (unchanged) | `searchdump` | Sends no refresh. Wins over `--refreshInterval`. |
| `--scrollDocSort` | off | `searchdump` | Opt-in. Adds `"sort":["_doc"]` to an unscored scroll body that has no user sort. |
| `--sliceCap=auto\|min\|total\|false` | `auto` | `searchdump` | A scroll dump lowers `--slices` to the primary-shard count of the input indices (one shard: unsliced) and prints a warning. `auto` (`true` is an alias) picks the count from the cluster version: the total on Elasticsearch 7.11–9.4, the smallest on Elasticsearch 5.0–7.10, 9.5+, OpenSearch and undetectable versions. `min` / `total` force one; `false` keeps the number you passed. |
| `--sliceField=<field>` | unset | `searchdump` | Slice on this numeric or date field with doc values, instead of `_id`. The mapping is checked first; a missing, non-numeric, or `doc_values:false` field is an error before the first search. Using this flag turns the shard-count cap off. |
| `--verifyCounts` | off | `searchdump-multi` | Always re-read dumped or loaded data and fail the index when the child's reported record count differs. |

Related switches that are not CLI flags:

- `SEARCHDUMP_BULK_PIPELINE=0` turns off encode/send overlap at `--concurrency 1` (for A/B checks).
- `SEARCHDUMP_MULTI_DEBUG=1` prints how `searchdump-multi` got each record count.
- `--statsFile` is a hidden `searchdump` flag. `searchdump-multi` passes it only to a child whose
  binary resolves to `searchdump*` (the `elasticdump` / `elasticdump-rs` compatibility symlinks
  included). A Node `--elasticdumpBin` never receives it.

## Behaviour changes versus 1.0

These compare 1.1 with the 1.0 line, including 1.0.0-beta1. Dump files, the
`multielasticdump.json` manifest, environment variable names, and the compatibility symlinks are
unchanged.

- **Refresh after every batch is no longer the default.** 1.0 refreshed the target after every
  successful bulk batch. 1.1 refreshes at most every 5 seconds (the clock starts at the first
  finished batch) and always once at the end of the run, including the `--concurrency 1` path,
  which previously relied on the last batch's refresh. After the run, the same documents are
  searchable; a reader watching *during* a restore may see new documents up to the interval later
  (the engine's own `index.refresh_interval`, 1 s by default, still applies as well). Use
  `--refreshInterval=0` for the 1.0 request pattern, or `--noRefresh` to send none.
- **A restore with no output index no longer refreshes the whole cluster.** 1.0 sent
  `POST /_refresh` in that case, which refreshes every index. 1.1 sends
  `POST /a,b,…/_refresh?ignore_unavailable=true&allow_no_indices=true` for the indices it actually
  wrote (after `--prefix` / `--suffix` / rename; data-stream creates record the stream), split so
  each URL path stays under ~3 KB, and sends nothing when nothing was written. With an output index
  the request is unchanged from 1.0.
- **`--slices` on a scroll dump is capped to the shard count.** In 1.0, `--slices` was used as
  given, even when it was higher than the primary-shard count, which can make the engine slower
  than an unsliced scroll. 1.1 lowers it (with a warning) unless you pass `--sliceCap=false`. With several input indices the
  limit follows the engine (`--sliceCap=auto`): Elasticsearch 7.11–9.4 spread slices across all
  shards of the request, so the total primary-shard count is used; Elasticsearch 5.0–7.10, 9.5+
  (scroll) and OpenSearch use the smallest count, as does an undetectable version. When
  settings cannot be read (for example no `view_index_metadata` privilege), the requested number is
  kept and a warning is printed. PIT (`--searchAfter --pit`) is never capped.
- **`--scrollDocSort` does not change default dumps.** Leave it off and the scroll request and the
  dump lines are the same as 1.0. With the flag on, unscored scroll hits are written with
  `"_score": null` and without the returned `sort` member; restores ignore both. The flag is
  skipped, with an info log, for PIT, `--searchAfter`, `--scrollId`, `--searchWithTemplate`, a user
  `sort`, scoring queries, and `track_scores` / `min_score` / `rescore`.
- **Compressed file output is produced on a per-part thread.** There is no flag. gzip and zstd
  bytes are identical to 1.0. The memory estimate counts three extra batches for a compressed file
  output.
- **`--concurrency 1` prepares the next bulk batch while one is in flight.** Sends still do not
  overlap. An error in batch N means batch N+1 is never sent; a run that stops with an error still
  finishes the one batch in flight. The version-conflict (409) and data-stream hints are printed
  once per run at `--concurrency N > 1` as well (1.0 printed them once per batch there). The memory
  estimate counts two extra batches.
- **`searchdump-multi` Ctrl-C stops running children.** 1.0 left them to finish unless the terminal
  also signalled them. 1.1 starts no new step, sends SIGTERM, then SIGKILL after 5 seconds. Each
  stopped child is named with its log, and its index is recorded as failed. Exit status 130 is
  unchanged. A restore still finishes in-flight bulk requests on SIGTERM and prints where to resume.
- **`searchdump-multi` no longer always re-reads data files for counts.** It prefers the child's
  `--statsFile` JSON, then the last `Total Writes: N` (and `Failed Writes: N`) log line, and only
  then scans the files. Summary and manifest formats are unchanged. Pass `--verifyCounts` to keep
  the 1.0 always-rescan check and fail the index on a mismatch.
- **The perf harness timer is exact.** `scripts/load/measure.py` no longer rounds wall time up to
  the next ~0.1 s tick. The `searchdump` and `searchdump-multi` binaries are unchanged by that
  commit.

## Behaviour changes versus Node elasticdump

File formats stay compatible: dumps made by either tool still load with the other, and
`searchdump-multi --nodeCompat` is unchanged.

Deliberate differences introduced in 1.1:

- **Refresh cadence (NODE-DIFFERENCES #26).** Node refreshes after every bulk batch
  (`_data.js` `reindex()`). 1.1 refreshes at most every `--refreshInterval` seconds (default 5)
  plus once at the end. `--refreshInterval=0` restores Node's per-batch refresh.
- **Refresh targets (NODE-DIFFERENCES #25).** When there is no output index, Node posts
  `${base.url}/_refresh`, which refreshes **every index in the cluster** after every batch. 1.1
  refreshes only the indices it wrote. The data visible after the run is the same; unrelated
  indices are no longer refreshed. With an output index, both tools refresh that index.
- **Scroll `_doc` sort (NODE-DIFFERENCES #27).** Node sets `sort: ['_doc']` on its request
  *options* object, not the body, so it never sends a sort and writes `"_score": 1`. 1.1 matches
  that by default (no sort, identical dump lines). Opt-in `--scrollDocSort` actually sends
  `"sort":["_doc"]` and writes `"_score": null`.

No Node compatibility difference (searchdump-only behaviour, or the same requests as Node):

- `--slices`, `--sliceCap`, and `--sliceField` (Node has no slicing).
- Off-thread compression (gzip bytes identical; zstd is a searchdump extension).
- Encode/send overlap (same requests in the same order at `--concurrency 1`).
- `searchdump-multi` Ctrl-C handling (not part of the dump format or CLI contract).
- Child-reported counts (`--statsFile` is never passed to a Node binary; Node's
  `Total Writes: N` log line is still parsed).

## Upgrade guide

1.1 is the next stable line. These changes are not backported to 1.0.

- **Existing dumps keep working.** NDJSON data files, metadata files, split parts, and
  `multielasticdump.json` are unchanged. `SEARCHDUMP_*` names, the deprecated `ELASTICDUMP_*`
  aliases, and the `elasticdump` / `elasticdump-rs` / `multielasticdump` compatibility symlinks
  are unchanged.
- **If a script or watcher needs documents to become searchable after every batch**, pass
  `--refreshInterval=0` (Node-identical requests) or `--noRefresh` if you do not want a refresh
  at all. Otherwise the new 5-second default is the one you get, including from
  `searchdump-multi` data runs unless you pass another value after `--`.
- **If you restore many indices into their original names** (no `--output-index`), only those
  indices are refreshed. That is intentional. Unrelated indices on the same cluster are no longer
  touched.
- **If you pass `--slices` higher than the primary-shard count** and want that number kept, add
  `--sliceCap=false`. To pin the limit instead of letting the version decide, use `--sliceCap=min`
  or `--sliceCap=total`. Live tests that exercise over-slicing already do this. PIT is not capped.
- **Leave `--scrollDocSort` off** when you compare dump files with Node or with 1.0, or when a
  scoring query / user sort / PIT / search-after is in use (the flag already refuses those
  cases). Turn it on only when you want `_doc` order on a plain scroll dump.
- **Ctrl-C on `searchdump-multi` now stops children.** Do not rely on them running to completion
  after the parent is interrupted. Use `--resume` (and the child's log) to continue a stopped
  load.
- **Pass `--verifyCounts`** if you want `searchdump-multi` to keep re-reading every data file and
  to fail the index when that count disagrees with the child. Without it, a successful child's
  reported count is trusted for the manifest and the summary.
- **Memory estimates are slightly higher** for compressed file output (three extra batches) and
  for `--concurrency 1` encode/send overlap (two extra batches). Peak behaviour is otherwise the
  same.

## Performance

*Placeholder — do not quote numbers here until the point-in-time run finishes.*

Each 1.1 commit, plus a re-measure of 1.0.0-beta1 with the 1.1 blocking-wait4 timer, will be built
and run on Elasticsearch 7.10, Elasticsearch 8.19 and OpenSearch 3 (plain). The step-by-step table
(wall median per case, change versus the previous commit, and a per-step summary) will be added
here and in the changelog.

Until those results are in, do not treat the tables in [README Performance](../../README.md#performance)
as 1.1 numbers: they were measured with the older 0.1 s timer and without these changes.

## See also

- [`CHANGELOG.md`](../../CHANGELOG.md) — Unreleased (1.1)
- [`README.md`](../../README.md) — flags, intentional differences, `searchdump-multi`
- [`NODE-DIFFERENCES.md`](../../NODE-DIFFERENCES.md) — #25 refresh targets, #26 cadence, #27 `_doc` sort
- [`project/plan/ROADMAP-1.1-PLAN.md`](../plan/ROADMAP-1.1-PLAN.md) — implementation order and the
  point-in-time test plan
