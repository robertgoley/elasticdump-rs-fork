# searchdump-rs 1.1: implementation plan (for review)

Status: **steps 1-4 approved and in progress** (see Status below); steps 5-9 still for review. Branch `release/1.1` is a local branch only and has not been pushed.
Scope: roadmap items 1.1-1 to 1.1-9 from `FEATURES-TODO.md`, section "Roadmap: 1.1 release (performance and robustness)".
Line references were rechecked at `0847643` (main, 2026-11-11). The roadmap table quotes them as of `a98be22`.

Rules that still apply: Rust 1.91 minimum (`rust-version = "1.91"`). No secrets in the repo; credentials appear only as blank variable references. Opt-in flags use Node-style camelCase names. Every change keeps or extends the existing tests.

## Status

Owner go-ahead for steps 1-4 on 2026-11-11 (t163u). Q1 and Q2 were not answered, so the recommended defaults are used: Q1 interval refresh every 5 s plus a final refresh (`--refreshInterval=0` = Node's per-batch refresh); Q2 option (c), opt-in `--scrollDocSort`.

| Step | Item | Status |
|---|---|---|
| 1 | 1.1-5 perf timer | in progress (release/1.1): harness change committed; offline selftests pass; beta1@timer2 re-measure pending |
| 2 | 1.1-2 targeted refresh | in progress (release/1.1): code + unit/contract tests committed, not yet compiled (suite running); A/B pending |
| 3 | 1.1-1 refresh cadence | planned |
| 4 | 1.1-3 `_doc` sort (opt-in) | planned |
| 5-9 | 1.1-4, 1.1-9, 1.1-6, 1.1-8, 1.1-7 | planned, not started |

"In progress" means committed on `release/1.1` but not yet compiled/tested or A/B measured (no cargo or podman while the beta1 suite runs).

## 0. Gate: the beta1 baseline comes first

* **Nothing lands until the beta1 baseline is complete.** The 16-engine L0 suite (`/workspace/perf-b1/run-all.sh`, `PERF_HOST_ID=grokbox-beta1`) has to finish and be aggregated into `scripts/perf/results/v1.0.0-beta1-L0.md`.
* While that suite runs, the box gets **no cargo builds or tests and no podman**, so the timings stay clean. Until then the only work is planning and reading code on this branch.
* Every 1.1 item is an A/B against that baseline:
  * A: `/workspace/perf-bin-beta1`.
  * B: the release build of `release/1.1` at the item's commit.
  * Both run on the same host, with the same tier and `--reps 3` (use 5 for cases under 3 s until 1.1-5 lands).
  * The B run uses its own host id, for example `PERF_HOST_ID=grokbox-1.1-<item>`, so its results never mix with the baseline.
* A change is accepted when its target cases improve beyond noise and no other case regresses by more than 5 % (or beyond the rep spread, whichever is larger) on the engines used.
* **Engine subset per item:**
  * Use the "quick trio" `es710-plain`, `es819-plain` and `os3-plain` for each item. That covers old ES, current ES and OpenSearch.
  * The full 16-engine run is for release candidates only (see Phase 5).
* 1.1-5 changes the harness, so the beta1 binaries are **re-measured with the new harness** before any other A/B (see question Q4).

## 1. Proposed order

| Step | Item | Phase | Effort | Main files | Why at this point |
|---|---|---|---|---|---|
| 1 | 1.1-5 perf timer resolution | 1: measure | S | `scripts/load/measure.py` | Every later A/B depends on honest wall times; today they are multiples of ~0.1 s |
| 2 | 1.1-2 targeted refresh, no whole-cluster `_refresh` | 2: refresh | S | `src/pipeline/bulk.rs` | A bug (cluster load). Same function as 1.1-1, and it adds the "indices written" set that 1.1-1 reuses. Lowest risk |
| 3 | 1.1-1 refresh cadence | 2: refresh | S | `src/pipeline/bulk.rs`, `src/cli.rs`, `src/lib.rs` | Largest expected win (2-4x restore/copy). Needs step 2's index set. Decides a Node-compat default (Q1) |
| 4 | 1.1-3 `_doc` sort for scroll | 3: retrieval | S | `src/retrieval/search_body.rs`, `src/retrieval/mod.rs`, `src/retrieval/extract.rs` | Independent and small, but it changes dump output bytes (Q2), so it goes after the refresh work and gets its own parity check |
| 5 | 1.1-4 cap slices to shard count, `--sliceField` | 3: retrieval | M | `src/retrieval/mod.rs`, `src/cli.rs`, `src/http/info.rs` | Touches the same slice/body setup as step 4, so it goes right after it. Needs a version-boundary investigation |
| 6 | 1.1-9 compression off the writer task | 4: pipeline CPU | S-M | `src/pipeline/file_sink.rs` | Self-contained, with output that can be checked byte for byte. Its win is visible mainly at L1 sizes |
| 7 | 1.1-6 encode next bulk batch while one is in flight | 4: pipeline CPU | M | `src/pipeline/bulk.rs` | Riskiest change in the hottest file. Done after 1.1-1/1.1-2 so it doesn't conflict with them, and so refresh no longer hides the overlap win |
| 8 | 1.1-8 `tokio::process` children in searchdump-multi | 5: multi | M | `src/bin/searchdump-multi.rs`, `Cargo.toml` (tokio `process` feature) | Restructures the child runner. Goes before 1.1-7 because 1.1-7 changes what the runner returns |
| 9 | 1.1-7 child-reported record counts | 5: multi | M (S if log-line based) | `src/bin/searchdump-multi.rs`, maybe `src/lib.rs` | Builds on the async runner from step 8. Keeps `count_records` as the fallback |
| 10 | Release-candidate check | 5 | — | — | Full 16-engine L0 run of `release/1.1` against the beta1 baseline, plus the parity checks. Report written to `scripts/perf/results/v1.1.0-rc-L0.md` |

How the order follows from the criteria:

* **Measurement first:** 1.1-5 comes first.
* **Dependencies:** 1.1-2 comes before 1.1-1 (shared index set and `refresh()`), and 1.1-8 before 1.1-7 (the runner shape).
* **Risk:** the S-sized bug fix and the large win come early. The two M-sized concurrency changes (1.1-6 and 1.1-8) come late, once the measurements are stable.
* **Node-compat risk:** the items that change behaviour (1.1-1 and 1.1-3) are isolated behind Robert's decisions (Q1 and Q2), and each is a separate commit that can be reverted on its own.
* **File overlap:**
  * `bulk.rs` items run one after another: 1.1-2, then 1.1-1, then 1.1-6.
  * Retrieval items run one after another: 1.1-3, then 1.1-4.
  * `searchdump-multi.rs` items run one after another: 1.1-8, then 1.1-7.
  * `file_sink.rs` (1.1-9) overlaps with nothing.

Each item is one commit on `release/1.1` (or two: tests, then the change), with a one-paragraph perf note in its message. No item is pushed until Robert asks.

## 2. Item details

### 1.1-5: Perf timer resolution (S), step 1

* **Where:**
  * `scripts/load/measure.py` lines ~177-185: a `while True` loop runs `proc_sample()`, then `os.wait4(pid, WNOHANG)`, then `time.sleep(0.1)`.
  * The roadmap says `scripts/perf/measure.py`, but the file is under `scripts/load/`.
* **Approach:**
  * The main thread blocks in `os.wait4(p.pid, 0)`, which still returns rusage.
  * A daemon sampler thread runs `proc_sample()` every 0.1 s until an Event is set.
  * Wall time is `time.monotonic()` taken right after `wait4` returns.
  * The sampler tolerates `/proc/<pid>` vanishing.
  * Nothing else changes in the output JSON.
  * Optionally add `"timer": "blocking-wait4"` to the result so aggregate.py can refuse to compare old and new harness runs without a flag.
* **Datasets:** optionally grow the L0 `logs` dataset so that the main cases run for 3 s or more (see Q4).
* **Compat:** harness only; the binaries don't change.
* **Tests:**
  * Add a `scripts/perf/selftest.py` case that measures `sleep 0.23` and asserts that wall time falls within 0.23 to 0.26 s and is not a multiple of 0.1.
  * Add an exit-code and rusage regression check.
* **Measure:**
  * Re-run beta1 binaries on the quick trio with the new harness, giving the "beta1@timer2" baseline.
  * Check that the rep-to-rep spread for short cases (`dump-small`, `mapping-*`, `settings-*`) drops.

### 1.1-2: No whole-cluster refresh when there is no output index (S), step 2

* **Where:** `src/pipeline/bulk.rs`:
  * `refresh()` at ~1102-1121 falls back to `POST /_refresh` when `opts.index` is `None`.
  * The per-batch call is at ~878-884, and the drain-time call at ~622-626.
  * `index_for()` at ~634 resolves each document's target index, with prefix and suffix, at ~660.
* **Approach:**
  * Record every resolved target index name in a set shared by the workers. `worker()` (~549) already clones Arcs.
  * `refresh()` sends `POST /<i1>,<i2>,…/_refresh?ignore_unavailable=true&allow_no_indices=true`.
  * Index lists are split so each URL stays under ~3 KB.
  * When `opts.index` is set, the behaviour is unchanged.
  * Data streams and aliases are valid refresh targets as written.
* **Compat:**
  * Node does the same whole-cluster refresh (`_data.js reindex()` posts `${base.url}/_refresh`, where `base.url` is the cluster URL when no index is given).
  * This is therefore a deliberate divergence, but every written index is still refreshed, so what a later reader can see is unchanged.
  * Recommendation: make it the default with no flag, and record it in the compat notes in README/FEATURES.
* **Tests:**
  * Unit test with a mock server: a multi-index restore with no `--output-index` sends `/a,b/_refresh` and never `/_refresh`.
  * URL chunking with many indices.
  * The prefix/suffix rename is reflected in the refreshed names.
* **Measure:**
  * Cases `restore-none` and `multi-restore-p1/p4/p8` on the quick trio.
  * Check the `refresh_total` delta on unrelated indices, which should be 0. measure.py's cluster sampler can be extended to read `_stats/refresh`.

### 1.1-1: Refresh cadence (S), step 3

* **Where:**
  * `src/pipeline/bulk.rs` ~878 (per-batch refresh after every successful batch) and ~614-628 (`drain()` refreshes only when `concurrency > 1`).
  * `src/cli.rs` ~238 (`--noRefresh`) and `src/lib.rs` ~459 (`refresh: !cli.no_refresh()`).
* **Approach:**
  * Replace `refresh: bool` with `RefreshPolicy { Off, EveryBatch, Interval(Duration) }`.
  * `Interval` refreshes when at least the interval has passed since the last refresh, using a shared `Instant` behind the existing `refresh_lock`.
  * `drain()` **always** does one final refresh unless the policy is `Off`. That fixes the current `concurrency == 1` path, which relies on the last batch's refresh.
  * Reuse 1.1-2's index set.
* **Flags:**
  * `--refreshInterval=<dur>`, where `0` means every batch.
  * `--noRefresh` stays and takes precedence.
  * The default depends on **Q1**. Recommendation: `5s` by default, with `--refreshInterval=0` documented as "Node-identical".
* **Compat:**
  * Node refreshes after every batch (`_data.js` ~267 calls `reindex()` after each write).
  * At the end of a run the data visible is identical. Only readers watching *during* a restore see it later.
* **Tests:**
  * Policy unit tests with `tokio::time::pause`, covering refresh counts for N batches under each policy and that the final refresh always happens.
  * Concurrency 1 and greater.
  * `--noRefresh` still sends zero refreshes.
* **Measure:**
  * Add perf cases `restore-norefresh` and `copy-norefresh` (A/B upper bound).
  * Compare `restore-*`, `copy` and `restore-split-*` on the quick trio.
  * Expect 2-4x at L0, from ~17.8k docs/s for a single restore today.

### 1.1-3: `_doc` sort for scroll (S), step 4

* **Where:**
  * `src/retrieval/search_body.rs` ~10-12 (`default_search_body`) and ~34-65 (`finalize_search_body`).
  * Applied only on the scroll path chosen in `src/retrieval/mod.rs` (~325-336).
  * PIT and `search_after` already sort on `_shard_doc` (`pit_search.rs`), so they are not changed.
* **Approach:**
  * When the search type is scroll and the user's body has no `sort`, insert `"sort":["_doc"]`.
  * A user-supplied `sort` is never touched.
* **Compat:**
  * Node sets `sort: ['_doc']` on the *request options object* (`_data.js` ~115), not the body, so Node actually sends **no** sort.
  * Our hits are written raw. With `_doc` sort, each hit gets `"_score": null` and a `"sort":[n]` member, so the dump lines would differ from Node's output (`_score: 1`, no `sort`).
  * Restore ignores both fields, but byte-for-byte parity checks would flag them. **Q2** decides between:
    * (a) on by default, with the injected `sort` member stripped from output hits and `_score: null` accepted;
    * (b) on by default with both normalised (strip `sort`, rewrite `_score` to 1.0), which costs a small extra rewrite per hit;
    * (c) opt-in `--scrollDocSort`.
  * Recommendation: (a) or (c). A scoring query (non-`match_all`) whose user wants scores must keep its scores, so the injection is skipped whenever the query is not `match_all`/`match_none`, or when `--searchBodyTemplate` is used.
* **Tests:**
  * `search_body.rs` unit tests for the default body, a user body with sort, a user body without sort, and PIT/search_after untouched.
  * An `extract.rs` test that the injected `sort` is stripped.
  * A nodecmp/parity run (`scripts/load/nodecmp.py`) on `dump-none` to confirm that the documented differences are the only ones.
* **Measure:** `dump-none`, `dump-big` and `dump-scroll-s1..s8` on the quick trio. The gain grows with index size, so L1 shows it best.

### 1.1-4: Cap slices to the shard count, plus optional `--sliceField` (M), step 5

* **Where:**
  * `src/retrieval/mod.rs` ~325-346 (slice count and worker clamp) and ~520-530 (`"slice": {"id","max"}`).
  * `src/cli.rs` ~542-544 (`--slices`).
  * Engine/version info in `src/http/info.rs`.
* **Approach:**
  * Before planning slices, read the primary-shard counts of the target indices (`GET /<index>/_search_shards`, or `_cat/shards?h=index,prirep`).
  * When `--slices` is greater than the smallest per-index primary count, warn or cap (**Q3**).
  * Confirm the engine/version boundary where slices above the shard count fall back to the `_id`-hash filter (ES before ~7.16; also check OS 1.3/2.x/3.x). Use the existing `dump-scroll-s{1,2,4,8}` and `dump-pit-s*` cases on 1-shard and multi-shard indices for this.
  * `--sliceField=<numeric doc-values field>` adds `"field"` to the slice object, so the user picks a cheap field. It is validated against the mapping with a clear error if the field is missing or has no doc values.
* **Compat:**
  * `--slices` is our extension, and Node has no slicing, so there is no Node risk.
  * Capping changes behaviour only for inputs that were slower anyway.
* **Tests:**
  * Mock-server tests: a 1-shard index with `--slices 2` gives the warning and the cap; `--sliceField` appears in the body.
  * A missing field gives an error.
  * A multi-index pattern uses the smallest count.
* **Measure:** `dump-scroll-s2/s4/s8` and `dump-pit-s*` on es710-plain (the known 2.1-2.2 s vs 0.8-0.9 s case), es819-plain and os3-plain.

### 1.1-9: Compression off the writer task, with a recycled buffer (S-M), step 6

* **Where:** `src/pipeline/file_sink.rs`:
  * `GZ_DRAIN = 256 KiB` at ~16.
  * `Encoder` at ~50-85.
  * `Part::write`/`flush`/`seal` at ~329-358. `std::mem::take(enc.pending())` sits at ~334 and ~345.
* **Approach:**
  * Give each `Part` a dedicated encoder thread (a `std::thread`, or `spawn_blocking` holding the encoder), fed through a bounded channel of input chunks.
  * Compressed output comes back through a second bounded channel. Two or three `Vec<u8>` buffers circulate between the threads, so the drain allocates nothing.
  * `flush`/`seal` send a control message and wait for the matching output.
  * Errors are carried back and surface on the next write or the seal.
  * Cancel drops the channels and joins the thread.
  * zstd multithread mode (the `zstdmt` feature) is **not** in scope, because it adds a C build feature and goes through the supply-chain gates. It can be noted as a follow-up.
* **Compat:** compressed bytes are identical, because the same encoder sees the same input in the same order. No flag is needed.
* **Tests:**
  * Byte-identical output against the current path for gzip-6 and zstd-3 across fixed inputs (proptest over chunk sizes).
  * Split parts and seal ordering.
  * An error injected in the encoder surfaces.
  * Cancellation leaves no thread behind.
* **Measure:**
  * `dump-gzip`, `dump-zstd`, `dump-split-gzip` and `dump-split-zstd` on the quick trio, plus writer CPU from rusage.
  * The gain is small at L0. It needs an L1 run (local or on the box) to show the effect (Q4).

### 1.1-6: Encode the next bulk batch while one is in flight (M), step 7

* **Where:** `src/pipeline/bulk.rs`:
  * `write_batch` at ~586-611: with `concurrency <= 1` it calls `process_batch` inline.
  * `process_batch` at ~814.
  * `encode_doc` at ~671.
  * `write_chunk` at ~886.
* **Approach:**
  * Split `process_batch` into `encode(buf) -> EncodedBatch` (CPU only) and `send(EncodedBatch)` (network, retries, item errors).
  * At `--concurrency 1`, keep a one-deep pipeline:
    * encode batch N+1;
    * await the in-flight send of batch N;
    * start sending N+1.
  * Sends never overlap, so cluster ordering and the retry/`--ignore-errors` semantics are unchanged.
  * Error handling: an error from batch N cancels N+1 before it is sent.
  * `next_doc` and the per-document indices are computed at encode time, as `write_batch` does today.
  * Memory: at most two encoded batches. memplan (`src/memplan.rs`) must count the second buffer.
* **Compat:** no wire or ordering change. No flag is needed, but an env or hidden switch to disable it helps during the A/B.
* **Tests:**
  * Mock server: request order at concurrency 1 is unchanged.
  * A failure on batch N means N+1 is never sent.
  * `--ignore-errors` counts are unchanged.
  * A cancel mid-flight works.
  * Existing `encode_doc` proptests pass.
  * A memplan test for the extra buffer.
* **Measure:** `restore-none`, `restore-gzip` and `copy` at `--concurrency 1` on the quick trio, after 1.1-1 (otherwise refresh dominates). Target: 10-20 %.

### 1.1-8: `tokio::process` children in searchdump-multi (M), step 8

* **Where:** `src/bin/searchdump-multi.rs`:
  * `Runner::run_as` at ~1353-1388: `std::process::Command` at ~1368, blocking `.status()` at ~1379.
  * Call sites in `block_in_place` at ~1590, ~1685, ~1738, ~1786, ~1821 and ~1864.
  * The Ctrl-C handler at ~2183 only sets `cancelled`, so running children go on until they finish.
* **Cargo.toml:** add the `process` feature to tokio (~32-41). tokio is already a dependency, so this adds no new crate; check the SBOM/deny gates.
* **Approach:**
  * `run_as` becomes `async`, using `tokio::process::Command` with `kill_on_drop(true)`.
  * It uses `select!` between `child.wait()` and a cancel `Notify`/`CancellationToken` (tokio-util is already a dependency).
  * On cancel, send SIGTERM, wait a grace period (5 s), then SIGKILL, and log which children were stopped.
  * Remove the `block_in_place` wrappers.
* **Compat:** CLI and behaviour are unchanged, except that Ctrl-C now stops running children promptly and reports them. Record this in the README.
* **Tests:**
  * With a fake child binary (a shell script that sleeps), cancel kills it within the grace period.
  * A non-zero exit still reports the log tail.
  * `--parallel 16` with sleeping children does not starve the runtime (a timer task keeps ticking).
* **Measure:**
  * `multi-dump-p1/p4/p8` and `multi-restore-p1/p4/p8` on the quick trio. Expect no regression and better scaling at p8.
  * A manual Ctrl-C check.

### 1.1-7: Child-reported record counts in searchdump-multi (M; S with the log-line option), step 9

* **Where:** `src/bin/searchdump-multi.rs`:
  * `count_records` at ~1001 decompresses the whole file.
  * It is used at ~1605 (dump summary) and ~1807 (split-part restore totals).
* **Approach:**
  * After 1.1-8, the runner returns a `ChildStats`.
  * Preferred source: a small JSON written by the child through a hidden `--statsFile=<path>` flag. `src/lib.rs` already computes `report.written` and prints "Total Writes" at ~1155/1173.
  * Fallback 1: parse `Total Writes: N` from the child's log. Node elasticdump prints the same line, so this also works with `--elasticdump_bin` pointing at Node.
  * Fallback 2: the existing `count_records` scan, used when both are missing or when `--verifyCounts` is set. That flag is an option to keep the full check available.
* **Compat:**
  * The summary/manifest format is unchanged.
  * The count source is logged at debug level.
  * Split dumps report per-run totals; per-part counts need the stats file, or the scan when that is missing.
* **Tests:**
  * Stats-file round trip.
  * Log-line parse for the Rust and Node formats.
  * Fallback to the scan when both are absent.
  * The existing `count_records` test at ~2761 stays.
* **Measure:** `multi-dump-p*` with gzip/zstd. The win is minutes on dumps of 100 GB or more, and is negligible at L0, so check correctness at L0 and time it at L1 if Q4 allows.

## 3. Open questions for Robert

1. **Q1, refresh default (1.1-1):** should the new default be interval refresh (recommended: every 5 s plus a final refresh, with `--refreshInterval=0` restoring Node's refresh-every-batch), or should we keep Node's per-batch default and make the interval opt-in?
2. **Q2, `_doc` sort output (1.1-3):** should `_doc` sort be on by default for scroll dumps with no user sort? On by default means hits show `_score: null`, and we strip the injected `sort` field. The alternative is to make it opt-in (`--scrollDocSort`) to keep Node-identical dump lines.
3. **Q3, slices above the shard count (1.1-4):** should we cap automatically to the primary-shard count with a warning (recommended), or only warn and run as asked?
4. **Q4, baseline and dataset size:**
   * After the timer fix, should the beta1 binaries be re-measured on the quick trio only (recommended), or on all 16 engines?
   * May we add a larger L0 dataset or an L1 run (box disk/time) so that 1.1-9, 1.1-3 and 1.1-7 can show their wins?
5. **Q5, the 1.1-2 bug and 1.0.0:**
   * Should the whole-cluster refresh fix be backported to the 1.0.0 final (it is a correctness/cluster-load bug), or wait for 1.1?
   * Should `release/1.1` merge to `main` only after 1.0.0 is tagged?

## 4. Effort summary

| Item | Effort |
|---|---|
| 1.1-5 timer | S |
| 1.1-2 targeted refresh | S |
| 1.1-1 refresh cadence | S |
| 1.1-3 `_doc` sort | S |
| 1.1-4 slice cap + `--sliceField` | M (S if warn-only, no `--sliceField`) |
| 1.1-9 compression thread | S-M |
| 1.1-6 encode overlap | M |
| 1.1-8 tokio::process | M |
| 1.1-7 child counts | M (S with the log-line source only) |

S means under a day and M a few days. Coding total: about 2-3 weeks. The perf A/B runs take about 1-2 h per item on the quick trio, and the release-candidate run takes as long as the beta1 suite (most of a day).
