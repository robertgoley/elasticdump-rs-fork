# elasticdump-rs: Test Expansion Requirements

**Covers:** API compatibility, data loads at scale, adversarial/chaos testing, and supply-chain/hostile-code review.

> Status: plan only (2026-10-29). Nothing here has been implemented. I read the code at `/workspace/elasticdump-rs`
> (branch `rewrite`, HEAD `d991ad8`) without changing it. No builds were run and no containers were started.
> Only read-only `cargo metadata` / `cargo tree --offline --locked` and `git blame` were used.
> Companion documents: `PLAN.md`, `TYPES-PLAN.md`, `GAPS.md` (same directory).
> Every dataset link in §5 was checked with `curl` on 2026-10-29. Dead links are excluded and listed in Appendix A.
>
> §7.3 (native/unsafe inventory) and §8 (injection review) were added the same day, under the same read-only rules.

**Requirement counts (204 total):**

| Area | must | should | could | Total |
|---|---|---|---|---|
| INF (harness) | 10 | 4 | 0 | 14 |
| API (compatibility) | 38 | 9 | 0 | 47 |
| LOAD (data loads) | 13 | 6 | 1 | 20 |
| DATA (sources/generators) | 6 | 2 | 2 | 10 |
| BRK (adversarial/chaos) | 55 | 17 | 1 | 73 |
| SUP (supply chain) | 14 | 7 | 2 | 23 |
| INJ (injection) | 13 | 3 | 1 | 17 |
| **Total** | **149** | **48** | **7** | **204** |

## 0. Conventions

* **IDs:** `TST-INF` (harness and infrastructure), `TST-API` (API compatibility), `TST-LOAD` (data-load), `TST-DATA` (datasets and generators), `TST-BRK` (adversarial/chaos), `TST-SUP` (supply chain and hostile-code review).
* **Priority:** **must** = needed before calling the rewrite production-ready. **should** = high value, next. **could** = nice to have.
* **Effort:** S < 1 day, M = 2–4 days, L = 1–2 weeks (one engineer who knows the codebase).
* **Standard acceptance (STD-ACC)** applies to every TST-BRK row and to every failure-path test, in addition to its own criterion:
  1. **no panic:** no `panicked at` and no abort or SIGSEGV;
  2. **no silent data loss:** a run that exits 0 must have a destination digest equal to the source digest, or print an explicit, counted loss summary that the test asserts;
  3. **clear error:** names the endpoint, operation, HTTP status or OS error, and what to do next;
  4. **correct exit code:** 0 = success, 1 = runtime failure, 2 = usage error, 130 = interrupted;
  5. **no secret leakage:** none of the run's planted secrets appear in stdout, stderr, output files, staging names, or `--debug` logs.
* **Digest:** the order-independent document-set digest defined in TST-INF-005.

---

## 1. Where testing stands today (read from the tree)

| Layer | Where | Count | Notes |
|---|---|---|---|
| Unit | `src/**` `#[test]` | ~150 | Retry classifier, bulk encoding, splitter, PIT coordinator, ILM→ISM, rename, URL/auth |
| CLI contract (no containers) | `tests/cli_contract.rs` | 22 | `tests/support/mock_es.rs` serves scripted canned responses **sequentially, one connection at a time** (`Connection: close`). It cannot do concurrency, routing by path, or stalls/truncation |
| Legacy integration (upstream) | `tests/integration_test.rs` | 36 | Upstream code (2,511 of 2,573 lines blame to upstream authors). ES at `IT_ES_URL`. Feature-gated `large_scale_test` / `manual_test` |
| Podman IT suite | `tests/it/*.rs` | 45 | smoke 3, data 9, engine 3, files 4, meta 5, meta_types 9, cross 4, concurrency 4, tls_auth 4. Run per profile by `scripts/it/run.sh` |
| Fault injection | `tests/it/concurrency.rs::fault_proxy` | 1 suite | In-process TCP proxy that cuts responses or refuses connections. Used only for restore (20/20 runs) |
| Node interop | `files.rs`, `meta.rs`, `meta_types.rs` | some | Runs only when `/tmp/ed/node_modules/.bin/elasticdump` (6.125.1) exists. Only **5 Node fixtures are tracked** (see SUP finding F-04) |
| Benchmarks | `scripts/benchmark/*` | 2 scripts | 100k-doc quick run vs Node; upstream 2M-doc ES 7.17 script |
| Property / fuzz | none | 0 | No `proptest`, `quickcheck`, or `cargo-fuzz` |
| Recorded golden fixtures | none | 0 | Contract tests hand-write responses. Nothing is recorded from real versions |

**Versions in the harness:** OpenSearch 1.3.20, 2.19.x, 3.x. Elasticsearch 7.10.2 (OSS bitnamilegacy for plain; official default distribution for secure), 7.17.29, 8.6.1, 8.19.23, 9.1.2. Each is tested plain and secured. `cross` brings up two nodes at 256m heaps.

**Gaps that drive this plan:**
* No per-version record of the request/response contracts the tool depends on.
* No tests that run without a cluster for anything beyond simple scripted sequences.
* No parser fuzzing.
* Load testing tops out at 2M docs.
* No end-of-run correctness digest.
* Chaos testing covers only connection cuts during restore.
* There is no supply-chain gate beyond `cargo audit` / `cargo deny` / image scans.
* `README.md` links `TESTING-MATRIX.md`, but **the file does not exist** in the tree or in git history.

**Box budget (measured now):**
* 8 vCPU.
* 15 GiB RAM, about 12 GiB available, shared with other agents' work.
* 35 GB free disk on `/` (73% used).
* podman 5.4.2.
* Node elasticdump 6.125.1 installed at `/tmp/ed`.

---

## 2. Test tiers and harness (TST-INF)

| Tier | Trigger | Needs | Wall-clock budget | Contents |
|---|---|---|---|---|
| T0 `unit` | every PR | nothing | < 3 min | unit, proptest (default cases), golden replay |
| T1 `contract` | every PR | nothing | < 5 min | router-mock contract tests, error paths, fuzz smoke (60 s per target) |
| T2 `it` | every PR (subset), nightly (full) | podman, 1 node | 15 min per profile | live per-version contracts, Node interop, TLS/auth |
| T3 `cross` | nightly | podman, 1–2 nodes | 60 min | export-once/import-everywhere matrix, live pairs |
| T4 `load-smoke` (L0) | every PR touching `src/` | 1 node | < 10 min | 100k docs, correctness digest, perf sanity |
| T5 `load-nightly` (L1) | nightly | 1 node, 4–6 GB RAM, 20 GB disk | < 2 h | 10M docs, scaling curves, Node comparison, chaos under load |
| T6 `soak` (L2) | weekly or manual, big runner | ≥ 16 vCPU, 64 GB, 1 TB NVMe | 6–12 h | 100M+ docs, 100+ GB, leak checks |
| T7 `chaos` | nightly | T1 + T2 infrastructure | < 60 min | TST-BRK suites |
| T8 `supply-chain` | every PR + weekly | network sandbox | < 15 min | TST-SUP gates |

| ID | Pri | Requirement | Acceptance criteria | Harness change | Effort |
|---|---|---|---|---|---|
| TST-INF-001 | must | Introduce tier selection with `cargo-nextest` profiles (`unit`, `contract`, `it`, `load`, `chaos`) plus Cargo features `it`, `load`, `chaos`, `fuzz`. Every test belongs to exactly one tier | `cargo nextest run --profile <tier>` runs only that tier. JUnit XML is produced. Per-test slow-timeout is set (T0 30 s, T2 5 min, load per scenario) | `.config/nextest.toml`; move `large_scale_test`/`manual_test` into `load` | M |
| TST-INF-002 | must | Replace the sequential `MockEs` with a **router mock**: match on method + path + query + body predicate; per-route response scripts; concurrent connections and keep-alive; full request recording; fault modes (delay, stall after headers, stall mid-body, truncate at N bytes, RST, slow drip, wrong `Content-Length`, bad chunk framing, `Content-Encoding: gzip` with plain body, HTML bodies, redirects) | Existing 22 contract tests pass unchanged on top of it. Each fault mode has a self-test | `tests/support/router_mock.rs` (hyper 1 server, already in the dependency tree) | L |
| TST-INF-003 | must | **Record/replay golden infrastructure:** `IT_RECORD=1` puts a recording proxy between the binary and a live container and writes `tests/golden/<engine>-<version>/<case>/{NNN.req.json,NNN.resp.json,meta.toml}`. A normaliser masks volatile fields (`took`, `_shards`, uuids, `creation_date`, `pit_id`/`_scroll_id` values, node names, `X-Opaque-Id`, `Date`). Replay mode feeds the recordings to the router mock | Re-recording an unchanged version gives a zero diff after normalisation. Replay runs offline in T0/T1 | recorder + normaliser module; `scripts/golden/record.sh` | L |
| TST-INF-004 | must | Promote the in-process fault proxy (`tests/it/concurrency.rs::fault_proxy`) to `tests/support/fault_proxy.rs` with a policy DSL (cut-after-bytes, refuse-every-N, latency, bandwidth cap, half-close, per-direction). Optionally add a **toxiproxy** container (`ghcr.io/shopify/toxiproxy`, MIT) for tests that need it outside the test process (Node comparison, CLI-only runs) | Both proxy kinds pass the same fault conformance tests | support module + `scripts/it/toxiproxy.sh` | M |
| TST-INF-005 | must | **Document-set verifier** `edverify` (test-only binary). It streams a cluster index (its own sliced-PIT/scroll reader, independent of the code under test) or a file (NDJSON/gz/zst/split set). Output: `count` plus an order-independent multiset digest: per document `h = SHA-256(index_rel ‖ 0x00 ‖ _id ‖ 0x00 ‖ routing ‖ 0x00 ‖ raw _source bytes)`, combined as `(Σ h mod 2^256, XOR h, count)`. Per-shard sub-digests localise mismatches. `--canonical` mode re-serialises `_source` with sorted keys and arbitrary-precision numbers for cross-engine or synthetic-source cases. Memory is constant | Detects 1 changed byte, 1 missing doc and 1 duplicate doc in 100M docs. Runs at ≥ 200k docs/s on the box. Reports the first 10 differing `_id`s by re-scanning the mismatched shard | new `tools/edverify` crate (dev-only) | M |
| TST-INF-006 | must | **Metrics wrapper** `scripts/load/measure.sh`: runs a command under `/usr/bin/time -v` and its own cgroup (`systemd-run --user --scope` or `podman run --cgroups`). Records wall time, user/sys CPU, max RSS, voluntary/involuntary context switches, read/write bytes, open FDs (sampled from `/proc/<pid>/fd`), exit code. It also parses the tool's stderr for retry warnings and `Failed Writes` and samples the cluster (`_nodes/stats`) before and after | Emits `results/<run-id>.json` against a JSON schema. A comparison script flags regressions above a threshold | script + schema | M |
| TST-INF-007 | must | **Resource guards** for the shared box: (a) memory preflight for every profile (generalise `CROSS_MIN_MB`); (b) disk preflight (`df`) before load tiers; (c) a `flock` on `/tmp/elasticdump-rs-it.lock` so concurrent agent runs do not fight over ports or memory; (d) every container gets `--memory`/`--cpus` caps; (e) ports bind to 127.0.0.1 only | A run on a box with too little memory SKIPs with a reason and exits 0 (or 1 when `*_REQUIRED=1`). Two parallel `run.sh` invocations serialise | `scripts/it/up.sh`, `run.sh` | S |
| TST-INF-008 | should | **Dataset cache manager** `scripts/data/fetch.sh <dataset>` with pinned URL, expected size, SHA-256, `curl -C -` resume, a cache directory (`$ED_DATA_CACHE`, default `/var/tmp/elasticdump-rs-data`) and converters to bulk-ready NDJSON with deterministic `_id`s (TST-DATA-010) | A second fetch is a no-op. A checksum mismatch fails. `--max-docs N` takes a prefix without decompressing the whole file | script + `datasets.toml` | M |
| TST-INF-009 | must | **Seeded generator `edgen`** (see §5.3) | see TST-DATA-001..009 | new `tools/edgen` crate | L |
| TST-INF-010 | should | CI layout: PR = T0, T1, T4 plus a T2 subset (OS 2.19 plain+secure, ES 9.1 plain, ES 7.10 OSS). Nightly = full T2, T3, T5, T7. Weekly = T6 on a big runner, T8 full. Failure artefacts: tool logs, `podman logs`, metrics JSON, digests, golden diffs | All jobs green on `rewrite`. A nightly failure opens no ticket automatically (Robert's call) but leaves artefacts | `.github/workflows/{ci,nightly,soak}.yml` | M |
| TST-INF-011 | must | Create the missing `TESTING-MATRIX.md`, generated from `IT-SUMMARY` lines, the golden fixture index and image digests (`podman image inspect --format '{{.Digest}}'`) | The README link resolves. Every image is listed with its digest. The cross-version expectation table (TST-API-071) is embedded | `scripts/it/matrix.sh` | S |
| TST-INF-012 | should | Pin container images by digest in `scripts/it/up.sh` / `run.sh` (tags today) and record the digests in TESTING-MATRIX.md | A re-pushed tag cannot change what CI runs without a diff | scripts | S |
| TST-INF-013 | should | Flake policy: correctness tests never auto-retry. A quarantine list in `nextest.toml` needs an owner and a date. Every chaos test prints its seed and can be replayed (`CHAOS_SEED=`) | A quarantined test older than 14 days fails CI | nextest config | S |
| TST-INF-014 | must | Track the Node fixtures that are currently untracked (`.gitignore` has `*.jsonl`, so `tests/fixtures/node/stored_scripts.jsonl` is ignored; see F-04) and add a CI check that every fixture the tests reference is tracked | A clean checkout passes `meta_types::scripts_round_trip_search_template_and_node` | `.gitignore` exception + check script | S |

---
## 3. API compatibility (TST-API)

### 3.1 Endpoint inventory (what the tool sends; from `src/`)

| # | Request | Used by (module) | Version and engine differences to pin down |
|---|---|---|---|
| E1 | `GET /` | `http/client.rs::info`, `http/info.rs::ClusterInfo::parse` | `version.distribution` (OpenSearch only); `build_flavor` (`oss`, `default`); OpenSearch `compatibility.override_main_response_version` reports `7.10.2`; unparsable body falls back to "Elastic 5.0.0" |
| E2 | `POST /{idx}/_search?scroll=` (+ `slice`) | `retrieval/scroll.rs` | `hits.total` int vs object; `track_total_hits`; max slices (`index.max_slices_per_scroll`, default 1024); `max_result_window` vs `size` |
| E3 | `POST /_search/scroll`, `DELETE /_search/scroll` | `scroll.rs`, `retrieval_task.rs` | `scroll_id` rotation; 404 on clear; expired context error shape |
| E4 | ES: `POST /{idx}/_pit?keep_alive=`, `DELETE /_pit`; OpenSearch: `POST /{idx}/_search/point_in_time?keep_alive=`, `DELETE /_search/point_in_time` | `retrieval/pit.rs` | PIT exists from ES 7.10 (default distribution); missing in OSS and OpenSearch < 2.4; sliced PIT needs ES ≥ 7.15; `_shard_doc` needs ES ≥ 7.12; OpenSearch id field names |
| E5 | `POST /_search` with `pit` + `search_after` (+ `slice`) | `retrieval/pit_search.rs` | `pit_id` rotation per page; sort values (`_shard_doc` long vs `_doc` + `_id`) |
| E6 | `POST /{idx}/_search` with `search_after`, no PIT | `retrieval/search_after.rs` | needs a user sort; tie handling |
| E7 | `POST /_render/template` | `lib.rs` (`--searchWithTemplate`) | stored vs inline template |
| E8 | `POST /{idx}/_bulk` or `/_bulk` (NDJSON, optional gzip body) | `pipeline/bulk.rs` | `routing` vs `_routing` (< 7), `_type` (< 7), `version_type`, item response keys and `error.type` names, `http.max_content_length` (413), gzip request bodies |
| E9 | `POST /{idx}/_refresh` | `bulk.rs::refresh` | refresh errors are only warned |
| E10 | `GET/PUT /{idx}/_mapping` | `meta/index_scoped.rs` | typed vs typeless; `include_type_name` (removed in 8); runtime fields; `flattened` vs `flat_object`; `_source` modes |
| E11 | `GET /{idx}/_settings`, `PUT /{idx}` | `index_scoped.rs` | engine-only keys; generated keys; `index.version.*` |
| E12 | `GET /{idx\|*}`, `PUT /{idx}` (`--type index`) | `index_scoped.rs` | hidden/system indices, `.ds-*` backing indices, closed indices |
| E13 | `POST /{idx}/_close`, `/_open`, `PUT /{idx}/_settings` (analyzer) | `index_scoped.rs::apply_analyzer` | `wait_for_active_shards`; close is blocked on some system indices |
| E14 | `HEAD /{idx}` and other existence checks | `meta/http.rs` | 404 vs 200 semantics; no body |
| E15 | `GET {idx\|*}/_alias/{a\|*}`, `POST /_aliases` | `meta/alias.rs` | `is_write_index`, `is_hidden`, filter/routing fields |
| E16 | `GET/PUT /_template/{n}` | `meta/templates.rs` | deprecated (Warning header) on ES ≥ 8; `order`, `version` |
| E17 | `GET/PUT /_component_template/{n}`, `/_index_template/{n}` | `templates.rs` | ES ≥ 7.8, OpenSearch ≥ 1.0; fields added per version (`allow_auto_create`, `ignore_missing_component_templates`, `deprecated`, `data_stream_options`, `template.lifecycle`) |
| E18 | `GET/PUT /_ilm/policy/{n}` | `meta/policy/mod.rs` | missing on 7.10 OSS; `_meta.managed`; new actions per version |
| E19 | `GET /_plugins/_ism/policies?size=&from=`, `PUT /_plugins/_ism/policies/{id}[?if_seq_no&if_primary_term]` | `policy/mod.rs` | 1.3 vs 2.x vs 3.x shapes; `_opendistro/_ism` legacy alias is never used |
| E20 | `GET /_cluster/state/metadata`, `POST /_scripts/{id}` | `meta/scripts.rs` | needs the `monitor` privilege; mustache vs painless |

### 3.2 Requirements

| ID | Pri | Requirement | Acceptance criteria | Harness change | Effort |
|---|---|---|---|---|---|
| TST-API-001 | must | Machine-readable endpoint inventory (`tests/golden/endpoints.toml`, rows E1–E20 plus the response JSON pointers each one reads). A guard test fails when the binary sends a request that matches no inventory row | Any new endpoint in a PR fails the guard until the inventory and goldens are updated | router mock records → matcher | S |
| TST-API-002 | must | **Live per-version contract suite:** for each of the 9 images (OS 1.3.20, 2.19.x, 3.x; ES 7.10.2 OSS, 7.10.2 default, 7.17.29, 8.6.1, 8.19.23, 9.1.2) and each endpoint E1–E20 that applies, run the operation through the recorder. Assert the status code and that every JSON pointer the tool reads exists with the expected type (e.g. `hits.hits[*]._id:string`, `hits.total.value:int\|hits.total:int`, `_shards.failed:int`, `pit_id:string`, `items[*].<action>.status:int`) | One green row per (version, endpoint) in TESTING-MATRIX.md. "Not applicable" (e.g. PIT on OS 1.3) is asserted as the specific expected failure, not skipped | recorder (INF-003), pointer schema per endpoint | L |
| TST-API-003 | must | **Golden replay** of every recorded exchange in T0/T1: the binary runs end to end against replayed responses, and its output file/stdout is byte-compared with a stored golden output | Runs offline in < 2 min. A change in request shape or output bytes shows a reviewable diff | INF-002, INF-003 | L |
| TST-API-004 | must | Golden refresh and drift job: nightly `IT_RECORD=1` against all images, then a normalised diff against the committed goldens; new upstream patch releases are tested by bumping one tag | Unchanged versions produce a zero diff; a drift report is an artefact | `scripts/golden/drift.sh` | S |
| TST-API-010 | must | Scroll: initial/continuation/clear on every version; sliced scroll (`slices` = shards, > shards, > `max_slices_per_scroll`); `scroll_id` rotation; clear returns 404 after expiry | Digest equal to source; no open contexts left (`_nodes/stats/indices/search.scroll_current == 0`) | live + golden | M |
| TST-API-011 | must | PIT: open/close request shape per engine; `pit_id` rotation each page; tiebreaker per version (ES ≥ 7.12 `_shard_doc`; ES 7.10/7.11 and OpenSearch `_doc` + `_id`); fallback to scroll on ES OSS and OS < 2.4 with a warning; sliced PIT → sliced scroll on ES < 7.15 | Digest equal for unsliced and slices ∈ {2, 4, 16}; no open PITs afterwards (`point_in_time_current == 0` where the stat exists) | live + golden | M |
| TST-API-012 | must | `search_after` without PIT: sort required (usage error otherwise); ties on sort values with deterministic data (1,000 docs sharing one timestamp) | Documents a known limitation with an asserted count, or digest equal if the tool adds an `_id` tiebreaker | live | S |
| TST-API-013 | must | `hits.total` variants: int (`rest_total_hits_as_int=true`, ES 6 goldens), `{value, relation: eq\|gte}`, missing; `track_total_hits=false` in a user `searchBody` | Progress estimate and final reconciliation (BRK-SRV-009) behave correctly in each case | golden + unit | S |
| TST-API-014 | should | `_render/template` with inline and stored (`id`) templates on every version that supports mustache | Same documents as the equivalent plain query | live | S |
| TST-API-015 | must | `searchBody` variants: `_source` includes/excludes, `stored_fields`, `docvalue_fields`, `fields` (meta extraction from `fields._routing`), `size` override by `--limit`, `sort` given by the user, `query` with `slice` already present | Output matches the golden for each variant; meta extraction order matches Node (`X`, `_X`, `fields.X`, `fields._X`) | golden | M |
| TST-API-020 | must | `_bulk` request shape per version and action (`index`, `create`, `update`, `delete`) with routing, version, versionType, `--parseExtraFields`, typed output (ES 6 goldens only), gzip body (`--esCompress`), `--maxBulkBytes` splits | Byte-exact NDJSON goldens; every version accepts the gzip body (or the failure is documented per version) | golden + live | M |
| TST-API-021 | must | `_bulk` response parsing per version: `errors:false` fast path; mixed items; `error.type` values (`version_conflict_engine_exception`, `mapper_parsing_exception`, `es_rejected_execution_exception`, `cluster_block_exception`, `document_missing_exception`); 413 for oversize; item count mismatch | `Total Writes`/`Failed Writes` equal the server's truth (cross-checked by `_count`) | golden + live | M |
| TST-API-022 | should | Refresh behaviour: per-batch `_refresh` unless `--noRefresh`; refresh failure is only a warning | Asserted request counts; visible-doc timing documented | golden | S |
| TST-API-030 | must | Mappings: typeless on 7+/OpenSearch; typed 6.x file unwrap; field literally named `properties`; runtime fields (ES ≥ 7.11); `flattened` (ES) vs `flat_object` (OS ≥ 2.7); `dense_vector`/`knn_vector`; `_source: {enabled:false}` and `mode: synthetic`; `subobjects:false` | Same-engine round trips are lossless; cross-engine either converts with a named warning or fails with a clear error (expectation table, API-071) | live + golden | M |
| TST-API-031 | must | Settings: generated keys stripped (`uuid`, `creation_date`, `version.*`, `provided_name`, `history.uuid`, `resize.*`); engine-only keys per direction (README list); `--excludeSettings`; analysis blocks | The restored `_settings` equal the source modulo an allow-list asserted per version pair | live | M |
| TST-API-032 | must | `--type index` with hidden and system indices (`expand_wildcards`), closed indices, data-stream backing indices (`.ds-*`, skipped with a warning), aliases with filter/routing | Expected inclusion list per version | live | M |
| TST-API-033 | must | Analyzer type: new index; existing index with error, `--skip-existing`, or `--closeIndex` (close → update → reopen, also on failure) on every version | The index is never left closed (asserted after an injected failure) | live + contract | S |
| TST-API-034 | must | Aliases: `is_write_index`, `is_hidden`, `filter`, `index_routing`/`search_routing`; atomic `POST /_aliases`; host-only dumps | Round trip equal per version | live | S |
| TST-API-035 | must | Legacy `_template` on all versions incl. Warning headers on ES ≥ 8 (logged once, not fatal) and whatever ES 9.x does (pinned by golden) | Golden + live | live + golden | S |
| TST-API-036 | must | Component and index templates: `composed_of` ordering; `priority`; `data_stream`; per-version fields stripped across engines; `--filterManaged` against **each version's built-in list** (snapshot the built-in names per version into the goldens) | A new built-in in a future version is caught by the drift job | live + golden | M |
| TST-API-037 | must | ILM (ES 7.10 default+, missing on OSS → clear error) and ISM (OS 1.3/2.19/3.x): paging past 20 and past 1000 policies; `if_seq_no`/`if_primary_term` overwrite; second 409 reported; shape differences (`ism_template` array vs object, `user`, `schema_version`, `last_updated_time`) | Round trip equal; 1,050-policy paging test | live | M |
| TST-API-038 | should | `_opendistro/_ism` vs `_plugins/_ism`: on OS 1.3 verify that policies created via the legacy path are visible via `_plugins` and that the tool never sends `_opendistro`. Decide whether Open Distro 1.13 (ES 7.10-based) is in scope (open question Q7) | Asserted on OS 1.3 | live | S |
| TST-API-039 | must | Scripts: `_cluster/state/metadata` with and without the `monitor` privilege (secure profiles: a dedicated low-privilege user); painless and mustache; an empty `stored_scripts` | A 403 gives a message naming the `monitor` privilege | live (secure) | S |
| TST-API-040 | must | Cluster info variants (golden): OpenSearch with `compatibility.override_main_response_version=true` (reports 7.10.2; see BRK-XV-001); missing `distribution`; `build_flavor: oss`; a non-JSON body (proxy page); ES product header absent | Engine detection is correct, or the run fails with a clear message. Silently falling back to "Elastic 5.0.0" is a failure | golden + live (OS setting) | S |
| TST-API-041 | should | Deprecation `Warning` headers and `X-Elastic-Product` are tolerated and logged at most once per kind | Golden | golden | S |
| TST-API-050 | must | 429 handling per request kind: initial search, scroll continuation (retry allowed only on 429), PIT search, whole `_bulk`, bulk items (429/503 re-sent individually), meta PUTs; delays follow `--retryDelay`/`--retryBackoff`; decide on honouring `Retry-After` | Exact attempt counts and delays under `tokio::time::pause` (mock) | router mock | M |
| TST-API-051 | must | 5xx (500/502/503/504) per request kind; scroll continuations are **not** retried on 5xx (by design) and the error must say how to resume (`--scrollId`) | Exit 1, staging removed, clear message | router mock | S |
| TST-API-052 | must | Partial results: `_shards.failed > 0`, `timed_out: true`, `_shards.skipped`, failures on one slice only | Fails; nothing committed; the message names the shard reason | golden + mock | S |
| TST-API-053 | must | Version conflicts: `create` on existing ids; `--handleVersion --versionType external` with a lower version; ISM 409 handling | Counts per item type are correct; exit code follows the ignore flags (Q3) | live | S |
| TST-API-054 | must | Auth failures: no credentials, wrong credentials, expired/locked user, missing index or cluster privileges per endpoint (read, write, `manage_ilm`, `manage_index_templates`, `monitor`); separate input and output credentials | One clear message per case naming the endpoint and privilege; no secret in output (STD-ACC 5) | secure profiles + extra users | M |
| TST-API-055 | must | TLS failures: unknown CA, expired cert, not-yet-valid cert, hostname mismatch, IP SAN, client cert required but missing, wrong `--pass`, legacy encrypted PEM key, `https://` to a plain port, `http://` to a TLS port, TLS 1.0/1.1-only server | One clear message per case; exit 1; `--insecure` and `NODE_TLS_REJECT_UNAUTHORIZED=0` print a warning | cert factory script (openssl) + secure profiles + mock TLS server | M |
| TST-API-056 | should | Server error passthrough: `index_not_found`, `resource_already_exists`, `illegal_argument` (mapping conflict), `mapper_parsing` | The message includes the server's `type` and `reason`, truncated to ≤ 2 KB | golden | S |
| TST-API-060 | must | **Node interop for every type** (`data`, `mapping`, `settings`, `index`, `analyzer`, `alias`, `template`, `component_template`, `index_template`, `policy`, `script`) × engine family (ES, OS): Node dump → Rust restore and Rust dump → Node restore. Byte compare where the format is a string literal; semantic compare otherwise. **Commit the Node-produced dumps as golden fixtures** so the test runs without Node; regenerate nightly when Node is present | 22 directions × 2 engines green; fixtures tracked (INF-014) | `tests/fixtures/node/<engine>/<type>.json` | M |
| TST-API-061 | must | Compression interop: Node gzip parts → Rust; Rust gzip parts → Node; Rust zstd → Node is refused (documented) | Asserted per case | live + Node | S |
| TST-API-062 | must | Split naming interop including the non-ASCII boundary difference (UTF-8 bytes vs UTF-16 units) | Part names equal for ASCII; documented difference asserted for non-ASCII | Node | S |
| TST-API-063 | must | Data file variants accepted: `--sourceOnly` files; `_type`; `fields`; `_routing`/`routing`/`fields._routing`; `--format human` pretty files; concatenated JSON; CRLF; BOM | Each restores to the same digest | fixtures | S |
| TST-API-064 | should | Node's `bigint*.json` fixtures and `--support-big-int` outputs round-trip byte for byte | Byte equal | fixtures | S |
| TST-API-070 | must | **Cross-version matrix, "export once, import everywhere":** each of the 9 images (plain) exports a standard fixture set (§3.3) to zstd files once per nightly run. Every other image then imports every export, one container at a time. 9 × 8 = 72 imports, each covering 11 types | Results match the expectation table (API-071) exactly | extend `cross.rs` + `IT_CROSS_DIR` per version | L |
| TST-API-071 | must | **Committed expectation table** `tests/golden/cross-expectations.toml`: for each (source version, destination version, type) one of `pass`, `pass-with-warnings[list]`, `refused[message substring]`, `needs-flag[flag]` (e.g. `--convertIlmToIsm`, `--excludeSettings=`) | A diff between actual and expected fails CI; changes need a review | toml + asserter | M |
| TST-API-072 | should | **Live cluster-to-cluster pairs** (two nodes at 256m heaps, when memory allows): ES 7.10.2 → OS 1.3.20 (the classic migration); ES 7.17 → OS 2.19; ES 8.19 → OS 3.x; ES 9.1 → OS 3.x; OS 2.19 → ES 8.19; OS 3.x → ES 9.1; ES 7.17 → ES 9.1 (skip-major); OS 1.3 → OS 3.x; downgrades ES 9.1 → ES 8.19 and OS 3.x → OS 2.19 | Digest equal for data; metadata per the expectation table | `run.sh cross PAIR=` | M |
| TST-API-080 | must | **proptest: NDJSON splitter** (`pipeline/json_split.rs::ValueSplitter`): any sequence of JSON values, serialised with random whitespace, CRLF, BOM, pretty printing, commas between values, and random chunk boundaries (1 byte to 1 MB), splits into exactly those values; `single_line` keeps the bytes of every string | 10k cases in T0; 1M nightly | dev-dep `proptest` | M |
| TST-API-081 | must | **proptest: double-encoded meta codec** (`meta/codec.rs::encode_line`/`decode_any`): arbitrary objects including unicode, escapes, `\u0000`, lone surrogates, big integers, deep nesting up to 127 levels; compared against Node `JSON.stringify(JSON.stringify(x))` on a corpus | Round trip equal; byte equal with Node on the corpus (or a documented difference) | proptest + Node corpus fixture | M |
| TST-API-082 | must | **proptest/fuzz: URL parsing** (`endpoint::parse`, `redacted_url`, `redacted_endpoint`, `split_index_type`): credentials containing `@ : % # / ?` and unicode, IPv6 hosts, ports, base paths, `%2F` in index names, `$`, file paths that look like URLs, `_cat`/`_search` paths | Property: no redacted string ever contains the password; parse never panics | proptest + cargo-fuzz | M |
| TST-API-083 | must | **proptest: rename rules** (`rename.rs`): first-match semantics, `--renamePrefix` before `--renameIndex`, captures; `validate_index_name` agrees with a live ES/OS oracle on 5,000 generated names (nightly) | 0 disagreements with the oracle | proptest + live oracle | M |
| TST-API-084 | must | **cargo-fuzz targets:** `json_split`, `codec_decode`, `bulk_encode_doc` (meta extraction), `extract_batch` (search response), `bulk_handle_response`, `parse_size`, `endpoint_parse`, `ilm_to_ism_convert`, `renamer_new_index`, `ini_auth_file`. Corpora are seeded from the goldens | 60 s per target on PR, 1 h nightly; crashes are minimised and committed as regression unit tests | `fuzz/` directory (needs `pub` test hooks or `#[cfg(fuzzing)]` entry points) | L |
| TST-API-085 | should | Differential bulk encoding against Node: for 10k random docs, Rust `encode_doc` output equals the NDJSON Node sends (captured once through the recorder) | Byte equal, or a documented difference | Node + recorder | M |
| TST-API-086 | should | **ILM→ISM property oracle:** generate random valid ILM policies (phases × actions × `min_age` units); the converter either refuses and names the action, or the converted policy is accepted by OS 1.3, 2.19 and 3.x (`PUT` nightly) | 0 accepted-by-converter-but-rejected-by-OpenSearch cases | proptest generator + live | M |

### 3.3 Standard fixture set for cross-version exports

* One index per mapping family:
  * `fx-logs`: keyword, text, date, ip, long, `geo_point`; 2 shards.
  * `fx-nested`: nested and object.
  * `fx-wide`: 2,000 fields.
  * `fx-edge`: §5.4 edge content and routing.
* Templates:
  * one legacy template;
  * two component templates plus one index template that composes them;
  * one data-stream template on ES ≥ 7.9 and OS ≥ 1.0.
* Aliases: a write alias and a filtered alias.
* An ILM or ISM policy (engine-native) and an analyzer index.
* One painless and one mustache stored script.
* Each export also stores `expected.json`: count, digest, and the normalised metadata.

### 3.4 Pair coverage summary

| Source ↓ / Destination → | ES 7.10 | ES 7.17 | ES 8.6 | ES 8.19 | ES 9.1 | OS 1.3 | OS 2.19 | OS 3.x |
|---|---|---|---|---|---|---|---|---|
| ES 7.10 (OSS/default) | same | F | F | F | F | **F+L** | F | F |
| ES 7.17 | F (down) | same | F | F | **F+L** | F | **F+L** | F |
| ES 8.6 | F (down) | F (down) | same | F | F | F | F | F |
| ES 8.19 | F (down) | F (down) | F (down) | same | F | F | F | **F+L** |
| ES 9.1 | F (down) | F (down) | F (down) | **F+L** (down) | same | F | F | **F+L** |
| OS 1.3 | F | **F+L** | F | F | F | same | F | **F+L** |
| OS 2.19 | F | F | F | **F+L** | F | F (down) | same | F |
| OS 3.x | F | F | F | F | **F+L** | F (down) | **F+L** (down) | same |

F = file leg (nightly, TST-API-070). L = live cluster-to-cluster leg (TST-API-072). "same" = covered by each profile's own suite.
Downgrade legs are expected to need `--excludeSettings`/`--excludeTemplateFields` for metadata; data must still be digest-equal.

---
## 4. Data-load testing (TST-LOAD)

### 4.1 Tiers

| Tier | Docs | Raw size | Primary data | Where | Budget |
|---|---|---|---|---|---|
| **L0 smoke** | 100k (+ 10k edge docs) | ~50–100 MB | `edgen --profile logs --seed 1` + `edgen --profile edge` | PR CI and the box | < 10 min, 1 node at 1 GB heap |
| **L1 nightly** | 10M | 1.3–10 GB | Rally/OSB `http_logs` `documents-191998` (9.7M docs, 1.30 GB) **and** `edgen --profile logs` 10M at ~1 KB/doc (~10 GB); variety runs on `geonames` 11.4M, `nested` 11.2M, `pmc` 574k large docs, `eventdata` 20M | box (with guards) or a 4 vCPU/16 GB CI runner with ≥ 40 GB disk | < 2 h, 1 node at 2–4 GB heap |
| **L2 soak/scale** | 100M–250M | 30–110 GB | OSB `big5` 100 GB corpus (116M docs, 107 GB) or the full `http_logs` (247M docs, 33.4 GB); `nyc_taxis` (165M docs, 79.8 GB); `edgen` 100M | big runner only (≥ 16 vCPU, 64 GB, 1 TB NVMe) | 6–12 h |
| L3 (not planned) | 1B+ | 0.9–1 TB | `big5` 880/1000 GiB | dedicated hardware | — |

### 4.2 Requirements

| ID | Pri | Requirement | Acceptance criteria | Harness change | Effort |
|---|---|---|---|---|---|
| TST-LOAD-001 | must | Load runner `scripts/load/run.sh <tier> <scenario>`: provisions the node with caps, seeds from the cache or `edgen`, runs the scenario under `measure.sh` (INF-006), verifies with `edverify`, tears down | One command per tier; results JSON per scenario; seeds are recorded | runner + scenario files (`scenarios/*.toml`) | M |
| TST-LOAD-002 | must | **Correctness on every load run:** `count` and digest equal (source index vs dump file(s) vs destination index). Also 1,000 random `_id`s byte-compared with the source; `Failed Writes` = 0 unless the scenario expects loss | Any mismatch fails and names the first 10 differing ids | `edverify` | S |
| TST-LOAD-003 | must | **Metrics per run:** wall time; docs/s; MB/s (in and out); CPU-seconds (user + sys); peak RSS; retries by kind (parsed from `retrying` warnings); `Failed Writes`; open FDs (peak); output bytes and compression ratio; cluster-side indexing/search rate and rejected count (`_nodes/stats/thread_pool`); open scroll/PIT contexts afterwards | All fields present in results JSON; schema-validated | `measure.sh` | S |
| TST-LOAD-004 | must | **Scenario matrix** (each tier runs a documented subset): dump scroll; dump PIT unsliced and slices ∈ {2, 4, 8, 16}; restore at concurrency ∈ {1, 4, 8} and `--limit` ∈ {1000, 5000}; cluster→cluster copy (memory permitting); `--fsCompress`; `--compressionType zstd` (levels 1/3/9); `--fileSize`, `--maxRows`; `--sourceOnly`; `--searchBody` range filter | Matrix defined in `scenarios/*.toml`; each cell has a correctness gate and a perf baseline | scenarios | M |
| TST-LOAD-005 | must | **Memory is bounded and flat:** peak RSS at 10M docs is within 10% of peak RSS at 100k for the same flags; the README formula `(bufferSize + slices + concurrency) × limit × avg_doc × 1.5 + 10 MB` holds within +25% for avg doc 200 B, 1 KB, 40 KB (pmc) and 1 MB (edgen `huge`) | Asserted per run; the formula is updated if it is wrong | `measure.sh` | M |
| TST-LOAD-006 | must | **Resumability under load:** (a) dump `kill -9` at 10 random points → final path absent, staging leftovers listed (BRK-SIG-003), rerun with `--overwrite` gives the full digest; (b) restore `kill -9` then rerun the same file (explicit ids) → digest equal; (c) restore resume with `--offset N`, N taken from the last `Total Writes`; (d) scroll resume with `--scrollId` while the context is alive | Digest equal in (a)–(d); the documented resume recipe is tested verbatim | runner hooks (`KILL_AT_DOCS=`) | M |
| TST-LOAD-007 | must | **Failure injection under load:** fault proxy cutting 1 in 50 connections; a 429 storm from `thread_pool.write.queue_size: 1` + `size: 1`; disk-watermark flood-stage block for 30 s; `podman restart` of the node mid-run | Run completes with digest equal, **or** exits non-zero with a clear message; never exit 0 with loss (see BRK-SRV-001) | runner + fault proxy | M |
| TST-LOAD-008 | must | **Compression at scale:** gzip levels 1/6/9 and zstd 1/3/9/19 on 10M docs; multi-GB single files; `cat` of parts readable; throughput per core vs the README claims | Results recorded; README numbers within ±20% or updated | scenarios | S |
| TST-LOAD-009 | must | **Splitting at scale:** 10M docs with `--fileSize 256mb` and `--maxRows 1000000`. Assertions: part count is as expected; no document spans parts; each part ≤ limit + 1 doc; restoring all parts gives the full digest; the staged parts are committed together (BRK-RES-002) | Asserted | scenarios | S |
| TST-LOAD-010 | must | **Concurrency scaling curves:** dump slices 1–16 with workers = slices; restore concurrency 1–16; record docs/s and CPU-s; compare with the previous nightly | Regression > 10% on any point fails nightly | trend store (LOAD-020) | M |
| TST-LOAD-011 | must | **Node comparison** at L0 and L1 (Node only up to 10M docs because of runtime): same flags for both tools; Node output digest equals Rust output digest (data); time and CPU ratios checked against PLAN.md §8 targets | Ratios recorded; digest equal | extend `quick-vs-node.py` | M |
| TST-LOAD-012 | must | **Resource limits by place:** box = 1 node, heap ≤ 4 GB, container `--memory 6g`, tool ≤ 1 GB, ≤ 25 GB disk incl. cache, L2 forbidden; standard 4-vCPU/16-GB/~14-GB-disk GitHub runner = L0 plus a reduced L1 (5M docs, zstd only, no cache) [runner disk figure to confirm]; big runner = L1 + L2 | The runner refuses tiers that do not fit (INF-007) | guards | S |
| TST-LOAD-013 | should | Cross-engine at scale: ES 8.19 → OS 2.19 and ES 9.1 → OS 3.x with 10M docs via file and live | Digest equal | runner | S |
| TST-LOAD-014 | should | Many indices and many fields: 1,000 small indices (wildcard dump of `data`, `mapping`, `settings`, `index`, `alias`); a 5,000-field mapping (`index.mapping.total_fields.limit` raised); 100 aliases | Completes; meta types finish in under N minutes (baseline) | edgen profiles | M |
| TST-LOAD-015 | should | Large documents: pmc corpus (~40 KB avg); edgen `huge` (1–10 MB docs); one 90 MB doc near `http.max_content_length` (100 MB) with `--maxBulkBytes` | Digest equal; 413 handled with a clear error when exceeded | edgen | S |
| TST-LOAD-016 | should | **Soak:** 6–12 h of repeated dump/restore/copy cycles on L2 data; watch RSS drift, FD count, thread count, server-side open contexts, staging leftovers, disk usage | RSS drift < 5% per hour after warm-up; 0 leaked contexts and FDs | soak runner | M |
| TST-LOAD-017 | must | **Server-side leak check after every load and IT run:** `_nodes/stats/indices/search` `scroll_current` = 0 and `point_in_time_current` = 0 (where the stat exists); no hidden staging indices; no `.part-` files | Asserted in the teardown of every load/IT run | harness teardown | S |
| TST-LOAD-018 | should | Data streams at scale: restore 1M docs into a data stream (requires `--bulkAction create`), and dump a data stream (`.ds-*` backing indices in `_index`) | Digest equal; behaviour documented (BRK-XV-003) | runner | S |
| TST-LOAD-019 | could | Cluster-bound baseline: Rally/OSB ingest of the same corpus on the same node, to show how close the tool gets to the cluster's own limit | Recorded ratio | Rally/OSB optional | S |
| TST-LOAD-020 | should | Trend store and regression gates: results JSON appended to a `bench-results` branch or an artefact series; plots per scenario | Nightly comment-free report artefact; > 10% regression fails | script | M |

---

## 5. Data sources and generators (TST-DATA)

### 5.1 Public datasets (links verified 2026-10-29 with `curl -I -L`; HTTP status in brackets)

Sizes come from each track's `track.json`/`workload.json` (`document-count`, `uncompressed-bytes`) or from HEAD `Content-Length`.

| Source | What it is | Size | Format | License | How to load | Mapping features | Tier fit |
|---|---|---|---|---|---|---|---|
| **Rally `http_logs`** — [track](https://github.com/elastic/rally-tracks/tree/master/http_logs) [200]; corpus e.g. [documents-191998.json.bz2](https://rally-tracks.elastic.co/http_logs/documents-191998.json.bz2) [200, 49.5 MB] (also `-181998` [200, 13.8 MB], `-241998` [200, 907 MB]); 1k samples like [documents-181998-1k.json.bz2](https://rally-tracks.elastic.co/http_logs/documents-181998-1k.json.bz2) [200] | 1998 World Cup web server logs, parsed (`@timestamp`, `clientip`, `request`, `status`, `size`) or unparsed (`message` only) | 7 files, 247,249,096 docs total, 33.4 GB parsed (1.24 GB bz2); `-181998` 2.7M/364 MB, `-191998` 9.7M/1.30 GB, `-241998` 181M/24.6 GB | NDJSON source docs (one `_source` per line, no action lines), bz2 | Same terms as the source (ITA WorldCup: "use and redistribute freely") | `rally race --track=http_logs --pipeline=benchmark-only --target-hosts=…` or `bzcat | ndjson2bulk | curl _bulk` / **`elasticdump-rs --input=file`** after the converter adds `_index`/`_id` | date, ip, keyword, integer, text | L0 (1k/181998), **L1 (191998)**, L2 (all) |
| **OSB `http_logs`** — [workload](https://github.com/opensearch-project/opensearch-benchmark-workloads/tree/main/http_logs) [200]; zstd mirror e.g. [documents-181998.json.zst](https://dbyiw3u3rf9yr.cloudfront.net/corpora/http_logs/documents-181998.json.zst) [200, 16.1 MB] | Same corpus, zstd and bz2 | as above | NDJSON, zst/bz2 | as above | `opensearch-benchmark execute-test --workload=http_logs --pipeline=benchmark-only …`, or decompress and convert | as above | as above (zstd is faster to decompress) |
| **Rally/OSB `nyc_taxis`** — [Rally](https://github.com/elastic/rally-tracks/tree/master/nyc_taxis) [200], [documents.json.bz2](https://rally-tracks.elastic.co/nyc_taxis/documents.json.bz2) [200, 4.82 GB]; [OSB zst](https://dbyiw3u3rf9yr.cloudfront.net/corpora/nyc_taxis/documents.json.zst) [200, 4.81 GB] | 2015 NYC yellow-cab trips | 165,346,692 docs, 79.8 GB | NDJSON | Public domain (NYC Open Data Law, per track README) | Rally/OSB or convert | `geo_point`, `scaled_float`, dates, keyword, many numerics | L2 |
| **OSB `big5`** — [workload](https://github.com/opensearch-project/opensearch-benchmark-workloads/tree/main/big5) [200]; [documents-60.json.zst](https://dbyiw3u3rf9yr.cloudfront.net/corpora/big5/documents-60.json.zst) [200, 4.31 GB], [documents-100.json.zst](https://dbyiw3u3rf9yr.cloudfront.net/corpora/big5/documents-100.json.zst) [200, 7.31 GB] (880/1000 GiB corpora also listed) | Modern ECS-style log events (`@timestamp`, `message`, `log.file.path`, `process.name`, `agent.*`, `cloud.*`, `aws.cloudwatch.*`, `metrics.*`, `event.*`) | 60 GB: 69.2M docs (64.0 GB raw); 100 GB: 116M docs (107 GB); 880 GiB: 1.02B; 1000 GiB: 1.16B | NDJSON zst/bz2 | Apache-2.0 (workload `LICENSE.txt`) | OSB `--workload=big5 --workload-params corpus_size:60`, or `zstdcat` + convert | deep objects, keyword-heavy, dates, text | **L2** (60/100) |
| **OSB `eventdata`** — [workload](https://github.com/opensearch-project/opensearch-benchmark-workloads/tree/main/eventdata) [200]; [eventdata.json.bz2](https://dbyiw3u3rf9yr.cloudfront.net/corpora/eventdata/eventdata.json.bz2) [200, 793 MB] | 20M Apache access-log events from the Rally eventdata generator (stats from elastic.co logs) | 20,000,000 docs, 16.4 GB | NDJSON bz2 | Apache-2.0 (workload README) | OSB or convert | geo, user agent objects, ip, keyword, text | **L1** (good 10–20M size) |
| **Rally `eventdata` track** — [rally-eventdata-track](https://github.com/elastic/rally-eventdata-track) [200] | Python generator inside Rally producing `elasticlogs` events; a 1B-event challenge yields ~200 GB on disk | unbounded | generated on the fly by Rally | repo has no licence file (GitHub API: none) → ask Elastic before redistributing outputs | `rally race --track-repository=eventdata …` | as above | L2 (CPU-heavy generator) |
| **Rally `elastic/logs`** — [track](https://github.com/elastic/rally-tracks/tree/master/elastic/logs) [200]; e.g. [nginx.access document-0.json.zst](https://rally-tracks.elastic.co/observability/logging/nginx/demo.elastic.co/nginx.access/raw/document-0.json.zst) [200, 10.4 MB] | Raw ECS log events per integration (kafka, nginx access/error, mysql error/slowlog, system auth/syslog, k8s app logs); files of ~1 GB decompressed each (~280k–880k docs per file) | 13 corpora, several files each (`document-{i}.json.zst`) | NDJSON zst (raw `message` + `data_stream` fields; the track replays them through ingest pipelines) | Apache-2.0 (track README) | Use the raw files as NDJSON sources; restore into data streams (`--bulkAction create`) | data streams, ECS, dates, text | L1/L2 data-stream tests |
| **Rally `geonames`** — [track](https://github.com/elastic/rally-tracks/tree/master/geonames) [200]; [documents-2.json.bz2](https://rally-tracks.elastic.co/geonames/documents-2.json.bz2) [200, 265 MB] | GeoNames places | 11,396,503 docs, 3.55 GB | NDJSON | CC-BY-3.0 | Rally or convert | `geo_point`, many fields, unicode names (many scripts) | **L1 variety + unicode** |
| **Rally `nested`** — [track](https://github.com/elastic/rally-tracks/tree/master/nested) [200]; [documents.json.bz2](https://rally-tracks.elastic.co/nested/documents.json.bz2) [200, 696 MB] | StackOverflow Q&A with nested answers | 11,203,029 docs, 3.64 GB | NDJSON | CC-BY-SA-3.0 | Rally or convert | **nested**, dates, text | L1 variety |
| **Rally `pmc`** — [track](https://github.com/elastic/rally-tracks/tree/master/pmc) [200]; [documents.json.bz2](https://rally-tracks.elastic.co/pmc/documents.json.bz2) [200, 5.93 GB] | PubMed Central full-text articles | 574,199 docs, 23.3 GB (**~40 KB avg**) | NDJSON | CC-BY-2.0 | Rally or convert | large text docs | **L1 large docs** |
| **Rally `noaa`** — [track](https://github.com/elastic/rally-tracks/tree/master/noaa) [200]; [documents.json.bz2](https://rally-tracks.elastic.co/noaa/documents.json.bz2) [200, 995 MB] | NOAA weather observations | 33,659,481 docs, 9.68 GB | NDJSON | US Government Work | Rally or convert | numerics, `geo_point`, dates | L1/L2 |
| **Loghub** — [GitHub](https://github.com/logpai/loghub) [200]; full data on [Zenodo record 8196385](https://zenodo.org/records/8196385) (API [200]; the HTML page returned 403 to curl, use the API or a browser) | 19 raw system-log collections: HDFS, Hadoop, Spark, BGL, HPC, Thunderbird, Windows, Linux, Mac, Android, Apache, OpenSSH, OpenStack, Zookeeper, HealthApp, Proxifier | Archives: Thunderbird 2.02 GB, Windows 1.67 GB, HDFS_v2 824 MB, HDFS_v3 567 MB, Android_v2 445 MB, HDFS_v1 187 MB, Spark 183 MB, BGL 57 MB, … (raw text is ~10× larger, tens of GB in total [per Loghub paper, not re-measured]) | raw text lines (one event per line) | Zenodo: **CC-BY-4.0**; repo README: "freely available for research or academic work" → **confirm commercial/test use (Q9)** | Convert lines → `{"@timestamp","host","component","level","message","source":"loghub/<set>"}` NDJSON (timestamp formats differ per set) | real-world messy text, odd bytes, long lines | L1 (BGL, HDFS_v1, Spark), L2 (Thunderbird, Windows) |
| **Loghub-2.0** — [GitHub](https://github.com/logpai/loghub-2.0) [200] | Annotated, parser-benchmark subsets of Loghub | small | text + templates | as Loghub | convert | — | L0 realism |
| **NASA HTTP (ITA)** — [page](https://ita.ee.lbl.gov/html/contrib/NASA-HTTP.html) [200]; [NASA_access_log_Jul95.gz](https://ita.ee.lbl.gov/traces/NASA_access_log_Jul95.gz) [200, 20.7 MB], [Aug95](https://ita.ee.lbl.gov/traces/NASA_access_log_Aug95.gz) [200, 16.6 MB] (`ftp://` links on the page did not answer; use `https://…/traces/`) | Two months of NASA KSC web logs (Jul and Aug 1995) | 3,461,612 requests total | Common Log Format text | "may be freely redistributed" | parse CLF → NDJSON | date, ip-ish host, keyword, int | L0/L1-lite |
| **WorldCup98 (ITA)** — [page](https://ita.ee.lbl.gov/html/contrib/WorldCup.html) [200]; [traces dir](https://ita.ee.lbl.gov/traces/WorldCup/) [200] (249 `wc_day*.gz` files, e.g. `wc_day46_1.gz` 45 MB) | Raw source of `http_logs`: all 1,352,804,107 requests to the 1998 World Cup site | 1.35B requests | **custom binary** format; needs the ITA tools (`recreate`) to convert | "use and redistribute these access logs freely" | prefer Rally `http_logs` (already converted) | — | L3 only |
| **SecRepo** — [site](https://www.secrepo.com/) [200]; [self.logs](https://www.secrepo.com/self.logs/) [200] (e.g. `access.log.2017-01-01.gz` [200, 17 KB]) | Security datasets: daily web access logs 2015–2017, Bro/Zeek, Snort, etc. | small per file (KBs–MBs) | CLF text, Zeek TSV | CC-BY-4.0 | parse → NDJSON | ip, user agents, odd requests (attack strings) | L0 edge content |
| **Kaggle "Web Server Access Logs"** (Zanbil) — [page](https://www.kaggle.com/datasets/eliasdabbas/web-server-access-logs) [200] | Nginx/Apache combined log from an Iranian e-commerce site | 3.3 GB, ~10M lines [line count per dataset page, not re-measured] | combined log text | CC0 (per page metadata) | needs a Kaggle account/API token to download; parse → NDJSON | non-ASCII URLs, user agents | L1 (needs login, Q9) |
| **elastic/examples apache_logs** — [file](https://raw.githubusercontent.com/elastic/examples/master/Common%20Data%20Formats/apache_logs/apache_logs) [200, 2.37 MB] | 10k Apache combined lines (repo archived) | 10,000 lines | text | Apache-2.0 | parse → NDJSON | — | L0 |
| **AIT Log Data Set V2.0** — [Zenodo 5789064](https://zenodo.org/records/5789064) (API [200]) | Multi-host attack-scenario logs (apache, auth, dns, suricata JSON, audit…) | 8 zips, 7.1–26.5 GB each | mixed text + JSON | **CC-BY-NC-SA-4.0** (non-commercial) | convert per source | heterogeneous | L2 optional (licence, Q9) |

**Excluded after checking:**
* OSB `elb_logs` corpus (`d270n0h488ttok.cloudfront.net/.../elb_logs/documents.json.bz2`) returned **403**.
* The NASA `ftp://` links did not respond.
* A guessed SecRepo file name returned 404; real names are listed in the directory index.

### 5.2 Generators

| Generator | What | Reproducible? | License | Fit |
|---|---|---|---|---|
| **flog** — [mingrammer/flog](https://github.com/mingrammer/flog) [200] | Go CLI: `apache_common`, `apache_combined`, `apache_error`, `rfc3164`, `rfc5424`, `common_log`, `json`; `-n` lines or `-b` bytes, `-l` loop, `-p` split | **No seed flag** → not byte-reproducible | MIT | L0/L1 volume and text realism; not for golden digests (digest the generated file instead) |
| **elastic-integration-corpus-generator-tool** — [repo](https://github.com/elastic/elastic-integration-corpus-generator-tool) [200] | Go CLI: `generate <package> <dataset> <version> -t N` (fields from the Elastic package registry) or `generate-with-template` (Go text templates + `fields.yml` + `configs.yml` with cardinality) | **Yes:** `--seed/-s` (default 1) and `--now` | **Elastic License 2.0** (fine for internal testing; not OSI) | L1/L2 realistic ECS logs for data-stream tests; `generate` needs network access to `epr.elastic.co` |
| **OSB `generate-data`** — [docs](https://docs.opensearch.org/latest/benchmark/reference/commands/generate-data/) [200] | OpenSearch Benchmark ≥ 2.0: synthetic corpora from an **index mapping** (`--index-mappings`) or a Python module (`--custom-module`, Mimesis providers), `--total-size` in GB, Dask workers | seed support not documented [verify] | Apache-2.0 | **Mapping-driven variety:** feed it our fixture mappings (§3.3) |
| **Rally eventdata generator** (above) | Python, inside Rally | seeded per track params [verify] | no licence file | L2 |
| **makelogs** — [repo](https://github.com/elastic/makelogs) [200]; npm `makelogs` latest 4.3.0 (registry [200]) | Node script that bulk-loads fake HTTP logs into `logstash-YYYY.MM.DD` indices; README: "designed for … latest builds … not compatible with previous versions", "do not use … on any production" | not documented | Apache-2.0 | could: Kibana-style indices; may not work against OpenSearch (legacy client product check) |
| **Logstash `generator` input** — [docs](https://www.elastic.co/docs/reference/logstash/plugins/plugins-inputs-generator) [200] | Emits fixed or listed lines `count` times | deterministic but low variety | Elastic (part of Logstash) | could: pure throughput only |
| **Faker** (Python) — [joke2k/faker](https://github.com/joke2k/faker) [200], [PyPI](https://pypi.org/project/Faker/) [200]; **`fake`** crate (Rust) — crates.io API [200], v5.1.0 | Fake-data libraries with seedable RNGs | **Yes** (seed) | MIT; `fake`: MIT OR Apache-2.0 | Building blocks for our own generator |

### 5.3 Recommended approach: our own seeded generator `edgen` (in-repo, Rust)

Rationale:
* External corpora fix the content, and most external generators cannot be seeded.
* We need **byte-reproducible** data at any size, with chosen mappings and planted edge cases.
* We need streaming output that never holds the corpus in memory.
* The output must be exactly what `edverify` digests.

Design:
* **Determinism:**
  * `ChaCha20Rng` seeded from `(seed, profile, doc_index)` (counter-based), so doc *i* can be generated independently. This gives parallel generation and lets `--start N --count M` produce any shard of the corpus.
  * Output is identical across runs, platforms and thread counts. A CI test checks that `sha256(edgen --seed 1 --count 100000)` equals a committed value.
* **Output modes:**
  * `--format source` (one `_source` per line);
  * `--format elasticdump` (`{"_index","_id","_source"[, "_routing"]}`, so `elasticdump-rs --input=file` loads it);
  * `--format bulk` (action + source, for seeding the cluster with the harness client, not the tool under test).
  * Compression: `--compress zstd|gzip`.
  * Splitting: `--split-bytes`.
* **Profiles:**
  * `logs`: ECS-like fields; ~0.3–1.5 KB; `@timestamp` monotonic with jitter; message text from a Markov model trained on a small bundled corpus; ip, status, bytes, user agent.
  * `metrics`: numeric-dense documents.
  * `nested`: nested arrays of objects.
  * `geo`: `geo_point` and `geo_shape` (WKT and GeoJSON).
  * `wide`: N fields, configurable up to 10k.
  * `huge`: 1–100 MB docs.
  * `edge`: the §5.4 catalogue.
  * `mixed`: weighted mix.
* **Each profile ships its mapping and settings** (`edgen --emit-mapping`), so the source index is created identically on every engine and version.
* **Ids:**
  * deterministic (`<profile>-<seed>-<i>` base36), plus an option for random UUIDv4 from the RNG;
  * an option for no `_id` (auto-id tests: BRK-NET-003);
  * routing for 10% of docs.
* **Throughput target:** ≥ 300k docs/s per core for `logs` (generation must never be the bottleneck in L1/L2).
* **Converters** (`edgen convert`): Rally/OSB NDJSON, CLF/combined logs (NASA, SecRepo, Kaggle, flog), and Loghub raw lines → `elasticdump` format with deterministic `_id = hash(dataset, line_no)`.

### 5.4 Edge-case content catalogue (`edgen --profile edge`, also used by BRK input tests)

Covered:
* **Unicode:**
  * all planes, including emoji, ZWJ sequences, RTL text, combining marks;
  * NUL as `\u0000`;
  * lone-surrogate escapes (`"\ud800"`);
  * U+2028/U+2029.
* **Escapes:**
  * every JSON escape;
  * strings consisting only of backslashes or quotes.
* **Numbers:**
  * integers > 2^53 and > 2^64;
  * negative zero;
  * `1e308`, `1e-320` (subnormal);
  * `1.0` vs `1`;
  * long decimal mantissas.
* **Structure:**
  * deep nesting (objects and arrays, depth 20, 100, 1,000, 10,000; deep ones only in file-level tests because ES rejects them);
  * empty objects and arrays;
  * mixed-type arrays inside a disabled-mapping object;
  * 10k-element arrays.
* **Size:**
  * 1 MB, 10 MB and 90 MB docs;
  * 64 KB single strings (Lucene keyword limit 32,766 bytes → `ignore_above`).
* **Ids:**
  * `_id` containing `/ ? # % & + : * " \ < > |`, spaces, unicode, a leading `_`;
  * an `_id` of exactly 512 bytes (the max) and one of 513 (rejected);
  * duplicate `_id`s inside one file.
* **Routing:** custom routing; `_routing` required by the mapping.
* **Index names:**
  * unicode;
  * `#`, `-`, `+`, `.`-prefixed (hidden);
  * date math `<logs-{now/d}>`;
  * 255-byte names.
* **Fields named like meta fields** (`_id`, `_source`, `hits`, `_scroll_id`, `pit_id`, `sort`) inside `_source`.
* **Dates:** `date_nanos`, epoch_millis strings, custom formats.

### 5.5 Requirements

| ID | Pri | Requirement | Acceptance criteria | Harness change | Effort |
|---|---|---|---|---|---|
| TST-DATA-001 | must | `edgen` with counter-based seeding, profiles `logs`, `nested`, `geo`, `wide`, `huge`, `edge`, `mixed`, and output formats `source`, `elasticdump`, `bulk` | Same seed gives the same bytes across runs and thread counts (CI hash check); ≥ 300k docs/s per core (`logs`) | `tools/edgen` | L |
| TST-DATA-002 | must | Profiles emit their mapping and settings, valid on every target version (with per-engine variants where needed, e.g. `flattened`/`flat_object`) | Index creation succeeds on all 9 images | edgen | M |
| TST-DATA-003 | must | Edge catalogue §5.4 implemented as `--profile edge`, with each case tagged (`_source.case`) so failures name the case | Every case has an expected outcome per version (store / reject with type X) | edgen + expectation file | M |
| TST-DATA-004 | must | Dataset manifest `datasets.toml` with URL, size, SHA-256, licence and tier for every §5.1 entry used in CI; fetch via INF-008; **licence recorded and checked** (no NC/research-only data in CI artefacts unless Q9 approves) | `fetch.sh --verify` passes; the licence gate fails on a disallowed licence | manifest | S |
| TST-DATA-005 | must | Converters: Rally/OSB NDJSON, CLF/combined, Loghub raw → `elasticdump` format with deterministic ids | Same input gives the same output hash | `edgen convert` | M |
| TST-DATA-006 | should | Cached L1 corpora on the runner (`http_logs-191998`, `eventdata`, `geonames`, `nested`, `pmc`): about 8 GB compressed; zstd re-compressed copies | Cache hit on a second nightly | CI cache or volume | S |
| TST-DATA-007 | should | OSB `generate-data` driven by our fixture mappings for a "foreign generator" cross-check of mapping coverage | Generated docs load on every version | optional job | S |
| TST-DATA-008 | could | elastic-integration-corpus-generator-tool with `--seed` for ECS data-stream corpora (nginx, system, aws) | Deterministic output hash | optional job (needs `epr.elastic.co`) | S |
| TST-DATA-009 | could | flog streaming mode for unbounded soak input (`flog -l -f json`), digested on the fly | Soak runs ≥ 6 h | soak runner | S |
| TST-DATA-010 | must | Deterministic `_id` policy for every external dataset (`hash(dataset, file, line)`) so dumps and restores are digest-comparable | Same id on every conversion | converters | S |

---
## 6. Adversarial and chaos testing: trying to break it (TST-BRK)

**Goal:** find fixes that need implementing.

**How the attacks were chosen:** each one comes from reading the code path it targets, at `rewrite` HEAD `d991ad8`, read-only.

**"Likely failure" is a hypothesis to test, not a confirmed bug,** unless it is marked **[code-confirmed]**. That mark means the behaviour follows directly from the cited lines. Even then it still needs a test to show user impact.

Every row also carries STD-ACC (§0): no panic, no silent data loss, clear error, correct exit code, no secret leakage.

### 6.1 Tooling

| Tool | Use | Where |
|---|---|---|
| Router mock (INF-002) | Malformed, slow, truncated, HTML, redirect, huge, and mixed-item responses; per-route scripts | `tests/support/router_mock.rs` |
| In-process fault proxy (INF-004) | Cut, refuse, half-close, latency or bandwidth cap between the binary and a real node | `tests/support/fault_proxy.rs` |
| toxiproxy (`ghcr.io/shopify/toxiproxy`, MIT) | The same faults for CLI-only runs, Node comparisons and load tiers; `reset_peer`, `timeout`, `slow_close`, `limit_data`, `latency`, `bandwidth` toxics | `scripts/chaos/toxiproxy.sh` |
| Node-side faults | `podman restart/pause/kill`; cluster settings (`cluster.routing.allocation.disk.watermark.flood_stage` low, `index.blocks.write`, `thread_pool.write.queue_size: 1`); `DELETE` index or `PUT _mapping` mid-run | `scripts/chaos/node.sh` |
| Resource limits | `podman run --memory=64m --pids-limit --ulimit nofile=64:64` around the tool image; `ulimit -n/-v` locally; a small tmpfs (`podman run --tmpfs /out:size=10m`) for ENOSPC; `/dev/full` for stdout | `scripts/chaos/limits.sh` |
| Signals | `kill -INT/-TERM/-KILL` at doc-count or time triggers; `timeout --signal` | runner hook `KILL_AT_DOCS` |
| Fuzzing | `cargo-fuzz` (libFuzzer) targets from TST-API-084; `proptest` | `fuzz/` |
| Cert factory | openssl script: expired, not-yet-valid, wrong host, IP-only SAN, self-signed, chain with a missing intermediate, RSA-1024, client certs | `scripts/chaos/certs.sh` |
| Secret canary | Unique random secrets planted in URL credentials, `--httpAuthFile`, env, `--headers`, `--params`, `--pass`; grep every output channel | `tests/support/canary.rs` |
| DNS faults | Unresolvable `.invalid` hosts; `/etc/hosts` override inside the tool container; `--dns` pointing at a black-hole resolver | podman flags |

### 6.2 Requirements

#### Network faults

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-NET-001 | must | Connection reset mid-response on a **scroll continuation** (after N bytes of the body) | `retrieval/scroll.rs` continuation (`RetryMode::RejectionOnly`), `http/retry.rs::send_inner` (body-read error → `TransportError`) | Not retried (the cursor may have advanced). Exit 1. Staged output discarded. Scroll cleared. Message suggests a larger `--retryAttempts` cannot help and that a rerun or `--scrollId` resume can | **[code-confirmed]** One network blip aborts a multi-hour dump with no resume path; this is by design but there is no way out. Test the message and the cleanup; open question Q4 on "re-open from last sort" resume | digest of a rerun = source | fault proxy | S |
| TST-BRK-NET-002 | must | Reset mid-response on a **PIT search** page, unsliced and sliced (16), with the PIT id rotating each page | `retrieval/pit_search.rs` (`Idempotent`), `retrieval/pit.rs::SharedPitCoordinator` (`observe_returned_id`, generations) | Retry the same `search_after` with the latest PIT id → no duplicates and no gaps | A retry after a slice already reported its generation uses a stale id, or double-counts a page (generation logic is upstream code, 406/452 lines) | digest equal over 50 seeded runs | fault proxy | M |
| TST-BRK-NET-003 | must | Reset or timeout **after the server applied a `_bulk`** (cut the response, not the request) | `pipeline/bulk.rs::send_bulk` (`RetryMode::Idempotent` → whole batch re-sent) | Explicit ids: harmless. **Auto-id docs** (`--sourceOnly` dumps, files without `_id`): duplicates must not happen silently. `create`: 409s on retry must not count as failures. `update` with scripts: non-idempotent | **[code-confirmed]** The whole batch is re-sent on any transport error or timeout: auto-id restores duplicate docs; `create` reports false failures | digest/count equal, or the tool refuses to retry non-idempotent batches and says so | fault proxy (cut response only) | M |
| TST-BRK-NET-004 | must | Server **stalls**: accepts and never responds; or sends headers then stops; no `--timeout` given | `http/client.rs::EsClient::new` (timeout only if set), `config.rs::request_timeout` (default `None`) | A default idle/read timeout (e.g. 5 min, configurable), then a retry or a clear failure | **[code-confirmed]** No default timeout → the process hangs forever (cron jobs pile up) | finishes or fails within the configured bound | router mock stall | S |
| TST-BRK-NET-005 | must | Slow drip (1 KB/s) on a large legitimate response with `--timeout 30000` | reqwest `.timeout()` is a **total** request deadline | Distinguish idle from total: a slow but progressing response succeeds | A legitimately large or slow page fails at 30 s, gets retried, and fails again | 200 MB response at 2 MB/s with `--timeout 30000` succeeds once idle-timeout semantics exist | toxiproxy `bandwidth` | S |
| TST-BRK-NET-006 | must | **Stale keep-alive connection:** the server or an LB closes idle pooled connections (e.g. after 5 s) while the sink is slow, then the next scroll continuation reuses the dead socket | hyper pool via reqwest; `scroll.rs` `RejectionOnly` | Requests that provably never reached the server (connection closed before any byte was written) are retried even for continuations | A reused dead connection surfaces as `TransportError` → continuation not retried → spurious fatal error on long dumps behind LBs | dump through a proxy with a 5 s idle close + `--throttleInterval 6000` succeeds | fault proxy (idle close) | M |
| TST-BRK-NET-007 | should | Half-closed socket: the server sends FIN after the headers, or the client write side is closed | client/hyper | Clear error, retry per mode | Hang or a misleading "connection closed" message | completes or fails within the timeout | fault proxy | S |
| TST-BRK-NET-008 | must | DNS failure: NXDOMAIN at start; resolver black-holed (no answer); DNS failing mid-run | reqwest resolver, `retry.rs::classify` (`ConnectError`) | NXDOMAIN fails fast with the host name. Black-holed DNS is bounded by the connect timeout. Mid-run failure is retried | Black-holed DNS has no connect timeout → hangs (no `connect_timeout` set in `EsClient::new`) | bounded time; message names the host (password-free) | podman `--dns` | S |
| TST-BRK-NET-009 | must | Node restart mid-dump and mid-restore (`podman restart`, ~30–60 s down) | retry defaults: 4 × 5 s = 20 s | Restore: survives if `--retryAttempts` covers the downtime, otherwise fails clearly. Dump: scroll/PIT contexts are lost → clear "context lost, rerun" error | Default 20 s retry budget is shorter than a restart: restore aborts mid-file; the message should give the `--offset` to resume | resume recipe works (LOAD-006) | node.sh | S |
| TST-BRK-NET-010 | must | Proxy returns **HTML**: 200 `text/html` login page; 502/503 HTML error pages; WAF 403 HTML; a 5 MB HTML body | `retry.rs::validate_response_bytes` (whole body in error), `extract.rs`, `info.rs::ClusterInfo::parse` (non-JSON → Elastic 5.0.0) | Content-type check: "expected JSON from <host>, got text/html (first 200 bytes …)". Error bodies truncated to ≤ 2 KB | **[code-confirmed]** The full body goes into the error message (terminal and log flood). A 200 HTML on `GET /` silently becomes "Elasticsearch 5.0.0" and drives wrong API choices | message ≤ 4 KB; no engine fallback without a warning | router mock | S |
| TST-BRK-NET-011 | should | `Content-Encoding: gzip` with a plain body; truncated gzip response; `Content-Length` shorter or longer than the body; malformed chunk sizes | reqwest decoding; `send_inner` (`Failed to read … response bytes`) | Clear decode error; treated like a transport error | Misclassified retry, or a hang on a long `Content-Length` | bounded, clear | router mock | S |
| TST-BRK-NET-012 | must | **Redirects:** 301/302/307/308 from the cluster URL to another host, to http (downgrade), and on `POST /_bulk` | reqwest default redirect policy (follows up to 10) | Do not follow redirects for API calls (or only same-origin 307/308), and say so | 301/302 turn POST into GET **without the body**: a bulk could "succeed" against a different endpoint, or credentials could follow to another host | no request reaches the redirect target with an `Authorization` header; no silent success | router mock (two listeners) | S |

#### TLS faults

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-TLS-001 | must | Server cert issues: expired, not yet valid, hostname mismatch, IP-only SAN accessed by name (and vice versa), self-signed, missing intermediate | `http/tls.rs::build_client_config`, `root_store` | Fails with the rustls reason, the host, and a hint (`--ca`, `--insecure`) | Generic "error sending request" without the cause (the `error_chain` helper should prevent this; verify per case) | message names the reason | cert factory | S |
| TST-BRK-TLS-002 | must | Client cert faults: required but missing; wrong CA; encrypted key with a wrong `--pass`; legacy `BEGIN RSA PRIVATE KEY` + DEK-Info; DER instead of PEM; cert/key mismatch; empty file; a CA bundle that also contains a private key | `tls.rs::load_certs`, `load_key` | Clear per-case error; never echo the passphrase | Panic on unexpected PEM sections, or the passphrase echoed in the error | canary not found | cert factory | S |
| TST-BRK-TLS-003 | should | Protocol mismatch: `https://` to a plain port; `http://` to a TLS port; TLS 1.0/1.1-only server; RSA-1024 server key | rustls config | Clear message ("server spoke plain HTTP", "no common protocol") | Long hang or retry loop on the plain↔TLS mix | bounded | openssl `s_server`, mock | S |
| TST-BRK-TLS-004 | should | `SSL_CERT_FILE`/`SSL_CERT_DIR` pointing at garbage; an empty system store | `rustls-native-certs` + `webpki-roots` fallback | Debug log, then falls back to bundled roots (documented) | Silent trust-store substitution surprises users of private CAs | documented and tested | env | S |

#### Server misbehaviour

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-SRV-001 | must | **429 storm on bulk items** for longer than the retry budget; also a flood-stage disk block (`cluster_block_exception` 429) and `index.blocks.write` (403 items) | `bulk.rs::write_chunk` → `report_failure` (with `ignore_es_write_errors`, default true, only `eprintln!`); `lib.rs` prints `Failed Writes` and returns `Ok` | Non-zero exit (or a distinct exit code) whenever documents were not written, unless the user opted in explicitly (Q3) | **[code-confirmed]** Exit **0** with `Failed Writes: N`: silent data loss for scripts that check `$?` (matches Node's default, but dangerous) | exit code policy enforced by test | node.sh + router mock | S |
| TST-BRK-SRV-002 | must | 5xx bursts on `_bulk` beyond the retry budget, with `--ignore-errors` | `bulk.rs::process_batch` (whole chunk counted failed, continue) | The loss is counted and reported; exit code per Q3 | Whole chunks of `--limit` docs dropped with exit 0 | asserted counts | router mock | S |
| TST-BRK-SRV-003 | must | Bulk response with mixed items: 201, 200, 409, 400 mapper_parsing, 429, 503, 500, and an item missing `status` | `bulk.rs::handle_response` (missing status → 0 → failure) | Each item classified correctly; only 429/503 retried; ids of failed docs listed | Item key different from the action (e.g. data streams) → status 0 → all counted failed | counts equal ground truth | router mock | S |
| TST-BRK-SRV-004 | must | Bulk response item count ≠ documents sent; `errors:false` but items missing; response not JSON | `handle_response` (`bail!`) | Fail clearly, never count as written | — (verify) | | router mock | S |
| TST-BRK-SRV-005 | must | Truncated or malformed JSON in a 200 search response (cut inside `hits.hits[k]`); missing `hits`; `hits.hits` not an array | `retrieval/extract.rs::extract_batch` (fused metadata + output) | The batch is rejected before any of its docs reach the sink | With **stdout** output (no staging), the first k hits of a broken page are already written when the error fires | stdout consumer sees no partial page + exit 1 | router mock | S |
| TST-BRK-SRV-006 | must | **Huge responses:** `--limit 10000` × 1 MB docs (10 GB page); a malicious server sending an endless body | `send_inner` → `response.bytes()` (unbounded buffer) | Response size cap (configurable), clear error suggesting a lower `--limit` | **[code-confirmed]** The whole response is buffered → OOM kill (exit 137) with no message | under a 512 MB cgroup: clear error, not OOM | router mock + limits.sh | M |
| TST-BRK-SRV-007 | must | Scroll expiry mid-dump (`--scrollTime 1s` + slow sink / `--throttleInterval`) and PIT expiry (`--pitKeepAlive 1s`) | `scroll.rs`, `pit_search.rs` | Clear "search context expired; raise --scrollTime/--pitKeepAlive" | Generic 404 text | message asserted | live | S |
| TST-BRK-SRV-008 | must | Index deleted mid-dump; index deleted mid-restore (auto-create recreates it with dynamic mappings); mapping changed mid-restore (`dynamic: strict` added) | retrieval; `bulk.rs` | Dump: clear error. Restore: detect that the target index was recreated (e.g. check the index uuid at start and end) or at least report mapper errors with a non-zero exit | Restore "succeeds" into an auto-created index with wrong mappings; strict-mapping rejections count as failed writes with exit 0 | asserted | node.sh | M |
| TST-BRK-SRV-009 | must | Server reports `hits.total.value = N` (`relation: eq`) for a scroll or PIT but returns fewer docs (an early empty page) | `retrieval/mod.rs` (totals only drive progress bars; no reconciliation) | For snapshot reads (scroll/PIT, no `--size`/`--offset`/`--searchBody` size), retrieved ≠ exact total is an **error** (or at least a loud warning) | **[code-confirmed]** No reconciliation → silently short dumps | asserted with mock early-EOF | router mock | S |
| TST-BRK-SRV-010 | must | Duplicate or overlapping docs across PIT pages or slices (mock returns the same hit on two pages; or sort values that tie) | `pit_search.rs`, `slice_state.rs` | Detected by the digest in tests; the tool does not dedupe, but tests catch regressions | Ties on `_doc` sort without `_id` (OS) lose or duplicate docs (README says this was observed) | digest equal on tie-heavy data | live + mock | S |
| TST-BRK-SRV-011 | should | Version conflicts under concurrency: two restores of the same file in parallel; `--bulkAction create`; ISM 409 twice | bulk; `policy/mod.rs::put_ism` | Counts correct; ISM second 409 reported as a concurrent change | — | | live | S |
| TST-BRK-SRV-012 | should | Cluster `red` / some shards unassigned during a dump | extract (`_shards.failed`) | Fails clearly naming the shards | — (verify) | | node.sh (`index.routing.allocation.require` to a missing node) | S |

#### Hostile or corrupt input files

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-IN-001 | must | Truncated gzip (cut at random offsets, incl. inside the trailer) and trailing garbage after a gzip member | `pipeline/file_source.rs::open_reader` (`MultiGzDecoder`), `read_loop` | Error naming the file, byte offset and docs read so far; non-zero exit | Last partial batch already sent; message without an offset | exit 1; the message has an offset | fixtures | S |
| TST-BRK-IN-002 | must | **Truncated zstd** frame; zstd frames concatenated with garbage; a `.zst` file that is really gzip | `open_reader` (`zstd::stream::read::Decoder`) | Error as above | A truncated zstd stream may end with `Ok(0)` (EOF) instead of an error → **silent partial restore** | exit 1 | fixtures | S |
| TST-BRK-IN-003 | should | Wrong compression: bzip2 (the Rally corpora!), xz, lz4, zip, tar.gz | `open_reader` (only gzip/zstd magic) | "bzip2 input is not supported; decompress first" | Treated as plain text → "Invalid JSON" with no hint | message asserted | fixtures | S |
| TST-BRK-IN-004 | must | **Mixed or wrong types:** a mapping, settings, alias or template dump restored with `--type data`; a data dump restored with `--type mapping` | data path has **no shape check** (only `meta/mod.rs:265` for meta types) | "this input looks like a --type mapping dump" for data input too | **[code-confirmed]** Metadata is bulk-indexed into the output index as a document → silent pollution | exit 2 or 1, nothing written | fixtures | S |
| TST-BRK-IN-005 | must | Malformed values that the splitter accepts: unbalanced `{"a":1]`, stray `}` at top level, top-level scalars (`123`, `"x"`, `null`), arrays, `NaN`/`Infinity` tokens (Python `json.dumps` default), trailing commas | `pipeline/json_split.rs::ValueSplitter::next_value` (bracket depth only, no validation), `bulk.rs::encode_doc` | Per-document validation with line number; skip or fail per flags, without poisoning the whole batch | **[code-confirmed]** The splitter passes these through; ES rejects the **whole** bulk request (400) → the entire chunk fails; with `--ignore-errors` up to `--limit` good docs are dropped | only the bad line is reported/skipped | fixtures + fuzz | M |
| TST-BRK-IN-006 | must | Invalid UTF-8 inside strings; UTF-16 LE/BE files with BOM; a UTF-8 BOM in the middle of the file; Latin-1 text | splitter (byte-level), bulk | "line N is not valid UTF-8" | Whole-bulk 400, as IN-005; UTF-16 gives a confusing error; `is_ws` treats 0xEF/0xBB/0xBF anywhere between values as whitespace | clear messages | fixtures | S |
| TST-BRK-IN-007 | must | **Giant single value:** a 2 GB document; a 10 GB file that opens `[` and never closes; a 1 GB string | `ValueSplitter` buffer (grows without a cap), `single_line` | Max-document-size guard (default ≤ `http.max_content_length`) with a clear error | **[code-confirmed]** Unbounded buffer → OOM | under a 512 MB cgroup: clear error | fixtures (generated sparse) + limits | S |
| TST-BRK-IN-008 | must | Deep nesting: 1k, 10k and 100k levels of `[`/`{` in a data file and in a meta file | data: `sonic_rs` lazy `get` in `bulk.rs::meta`/`encode_doc` on worker threads; meta: `serde_json` (recursion limit 128) | Clear error, no crash | A stack overflow in a worker thread **aborts the process** (not a catchable panic) | no SIGSEGV/SIGABRT | fixtures + fuzz | S |
| TST-BRK-IN-009 | must | Duplicate keys in a document line: `{"_id":"a","_id":"b",…}`, duplicate `_source`/`_index`/`_routing` | `bulk.rs::meta` (sonic `get` returns the first match) | Same semantics as Node (`JSON.parse` keeps the **last**) or reject | Different `_id`/`_index` than Node for the same file → interop divergence | Node vs Rust digest equal, or a documented rejection | fixtures + Node | S |
| TST-BRK-IN-010 | must | Duplicate `_id`s across lines (last write wins) | bulk accounting | Report created vs updated counts (from item `result`) so users can see overwrites | `Total Writes` = lines while `_count` is smaller; nothing flags it | summary includes `updated` | live | S |
| TST-BRK-IN-011 | must | Big numbers and odd numerics: > 2^64, `-0`, `1e400`, long mantissas, numbers in `_id`/`_routing` positions (`"_id": 12345678901234567890`) | raw byte pass-through; `meta()` returns the raw JSON of a non-string `_id` | Numeric `_id` handled as Node does (stringified) without precision loss | A numeric `_id` written raw into the action line → ES coerces it, or a precision change | byte-exact `_source`; `_id` equals Node's | fixtures + Node | S |
| TST-BRK-IN-012 | must | Special characters in `_id` (`/ ? # % + " \` unicode, 512/513 bytes); in index names (`#`, `:`, uppercase, `..`, `-x`, 256 bytes, date math `<logs-{now/d}>`); template, policy and script ids with `/` and unicode | `http/client.rs::url` segment encoding; `rename.rs::validate_index_name` | Encoded correctly or rejected up front with the ES rule | Date math or `%` mis-encoded in URL paths; 513-byte ids fail the whole batch | per-case expectation file | edgen edge + live | S |
| TST-BRK-IN-013 | must | Node double-encoding edge cases for meta files: triple-encoded strings; a string literal holding an array; `\u0000`; lone surrogates (`"\ud800"`, which Node ≥ 12 emits escaped); an object instead of a string; several values in one file | `meta/codec.rs::read_first_value`, `decode_any` (`serde_json`) | Accept everything Node writes; reject the rest clearly | `serde_json` rejects lone-surrogate escapes that Node produces → valid Node dumps fail to restore | Node-generated corpus all restores | Node corpus + proptest | S |
| TST-BRK-IN-014 | must | Empty inputs: 0-byte file, whitespace only, BOM only, an empty `.gz`, an empty `.zst`, `/dev/null`, a closed stdin | `file_source.rs`, `codec.rs` ("Input is empty") | Data: `Total Writes: 0` plus a warning, exit 0 (or Q3). Meta: clear error | Panics or confusing errors on the empty compressed cases | asserted | fixtures | S |
| TST-BRK-IN-015 | should | Split-part problems: a missing part (`split-1`, `split-3`); parts restored out of order; a staging leftover `.x.part-…` passed as input; a part that is truncated | per-part restore loop (README) | Each part restores independently; truncated part per IN-001; a helper (`--input` glob or a manifest) warns about gaps | Gaps are undetectable → silent loss | gap warning if a manifest/glob mode is added | fixtures | M |
| TST-BRK-IN-016 | could | Input file modified while reading (appended, truncated, replaced) | `read_loop` | Defined behaviour (read to the EOF seen at open, or fail) | Mixed content | documented | fixtures | S |

#### Resource exhaustion

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-RES-001 | must | Low memory: cgroup 64/128/256 MB with defaults; with `--limit 10000` × 40 KB docs; with `--concurrency 16 --slices 16` | buffers in `retrieval/*`, `bulk.rs`, `file_source.rs` | Stays within the README memory formula; warns at start when the estimate exceeds the available memory | OOM kill (137) with no message; staging left behind | no OOM inside the formula's bound; leftovers handled (SIG-003) | limits.sh | M |
| TST-BRK-RES-002 | must | **Disk full** on output (tmpfs 10 MB): single file; split parts; ENOSPC **during the commit** of part k of n | `output.rs::commit`, `file_sink.rs::finalize` (commits parts **sequentially**, discards the rest on error) | All-or-nothing split commit as documented ("staged and committed together"); existing destination untouched; staging removed | **[code-confirmed]** Parts 1..k-1 stay committed when commit k fails (EEXIST or I/O error) → a partial split set on disk contradicts the README | after failure: either all parts or none | limits.sh (tmpfs) + pre-created conflicting part file | S |
| TST-BRK-RES-003 | should | stdout to `/dev/full`; stdout to a closed pipe (exit 0 tested already); stderr closed | `output.rs::is_broken_pipe` | ENOSPC on stdout → exit 1 | Treated like EPIPE → exit 0 with loss | asserted | shell | S |
| TST-BRK-RES-004 | must | FD limit `ulimit -n 32` with `--concurrency 64`, `--slices 16`, many split parts | reqwest pool; `file_sink.rs` (staged parts kept until finalize) | "too many open files" with a hint | EMFILE classified as `ConnectError` → retried 4× → confusing error; split mode hits EMFILE at part ~25 | message asserted | limits.sh | S |
| TST-BRK-RES-005 | must | Extreme numeric flags: `--limit 1000000`, `--limit 18446744073709551615`, `--concurrency 100000`, `--slices 5000`, `--bufferSize 1e9`, `--maxBulkBytes 1`, `--retryAttempts 1e6`, `--retryDelay 0` | `cli.rs` parsers; `Vec::with_capacity` sites; ES `max_result_window` (10,000) and `max_slices_per_scroll` (1,024) | Validated at parse time (exit 2) or server errors explained | Capacity-overflow panic or a huge allocation; raw ES errors | no panic for any value in the u64 range (proptest over the CLI) | proptest(CLI) | S |
| TST-BRK-RES-006 | must | Tiny `--fileSize` (`1b`) and `--maxRows 1` on 1M docs | `file_sink.rs` (one staged file per part, all kept until finalize) | Refuse below a sane minimum, or stream-commit with a warning | 1M staged files and FDs/inodes exhausted; finalize takes hours | refused (exit 2) or completes within budget | scenario | S |
| TST-BRK-RES-007 | should | Very high `--concurrency` with `--maxSockets 1` | `http/gate.rs`, reqwest `pool_max_idle_per_host` (not a connection cap) | Semantics documented and enforced | `--maxSockets` caps idle connections only, not concurrent requests | asserted connection count | fault proxy counts | S |

#### CLI and configuration abuse

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-CLI-001 | must | Conflicting flags, table-driven (≥ 40 combos): `--fsCompress` + `--compressionType zstd`; `--sourceOnly` + `--bulkAction update`; `--pit` + `--scrollId`; `--searchAfter` without sort; `--offset` + `--slices`; `--format human` + `--fileSize`; `--renameIndex` + `--output-index`; `--type alias` + `--sourceOnly`; `--skip-existing` + `--overwrite` | `lib.rs::validate`, `cli.rs` | Each is a usage error (exit 2) or has documented precedence | Silent precedence | table asserted | cli_contract | M |
| TST-BRK-CLI-002 | must | **Same source and destination** (same cluster and index; same file path for input and output) | `lib.rs::validate` | Refuse unless `--force` | With auto ids (`--sourceOnly`), a self-copy doubles the index; a same-file run reads its own staging | refused | cli_contract + live | S |
| TST-BRK-CLI-003 | must | **Rename collisions:** two source indices mapping to one target (`--renameIndex '^(a\|b)-(.*):x-$2'`); rename to an existing unrelated index; rename chains (a→b and b→c) | `rename.rs::Renamer::index` (no collision detection found) | Detect many-to-one mappings before writing and fail (or require `--allowMerge`) | **[code-confirmed]** No detection → indices merge silently, `_id` overwrites | refused, nothing written | contract + live | S |
| TST-BRK-CLI-004 | should | Regex abuse in `--renameIndex`, `--templateRegex`: huge patterns (`(a{1000}){1000}`), 1 MB pattern, invalid UTF-8 escapes, capture references to missing groups (`$9`), `${name}` typos | `rename.rs` (`Regex::new`), `lib.rs:457`; `meta/policy/mod.rs:35` (`is_ok_and` silently treats a build failure as "no match") | Clear compile error with a size-limit hint; missing captures are an error | Silent empty replacements for missing groups; silent non-match in the policy pattern filter | asserted | cli_contract | S |
| TST-BRK-CLI-005 | must | Output path abuse: a directory; `/`; a symlink to elsewhere (hard-link commit semantics); a FIFO; `/dev/stdout`; a path with `..`; a non-existent parent; a read-only directory; an existing file without `--overwrite`, and with mode 0600 (mode preserved?) | `output.rs::create_output_target`, `build_staging_path`, `commit` (hard link, then rename) | Clear errors; the symlink target is never silently replaced; the mode is preserved | Committing through a symlink replaces the link rather than the target, or vice versa (decide and test) | asserted | fs fixtures | S |
| TST-BRK-CLI-006 | must | Credential sources and precedence: URL vs `--httpAuthFile` vs env vs `--username/--password`; passwords containing `: @ % # / ?`, spaces, unicode, trailing `\r` (CRLF auth file), empty password; an auth file that is world-readable | `endpoint.rs::resolve_auth`, `read_auth_file` (`ini` crate) | Documented precedence; CRLF trimmed; warning for world-readable auth files | `\r` kept in the password → confusing 401; `#` in a URL password truncates it (fragment) | asserted | contract | S |
| TST-BRK-CLI-007 | must | **Secret leakage canary:** plant unique secrets in every credential channel (URL, auth file, env, `--headers Authorization`, `--params api_key=…`, `--pass`), then trigger every error class (connect, DNS, TLS, 401, 403, 500 that echoes the request, a panic via a fuzz-found input) with and without `--debug` | `http/client.rs::send` (**`--debug` logs the full URL including `--params`**), `endpoint.rs::redacted_endpoint` (**returns the raw string when URL parsing fails**), `EsHttpError` (echoes the server body), clap errors (echo argv values), `ClientOptions`/`Cli` (`#[derive(Debug)]` with password fields) | No canary in stdout, stderr, log files, output files or staging names | **[code-confirmed]** `--params` secrets are printed by `--debug`; scheme-less or invalid URLs containing credentials are logged raw at start | 0 canary hits across the matrix | canary + router mock | M |
| TST-BRK-CLI-008 | should | Scheme-less URLs (`user:pw@host:9200/idx`), `HTTP://` uppercase, `http:/host` typos | `endpoint.rs::parse` (anything not `http(s)://` is a **file path**) | Warn when a "file" output looks like a URL (`host:port`, `@`) | Writes a file named `user:pw@host:9200/idx` (credentials on disk); a restore reads a missing "file" | warning asserted | contract | S |

#### Signals and interruption

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-SIG-001 | must | **SIGTERM** mid-dump, mid-split, mid-restore (what `podman stop`, Kubernetes and systemd send) | `retrieval/mod.rs` installs a handler for **ctrl_c only** (SIGINT) | Same as SIGINT: cancel, clear scroll, close PIT, remove staging, exit 143 (or 130) | **[code-confirmed]** Default SIGTERM disposition kills the process: staging files left, scroll/PIT contexts open until expiry | no `.part-` leftovers; contexts closed | runner kill hook | S |
| TST-BRK-SIG-002 | must | SIGINT during a **file→cluster restore** and cluster→cluster | `lib.rs` file path uses `CancellationToken::new()` (not wired to a signal) | Stop after in-flight bulks, print `Total Writes` and the `--offset` to resume, exit 130 | Immediate kill without a resume hint | message includes the resume offset | kill hook | S |
| TST-BRK-SIG-003 | must | `kill -9` at random points (dump, split, restore) ×50 runs: crash safety of output and split commits | `output.rs` (staging + fsync + hard link/rename), `file_sink.rs::finalize` | The final path is either absent or complete (digest equal); staging leftovers are detectable and a later run cleans them up or warns | Leftover `.name.part-PID-TS` files accumulate forever (no cleanup on the next run); a torn split set if killed mid-commit (RES-002) | 50/50 runs crash-consistent | kill hook | M |
| TST-BRK-SIG-004 | should | Second SIGINT during cleanup (forced exit path `std::process::exit(130)`); SIGINT while the analyzer index is closed (`--closeIndex`) | `retrieval/mod.rs:200`; `index_scoped.rs:481` | The forced exit still leaves no committed partial output; the analyzer index is reopened, or the message says it is closed | Index left closed after a double interrupt | asserted | kill hook | S |
| TST-BRK-SIG-005 | must | Resume semantics end to end: `--scrollId` after a kill (context alive), `--offset` after a restore kill, rerunning a dump with `--overwrite` | `scroll.rs` resume, `pipeline/window.rs` | Documented recipes produce the full digest | Off-by-one in `--offset` vs `Total Writes` with concurrency > 1 (out-of-order completion) | digest equal | LOAD-006 | S |

#### ILM→ISM conversion

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-ILM-001 | must | Unsupported phases and actions (`frozen`, `searchable_snapshot`, `downsample`, `wait_for_snapshot`, `unfollow`, `freeze`, `total_shards_per_node`, `index_codec`, `allow_write_after_shrink: true`, `max_primary_shard_docs`, `min_*` rollover conditions, unknown future action) with and without `--lenientPolicyConversion` | `meta/policy/ilm_to_ism.rs::convert` | Refused naming phase and action; lenient mode drops with `DROPPED …`; no partially written policy set | — (unit-tested; extend to every combination via proptest, API-086) | OpenSearch accepts every converted policy | proptest + live | S |
| TST-BRK-ILM-002 | must | Odd but valid ILM: phases out of JSON order; `min_age` in every unit (`0ms`, `30s`, `1d`, `1000d`); missing `min_age`; empty `phases`; only a `delete` phase; rollover-only hot phase; `_meta` with nested objects; unicode policy names | `convert` | Correct state order and transitions; ISM accepts them | Transition units mis-converted (e.g. `ms`, `micros`, `nanos`) | live PUT on OS 1.3/2.19/3.x | live | S |
| TST-BRK-ILM-003 | should | Converting ES built-in managed policies (when `--filterManaged=false`) and ISM → ILM attempts | `policy/mod.rs` | Clear refusal for ISM → ILM; managed policies convert or refuse clearly | — | | live | S |

#### Cross-version oddities

| ID | Pri | Attack | Target (module::function) | Expected correct behaviour | Likely failure (hypothesis) | Extra acceptance | Tool | Effort |
|---|---|---|---|---|---|---|---|---|
| TST-BRK-XV-001 | must | OpenSearch with `compatibility.override_main_response_version: true` (reports `7.10.2`; check whether `distribution` is still present); proxies that rewrite `GET /`; AWS-style responses | `http/info.rs::ClusterInfo::parse` (flavor = OpenSearch only if `version.distribution == "opensearch"`; unparsable → Elastic 5.0.0) | Detect OpenSearch by other signals (`tagline`, `_plugins` probe) or fail clearly | Misdetected as Elasticsearch → ES PIT API and `_shard_doc` sent to OpenSearch → errors, or worse a silent fallback path | correct engine on the live OS setting | live (OS setting) + golden | S |
| TST-BRK-XV-002 | must | **`_source` not stored:** `_source.enabled: false`, synthetic source (ES ≥ 8.4), `index.mode: logsdb` (ES 8.17+, default for `logs-*` data streams in 9.x), `_source.excludes` in the mapping | data dump path (no mapping check before dumping) | Detect from the mapping and refuse or warn loudly ("documents cannot be fully dumped") | **[code-confirmed]** No check: disabled `_source` → empty docs restored; synthetic source → reordered keys, deduplicated/sorted arrays, changed numbers, silently | warning/refusal asserted per version | live (ES 8.19, 9.1) | S |
| TST-BRK-XV-003 | must | **Data streams:** dump `logs-x-y` (hits carry `_index: .ds-logs-x-y-…`), then restore to another cluster or engine without `--output-index` | `bulk.rs::index_for` (uses `_index` from the doc) | Map backing indices back to the data stream name and require `--bulkAction create`, or warn and stop | Restores into hidden plain indices named `.ds-…` on the target (OpenSearch accepts them) → data invisible to users | asserted | live | S |
| TST-BRK-XV-004 | should | Field types without an equivalent across engines: `flattened` → OS, `flat_object` → ES, `semantic_text`, `sparse_vector`, `dense_vector` dims > OS limit, `match_only_text`, `wildcard`, `constant_keyword`, `aggregate_metric_double` | mapping restore cross-engine | Clear refusal naming the field and type; data restore fails clearly when the mapping is missing | Dynamic mapping on the target silently guesses types | expectation table (API-071) | live | M |
| TST-BRK-XV-005 | should | ES 7.10 OSS (no X-Pack) as a target of ILM, security and PIT features; OS 1.3 as a target of composable templates with `data_stream` | per-type paths | Clear capability errors | Raw 400s | asserted | live | S |
| TST-BRK-XV-006 | should | `hits.total` as an int (ES 6.x-shaped goldens, `rest_total_hits_as_int`) and `_type` present in hits on 7.x for old indices | `extract.rs`, `retrieval_task.rs::extract_total_hits_estimate` | Handled | Progress/total confusion | golden | golden | S |

### 6.3 Top 15 most likely to find real bugs (run these first)

| # | ID | Why it is likely |
|---|---|---|
| 1 | TST-BRK-NET-003 | Whole-batch bulk retry after an applied request duplicates auto-id docs and turns `create` into false failures (code-confirmed retry mode) |
| 2 | TST-BRK-SRV-001 | Exit 0 with `Failed Writes > 0` after 429 storms or flood-stage blocks: silent loss for automation (code-confirmed) |
| 3 | TST-BRK-NET-004 | No default request timeout: a stalled server hangs the tool forever (code-confirmed) |
| 4 | TST-BRK-SRV-009 | No reconciliation of exact `hits.total` vs retrieved docs: short dumps pass silently (code-confirmed) |
| 5 | TST-BRK-IN-004 | No shape check on data input: a metadata dump restored as data pollutes the index (code-confirmed) |
| 6 | TST-BRK-IN-005 | The splitter accepts invalid values; one bad line fails a whole bulk of up to `--limit` docs (code-confirmed) |
| 7 | TST-BRK-SIG-001 | SIGTERM is not handled: staging leftovers and leaked scroll/PIT contexts under containers or systemd (code-confirmed) |
| 8 | TST-BRK-RES-002 | Split commit is sequential: a failure at part k leaves parts 1..k-1 committed, contradicting "committed together" (code-confirmed) |
| 9 | TST-BRK-XV-002 | Disabled/synthetic `_source` and logsdb dump silently wrong data (code-confirmed absence of a check) |
| 10 | TST-BRK-XV-003 | Data stream dumps restore into hidden `.ds-*` plain indices |
| 11 | TST-BRK-SRV-006 / IN-007 | Unbounded response and line buffering → OOM kill without a message (code-confirmed) |
| 12 | TST-BRK-CLI-003 | Rename collisions silently merge indices (code-confirmed absence) |
| 13 | TST-BRK-CLI-007 | Secret leakage via `--debug` URLs with `--params` and raw logging of unparsable URLs (code-confirmed) |
| 14 | TST-BRK-NET-006 | Stale pooled connections make scroll continuations fail fatally behind load balancers |
| 15 | TST-BRK-IN-002 | Truncated zstd may read as a clean EOF → silent partial restore |

**Runners-up:**
* NET-012 (redirects turning bulk POST into GET);
* IN-008 (deep nesting stack overflow aborting the process);
* IN-009 (first-wins vs last-wins duplicate keys);
* IN-013 (lone surrogates in Node meta dumps);
* XV-001 (OpenSearch compatibility-mode misdetection);
* NET-010 (HTML on `GET /` → "Elasticsearch 5.0.0").

---
## 7. Supply chain and hostile-code review (TST-SUP)

**Threat model** (as instructed):
* the upstream code (`tenyears-tech/elasticdump-rs`, self-described AI-written; commits up to `b32b6e9`, authors "Kainan Zhu" and "microdog-codev") is treated as potentially hostile;
* so is every third-party crate, GitHub Action, container image and install script.

**Goals:**
* the binary talks only to the endpoints the user names;
* it reads and writes only the paths it is given;
* it never spawns processes;
* builds are reproducible and reviewed;
* regressions in any of this are caught automatically.

### 7.1 Initial audit (read-only, 2026-10-29, `rewrite` HEAD `d991ad8` + uncommitted `SECURITY-CVE.md`)

**How the audit was done:**
* `rg` over `src/`, `tests/`, `scripts/`, `.github/`, `Dockerfile`, `docker-compose.yaml`, `Makefile`.
* `cargo metadata --offline --locked` and `cargo tree --offline --locked --target x86_64-unknown-linux-gnu` (read-only; nothing was built).
* The `build.rs` of the active build-script crates, read from `~/.cargo/registry/src`.
* `git blame HEAD` for upstream vs rewrite origin.

**Code origin (blame of `src/`):**
* 9,461 lines rewrite (Robert Goley commits `6cc6a5f`…`d991ad8`);
* 4,232 lines upstream (Kainan Zhu 2,092 + microdog-codev 2,140), about 31%.
* Upstream lines are concentrated in:
  * `src/retrieval/` (`worker.rs` 571/571, `extract.rs` 542/549, `pit_search.rs` 489/497, `pit.rs` 406/452, `retrieval_task.rs` 569/605, `mod.rs` 634/748, `scroll.rs` 254/296, `slice_state.rs`, `messages.rs`, `progress.rs` 100%);
  * `src/output.rs` (395/507).
* Outside `src/`, also upstream: `tests/integration_test.rs` (2,511/2,573), `tests/support/mock_es.rs` (187/209), `scripts/benchmark/compare-with-elasticdump.sh` (1,506/1,506), `docker-compose.yaml` (100%).
* Upstream also depended on the alpha `elasticsearch` crate; the rewrite removed it.

**Checked and found clean:**

| Check | Result |
|---|---|
| Outbound network | Only `reqwest` to the user's `--input`/`--output` URLs. No hard-coded hosts, IPs or domains in `src/` (only `localhost`/`h` inside `#[cfg(test)]` modules). No telemetry, update checks or analytics. No `TcpStream`/`UdpSocket`/`lookup_host` use in `src/`. **Caveat:** env proxies, F-07 |
| Credential and file reads | Only paths the user gives (`--input`, `--searchBody=@file`, `--httpAuthFile`, `--ca/--cert/--key`) plus the system CA store (`rustls-native-certs`/`openssl-probe`, which also read `SSL_CERT_FILE`/`SSL_CERT_DIR`). No `HOME`, `~/.ssh`, `~/.aws`, `~/.kube`, or browser profile access. No `std::env::vars()` dumps |
| Env vars read by first-party code | `ELASTICDUMP_{INPUT,OUTPUT}_{USERNAME,PASSWORD}` (`config.rs:154,168` → `endpoint.rs::resolve_auth`), `NODE_TLS_REJECT_UNAUTHORIZED` (`config.rs:181`), `RUST_LOG` (env_logger) |
| Writes | The output path and a staging sibling `.<name>.part-<pid>-<ns>` in the same directory (`output.rs:370-390`). `file_sink.rs:168` lists the output directory to find existing split parts. Tests write to `temp_dir()` (except F-11) |
| Process spawning | None in `src/` (`std::process` is used only for `id()` and `exit()`). Worker OS threads (`retrieval/worker.rs:62`) wrap work in `catch_unwind` |
| `unsafe` | 0 occurrences in `src/` and `tests/` |
| Obfuscation | None. `base64` is used only to build the Basic auth header (`http/client.rs:118`). No `include_bytes!`/`include_str!`, no hex/XOR blobs |
| `build.rs` / proc macros (first party) | None |
| Dependency sources | All 271 `Cargo.lock` entries come from crates.io. No git or path dependencies. `deny.toml` denies unknown registries and git sources |
| Typosquats | Manual scan of the 245 crates on the Linux build graph: all well-known names and publishers. Two need a quick publisher check (H-04) |
| Third-party build scripts (active on Linux) | Read `ring` (cc/asm; `nasm` only on Windows), `zstd-sys` (bundled zstd 1.5.7 C via cc), `anyhow`/`httparse`/`zmij`/`rustversion` (run `rustc --version`), `crunchy`, `tiny-keccak`, `serde_json`, `rustls`: only `OUT_DIR` writes, compiler probes and C compilation. No network, no home-directory access |
| CI exfiltration and publishing | Nothing pushes or publishes. Artefacts are SBOMs, scan reports and release tarballs attached to workflow runs. The only secret is `DHI_TOKEN`/`DHI_USERNAME`, used for a pull-only login on manual dispatch (`security.yml:43-45`, password via stdin) |
| Benchmark and IT scripts | `compare-with-elasticdump.sh` (upstream): only `curl` to `$ES_URL`, its own `mktemp` dir and `rm -rf` of that dir, optional `taskset`/`cpulimit`. No downloads or remote code. `scripts/it/*`: podman, openssl, curl to the local container |

**Confirmed issues (severity-ranked).** All are fixes to schedule, not exploits. None shows hostile intent.

| # | Severity | Finding | Location | Origin | Recommended fix |
|---|---|---|---|---|---|
| F-01 | Medium | CI runs remote install scripts from **unpinned `main` branches** piped to `sh` (grype, syft, trivy) | `.github/workflows/security.yml:40-42` | rewrite | Pin release versions and verify checksums/cosign signatures, or use digest-pinned scanner images. Never `curl … \| sh` from a branch |
| F-02 | Medium | GitHub Actions pinned to **mutable tags** (`actions/checkout@v4`, `dtolnay/rust-toolchain@1.85.1`, `Swatinem/rust-cache@v2`, `taiki-e/install-action@v2`, `mlugg/setup-zig@v1`, `actions/upload-artifact@v4`) | `ci.yml:12-15,48-50,64-66`; `release.yml:15-30`; `security.yml:23-31,59` | rewrite | Pin to full commit SHAs (with tag comments); Dependabot for Actions |
| F-03 | Medium | No `permissions:` block in `ci.yml` and `release.yml` → the `GITHUB_TOKEN` scope falls back to repo/org defaults (possibly write) | `ci.yml:1-8`, `release.yml:1-6` | rewrite | `permissions: contents: read` at the top of each workflow; `persist-credentials: false` on checkout |
| F-04 | Medium (test integrity) | `.gitignore` `*.jsonl` ignores **`tests/fixtures/node/stored_scripts.jsonl`** (`git status --ignored` shows `!!`), but `tests/it/meta_types.rs:997` needs it → a clean CI checkout cannot run that test as on the box | `.gitignore` (`*.jsonl`), `tests/it/meta_types.rs:997` | rewrite | Add `!tests/fixtures/**` and commit the fixture; INF-014 check |
| F-05 | Medium (secret leak) | `--debug` logs the **full request URL including `--params` query values** (e.g. `api_key=…`) | `src/http/client.rs:225-226` | rewrite | Log method and path only, or mask every query value except an allow-list (`scroll`, `keep_alive`, `size`, `from`, …); covered by BRK-CLI-007 |
| F-06 | Low | `redacted_endpoint` returns the **raw string** when the URL does not parse or is not http(s), and `lib.rs:582-585` logs it at info; scheme-less `user:pw@host:9200/idx` becomes a *file path* | `src/endpoint.rs:71-76`, `src/endpoint.rs:115-122`, `src/lib.rs:582-585` | rewrite | Regex fallback masking `//user:pass@` and `user:pass@host`; warn when a file endpoint looks like a URL (BRK-CLI-008) |
| F-07 | Low | **Implicit egress via env proxies:** reqwest honours `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` by default; `EsClient::new` neither disables nor logs it, so data and Basic-auth headers (cleartext on `http://`) can go to a host not named on the command line | `src/http/client.rs:149-159` (`reqwest::Client::builder()` without `.no_proxy()`) | rewrite | Log "using proxy X for Y" at warn level; add `--noProxy` (or require an explicit `--proxy`); document (Node's `request` also honours env proxies, so this is about visibility) |
| F-08 | Low | `#[derive(Debug)]` on structs holding secrets (`ClientOptions.password`; `Cli.password/pass/input_pass/output_pass`). Not printed today, but one `{:?}` would leak them | `src/http/client.rs:47-48`, `src/cli.rs:95` | rewrite | Wrap secrets in a redacting newtype (e.g. `secrecy::SecretString` or a local `Secret<String>` with a masked `Debug`) |
| F-09 | Low | `workflow_dispatch` input interpolated straight into a shell script (`case "${{ github.event.inputs.base \|\| 'alpine' }}"`); only users with write access can trigger it | `.github/workflows/security.yml:48` | rewrite | Pass via `env:` and validate against an allow-list |
| F-10 | Low | `cargo install cargo-zigbuild --locked` without `--version`; `setup-zig` downloads Zig at run time | `.github/workflows/release.yml:21-23` | rewrite | Pin versions and checksums |
| F-11 | Low | Tests write into the CWD (repo root): `test_query_<index>.json`; one such file and an empty `oom` file are **committed** | `tests/integration_test.rs:634-638` (upstream code); stray files committed in `e37e696` and `6cc6a5f` (rewrite) | upstream + rewrite | Use `tempfile`; delete `test_query_elasticdump_rs_test_857109_16.json` and `oom` |
| F-12 | Low | `docker-compose.yaml` publishes 9200 on **all interfaces** with security disabled | `docker-compose.yaml:21-22` (ports) and `:10` (`xpack.security.enabled=false`) | upstream | Bind `127.0.0.1:9200:9200` |
| F-13 | Low | IT harness: fixed default admin passwords, written to `$IT_DIR/env` under `/tmp` with the default umask (readable by other local users on a shared box) | `scripts/it/up.sh:106,154,228-251` | rewrite | Random per-run passwords; `umask 077` for `$IT_DIR` |
| F-14 | Info | Images pulled by tag, not digest (IT profiles, Dockerfile bases, compose); `bitnamilegacy/*` is a frozen namespace | `scripts/it/up.sh:21,27`, `scripts/it/run.sh:41-46`, `Dockerfile` `FROM` args, `docker-compose.yaml:3` | rewrite + upstream | INF-012 digest pinning |

**Needs a human look (not confirmed problems):**

| # | Topic | Location | Origin | Why look / suggested action |
|---|---|---|---|---|
| H-01 | `sonic-rs` 0.5.10 (+ `sonic-simd`, `sonic-number`, `faststr`, `ahash`): SIMD JSON with heavy internal `unsafe`, parsing **all untrusted server responses and file input** | `Cargo.toml` (`sonic-rs = "0.5.8"`); used in `retrieval/extract.rs`, `pipeline/bulk.rs`, `http/info.rs` | upstream choice, kept by the rewrite | Run cargo-geiger, fuzz the call sites under ASan (TST-API-084), review its RustSec/issue history. Consider `serde_json` + `RawValue` on the cold paths |
| H-02 | Third-party build scripts and C code compiled in: `ring` 0.17.14 (C/asm), `zstd-sys` 2.1.2 (bundled zstd C) | Cargo.lock | transitive | Read once; record the reviewed versions in cargo-vet; build offline in a network-less sandbox (SUP-009) |
| H-03 | `const-random-macro` 0.1.16, a proc macro that calls `getrandom` **at compile time**, pulled via `rust-ini → ordered-multimap → dlv-list`. It is used only under `cfg(not(feature = "std"))`, and `std` is enabled, so the binary is unaffected | `dlv-list-0.5.2/src/lib.rs:2254` | rewrite (`rust-ini` added in M1) | Replace `rust-ini` with a ~30-line `user=`/`password=` parser (drops 5+ crates incl. this proc macro), or accept with a vet note |
| H-04 | Young or less-known crates on the graph: `zmij` 1.0.23 (float formatting, via `serde_json` 1.0.154; build.rs runs `rustc --version`), `faststr`, `munge`/`rancor` (inactive) | Cargo.lock | transitive | Confirm crates.io owners (expected: dtolnay for `zmij`, cloudwego for `faststr`) and record in cargo-vet |
| H-05 | `Cargo.lock` lists optional crates that are **not compiled** on Linux (`quinn`/HTTP3 via reqwest, `rkyv`/`uuid` via faststr, `rand` 0.10, `wasm-bindgen`, `defmt`), and `deny.toml` uses `all-features = true`, so SBOMs and audits over-report; also `reqwest` feature `http2` is enabled although ES/OpenSearch REST is HTTP/1.1 | `Cargo.toml:32-36`, `deny.toml` `[graph]` | rewrite | Drop `http2` if not needed (smaller attack surface); generate SBOMs from the resolved Linux graph; keep `all-features` for advisories only |
| H-06 | The upstream retrieval and output code (~4.2k lines, AI-written per upstream): grep found nothing hostile, but it contains the concurrency core (PIT generation coordinator, worker threads, `catch_unwind`, signal handling with `process::exit(130)`) | `src/retrieval/*`, `src/output.rs` | upstream | Line-by-line review with a checklist (SUP-012); BRK-NET-002 and SIG-* target this code |
| H-07 | Upstream benchmark script (1,506 lines) is long for what it does | `scripts/benchmark/compare-with-elasticdump.sh` | upstream | Read in full before running it anywhere with credentials; run it under the egress sandbox (SUP-001) |
| H-08 | Silent trust fallback: when the system store has no usable certs, the bundled Mozilla roots are trusted, with only a debug log | `src/http/tls.rs:70-79` | rewrite | Decide: warn at info/warn level, or require `--ca` in that case |
| H-09 | Upstream feature flags `large_scale_test`, `manual_test` gate tests that write to a live cluster at `localhost:9200`, and `test_create_data_for_manual_testing` | `Cargo.toml` `[features]`; `tests/integration_test.rs:910,1645` | upstream | Move to the `load` tier (INF-001) or delete; they are not in the shipped binary |
| H-10 | `RUST_LOG=trace` turns on dependency logging (hyper, h2, rustls, reqwest); header values are marked sensitive (`client.rs:121,136`), but this was not verified end to end | `src/http/client.rs` | rewrite | Include `RUST_LOG=trace` in the canary matrix (BRK-CLI-007) |

**Counts:**
* Confirmed: **14** (High 0, Medium 5, Low 8, Info 1). By origin: 11 rewrite, 1 upstream, 2 both (F-11, F-14).
* Needs a human look: **10** (H-01…H-10).

### 7.2 Requirements

| ID | Pri | Requirement | Acceptance criteria | Harness change | Effort |
|---|---|---|---|---|---|
| TST-SUP-001 | must | **Egress-denied test runs:** run the tool (from the built image) on a podman `--internal` network whose only other members are the cluster container(s) and a logging DNS/TCP sink. Every T1/T2/T7 scenario runs this way at least nightly. Any connection or DNS query to anything other than the named endpoints fails the run | A planted test that resolves `example.com` makes the job fail. The normal suite passes with 0 unexpected connections | `scripts/sup/egress.sh` (podman network + `dnsmasq --log-queries` sink + nft counters) | M |
| TST-SUP-002 | must | **Syscall and file-access tracing:** `strace -f -e trace=%file,%network,%process -o …` around a representative command matrix (dump/restore/split/meta/TLS). Assertions: no `execve` beyond the binary; `connect` only to allow-listed addresses; `openat` only within the allow-list (input, output dir, staging, cert files, system CA dirs, `/proc/self`, `/etc/{hosts,resolv.conf,nsswitch.conf,localtime}`, dynamic-loader paths) | The allow-list diff is empty; a new path requires a reviewed allow-list change | `scripts/sup/trace.sh` + allow-list file | M |
| TST-SUP-003 | should | **Enforcement profile:** ship an optional seccomp profile (no `execve` after start, no `ptrace`, `mount` etc.) and a Landlock wrapper (read-only everywhere except the output directory), and use them in CI runs of the image | Suite passes under the profile; a test binary that tries `execve` or writes outside is blocked | `contrib/seccomp.json`, `podman --security-opt` | M |
| TST-SUP-004 | must | **cargo-vet** (preferred) or cargo-crev: import audits from Mozilla, Google, Bytecode Alliance and Embark; every crate on the Linux graph is audited or exempted with a reason; unaudited new crates or versions block merge | `cargo vet` green in PR CI | `supply-chain/` directory | L (initial), S (ongoing) |
| TST-SUP-005 | must | **cargo-deny tightening:** keep sources (crates.io only, no git/path); `multiple-versions = "deny"` with an explicit skip list; `[bans.build]` with `allow-build-scripts` allow-list and `executables = "deny"`; deny list for crates we never want (e.g. `openssl-sys`, `git2`, HTTP clients other than reqwest/hyper) | `cargo deny check` green; adding a build-script crate fails until allow-listed | `deny.toml` | S |
| TST-SUP-006 | must | **Unsafe inventory:** `#![forbid(unsafe_code)]` in `src/lib.rs` and `src/main.rs` (0 today); `cargo geiger` report per crate with a baseline; an increase in used-unsafe count fails PR CI | Baseline committed; diff gate works | CI job | S |
| TST-SUP-007 | should | **Unused dependencies and features:** `cargo machete` on PR, `cargo udeps` nightly; `cargo hack --each-feature` / feature-usage report; justify every enabled feature of reqwest, tokio, rustls, regex | 0 unused deps; feature list documented | CI jobs | S |
| TST-SUP-008 | should | **Dead and unreachable code:** clippy `-D warnings` already denies `dead_code`; add coverage (`cargo llvm-cov`) across T0–T2 and list never-executed functions for review each release; remove flags or features with no purpose (H-09) | Uncovered-function report reviewed per release | CI job | S |
| TST-SUP-009 | must | **Build-time behaviour review:** inventory of all build scripts (45) and proc macros (18) in the lock graph, with the subset active on Linux; each reviewed at a pinned version; **builds run offline in a network-less sandbox** (`cargo fetch` first, then `podman run --network none … cargo build --offline --locked`) so build scripts cannot reach the network | CI release build runs under `--network none`; the inventory diff gate fires on new build scripts or proc macros | `scripts/sup/offline-build.sh` | M |
| TST-SUP-010 | must | **Reproducible builds:** two builds in different directories, at different times and as different users, with `SOURCE_DATE_EPOCH`, `--remap-path-prefix`, `CARGO_INCREMENTAL=0`, the same toolchain image (by digest) → identical SHA-256 for the binary; images built with `--timestamp` must match too; embed the dependency list with `cargo auditable` | Bit-for-bit equal in CI | `scripts/sup/repro.sh` | M |
| TST-SUP-011 | must | **SBOM diff per PR:** CycloneDX from `cargo cyclonedx` (resolved Linux graph) and from the image (syft); diff against the base branch (crates added/removed/upgraded, licences, new build scripts, new proc macros, unsafe deltas, image packages); attach as an artefact; a non-empty diff requires a reviewer label | Diff artefact on every PR; label gate | CI job | M |
| TST-SUP-012 | must | **Hostile-code review of upstream-origin code:** line-by-line review of the ~4.2k upstream `src/` lines plus upstream tests and scripts against a checklist (I/O targets, spawned work, error swallowing, `catch_unwind`/`exit` paths, data loss on cancellation, logging of secrets); recorded in `REVIEW.md` with the commit; any later change to those lines requires re-review (CODEOWNERS) | Review record exists; CODEOWNERS rule active | doc + CODEOWNERS | M |
| TST-SUP-013 | must | **CI hardening** (fixes F-01, F-02, F-03, F-09, F-10): SHA-pinned actions; least-privilege `permissions`; no `curl \| sh`; pinned tool versions with checksums; inputs via `env`; `persist-credentials: false`; no caches restored for fork PRs; optional egress audit for runners (e.g. harden-runner in audit mode) | Lint (`zizmor` or `actionlint` + custom rules) green | workflow edits + lint job | S |
| TST-SUP-014 | must | **New-dependency policy:** CODEOWNERS on `Cargo.toml`, `Cargo.lock`, `deny.toml`, `.github/`, `scripts/`, `Dockerfile`; a PR template checklist (purpose, alternatives, maintainers/owners, age, downloads, `unsafe`, `build.rs`, proc macros, network/file access, licence); Dependabot/Renovate with grouped updates and a **cooldown (≥ 7 days)** before adopting new releases | Policy doc; bot config; CODEOWNERS enforced | docs + config | S |
| TST-SUP-015 | should | **Secret scanning and canaries:** gitleaks/trufflehog on history and PRs; the BRK-CLI-007 canary matrix runs in T7 | 0 findings; canary job green | CI job | S |
| TST-SUP-016 | should | **Release-candidate behaviour diff:** for every RC, run SUP-001/002 and compare the syscall/egress profile with the previous release; any new syscall class, path or destination needs sign-off | Profile diff artefact | script | S |
| TST-SUP-017 | should | **Image hardening tests:** non-root (exists), read-only root filesystem run (`--read-only` plus a writable `/data`), no extra binaries beyond the documented set, base images by digest, image SBOM diff (SUP-011) | `make test-image` extended | Makefile | S |
| TST-SUP-018 | must | **Fix-and-test the audit findings F-01…F-14** and re-run the audit (this §7.1) each release with the same commands, recording results | Each finding has a linked fix or an accepted-risk note and a regression check | tracking table | M |
| TST-SUP-019 | could | Publisher/ownership monitoring: `cargo supply-chain publishers` snapshot diffed weekly; alert on crate ownership changes | Weekly report | CI cron | S |

### 7.3 Native code and `unsafe` inventory (read-only, 2026-10-29)

**Method.** cargo-geiger was **not** run. It compiles the whole graph. Doing that from a copy of the repo with its own `CARGO_HOME`/`CARGO_TARGET_DIR` would have been allowed, but it would have competed for CPU and RAM with the process using the shared box. I used `rg` over the unpacked sources of the exact locked versions instead. The 196 crates come from `cargo tree --offline --locked -e normal,build --target x86_64-unknown-linux-gnu`, and the sources are in `~/.cargo/registry/src/index.crates.io-*/` (Appendix B). The counts are **text matches across the whole crate directory, including the crate's own tests and benches**. Use them to rank crates, not as exact figures. SUP-006 asks CI to replace them with exact geiger numbers.

**Main project (`src/`, `tests/`, `benches/`): no non-safe Rust.** Running `rg -n '\bunsafe\b|extern\s+"|asm!|transmute|static mut|no_mangle|from_utf8_unchecked|\*const |\*mut ' src tests benches` finds **0 matches**. There is no `build.rs`, no FFI and no `#![forbid(unsafe_code)]` attribute (SUP-006 adds it). Nothing in the project's own code needs an individual review. All unsafe code in the binary comes from dependencies.

**Absent on purpose (checked):**
- openssl/openssl-sys/native-tls: only `openssl-probe` is present, and it just looks up certificate file paths in pure Rust.
- aws-lc-rs/aws-lc-sys, libz-sys/libz-ng-sys/zlib-ng, bindgen, cmake: none of them is in the Linux graph.

The only **C/asm compiled into the binary** comes from **zstd-sys** and **ring**, both built with `cc` (with `jobserver` and `find-msvc-tools` as build dependencies).

| Crate (locked) | Kind of non-safe code (unsafe / extern "C" / asm! / transmute; C or asm files) | Why it is in the graph | Hot path for user data? | Pure-safe or safer alternative | Recommendation |
|---|---|---|---|---|---|
| **zstd-sys 2.1.2+zstd.1.5.7** | **Bundled C zstd 1.5.7**: 43 C/asm files, including `huf_decompress_amd64.S`; 494 `extern` declarations; 8 `no_mangle`. `pkg-config` feature is off, so the bundled source is always built | `zstd 0.14` → `zstd-safe 8.2` (std only) for `.zst` files | **Yes.** It decompresses **untrusted `.zst` input** on restore and compresses every zstd dump | **ruzstd** (pure Rust, decoder is mature; its encoder only does the fastest level) | Keep the C library for compression. Fuzz decoding of hostile `.zst` input and cap the window size (SUP-020). Evaluate ruzstd for **decode only** (restore path); also hold the `zstd-sys` version via cargo-deny. |
| **ring 0.17.14** | 230 unsafe; 37 extern; **145 C/perlasm/pregenerated asm files** | rustls crypto provider, chosen with reqwest `rustls-tls-manual-roots` | **Yes for the network:** every TLS byte to and from the cluster. It does not parse JSON | aws-lc-rs (more C, FIPS option; not safer); `rustls-rustcrypto` (pure Rust, not production-grade yet); graviola (Rust plus asm, young) | Keep ring. It is the most audited option for rustls and smaller than aws-lc. Pin it in cargo-vet (`safe-to-deploy`) and watch RUSTSEC. |
| **sonic-rs 0.5.10** (+ sonic-simd 0.1.4, sonic-number 0.1.3, faststr 0.2.34, bumpalo 3.20.3, ahash 0.8.12 → zerocopy 0.8.63, simdutf8 0.1.5) | 251 + 91 + 28 + 63 + 235 unsafe; SIMD intrinsics; `LazyValue::as_raw_str` uses `from_utf8_unchecked` (`lazyvalue/value.rs:351-355`) | Main JSON engine: `Top::parse`, `extract_batch`, `get`, iterators | **Yes, the hottest path.** Every hit and file line goes through it | **serde_json** with `RawValue` (20 unsafe matches, mostly itoa/ryu/zmij); simd-json has just as much unsafe | **Highest-priority review.** I checked UTF-8 handling: for `&[u8]` input, `Read::new_in` finds the first invalid UTF-8 byte up front and the iterators and `get` return an error before handing out values (`reader.rs:170-176`, `iterator.rs:91-113`, `get.rs:398-412`), so `as_raw_str` is sound as used. Remaining risk is SIMD out-of-bounds reads on adversarial lengths and lazy-skip divergence. Add a sonic-vs-serde_json **differential fuzz** (SUP-022). Keep a serde_json fallback build so the speed cost can be measured. |
| **flate2 1.1.10 → zlib-rs 0.6.8** (+ miniz_oxide 0.9.1, adler2, crc32fast, simd-adler32) | zlib-rs: 456 unsafe, `asm!` only in the aarch64 crc32 code, no C. miniz_oxide and adler2 are `#![forbid(unsafe_code)]` | gzip files and HTTP gzip (`compression-codecs`) | **Yes.** It inflates untrusted `.gz` input and HTTP responses | **miniz_oxide** (`flate2` `rust_backend`, 0 unsafe, slower; measure before switching) | zlib-rs already replaces zlib-ng/C zlib and is fuzzed upstream. Keep it, include it in the decompression-bomb tests (BRK-RES), and record miniz_oxide as the zero-unsafe fallback. |
| hyper 1.12 / h2 0.4.20 / httparse 1.10 / http 1.5 / bytes 1.12 / hyper-util | 67 / 2 / 76 / 65 / 171 unsafe | HTTP client stack | **Yes.** It parses every server response header and body chunk | none with the same features | Keep. **Turn off the reqwest `http2` feature** (H-05) to remove h2 from the graph. Responses are already fully buffered. |
| tokio 1.54 / mio 1.2 / socket2 0.6 / libc 0.2.191 / rustix 1.1 / linux-raw-sys 0.12 / signal-hook-registry / errno | syscall FFI and raw fds. libc has 512 extern; linux-raw-sys's 4,412 transmute matches are generated constants | runtime, sockets, signals; rustix comes in through clap → terminal_size | Indirectly (I/O) | none | Keep; these are the standard ecosystem crates. |
| memchr 2.8 / aho-corasick 1.1 / regex-automata 0.4 | SIMD unsafe (340 / 227 / 57) | line splitting (`memchr` throughout `pipeline/`), `--renameIndex`/template regexes | **Yes** (memchr on every buffer) | none worth having | Keep; widely fuzzed and audited. |
| hashbrown 0.17 and 0.14, indexmap, smallvec, once_cell, slab, futures-*, pin-project-lite, tokio-util, try-lock, atomic-waker | collection and async unsafe | std-like plumbing; hashbrown 0.14 comes only via rust-ini → ordered-multimap | Indirectly | — | Keep. Removing rust-ini (Q10) also removes hashbrown 0.14 and dlv-list. |
| serde_json 1.0.154 → zmij 1.0.23, itoa, ryu | 20 / 70 (7 `asm!`) / 13 / 33 | meta types (`codec.rs`), float formatting | Yes for meta, partly for data | — | Keep. Review the zmij publisher (H-04). |
| icu_* 2.1, zerovec, yoke, tinystr, zerotrie, potential_utf, idna | zero-copy unsafe (zerovec 254, yoke 129) | IDNA host handling in `url` | Only for hostnames | `idna` `compiled_data` cannot be avoided | Keep. |
| getrandom 0.2 and 0.3 | syscall FFI (`getrandom(2)`) | hash seeds (ahash), rand | No | — | Keep. |
| aes 0.8, scrypt 0.11, pbkdf2, sha2 0.10, salsa20, cbc, pkcs5 (via `pkcs8` feature `encryption`) | RustCrypto; aes and sha2 use `asm!`/intrinsics (aes 7, sha2 10) | **only** to decrypt an encrypted PKCS#8 client key | No (runs once at startup) | — | Keep, but **feature-gate `encryption`** so default builds leave it out unless encrypted keys are needed (SUP-023). |
| portable-atomic 1.15 (via indicatif), jiff 0.2 (via env_logger), console, terminal_size | asm and unsafe | progress bar, log timestamps, terminal width | No | indicatif features / plain stderr counter | Could-have: trim these to cut non-hot unsafe (SUP-023). |
| anyhow 1.0.104 | vtable unsafe; its build.rs probes rustc (read earlier, no network) | errors | No | thiserror only | Keep. |
| rustls 0.23.46 / rustls-webpki 0.103 / rustls-pki-types / untrusted | 5 / 0 / 1 / 0 | TLS state machine and certificate checks | Yes (network) | — | Already almost entirely safe; keep. |

**Summary:**
- **Native C/asm** comes from two crates: **zstd-sys** (bundled zstd, which handles untrusted input) and **ring** (TLS crypto).
- **Unsafe on the user-data hot path** is concentrated in **sonic-rs** and its helper crates, then zlib-rs, memchr, hyper/httparse and bytes.
- **The main project has 0 unsafe.**
- Highest-value actions:
  1. sonic-rs vs serde_json differential fuzzing;
  2. fuzzing zstd decoding with a window cap;
  3. dropping `http2`;
  4. feature-gating PKCS#8 encryption;
  5. a geiger baseline gate in CI (SUP-006).

#### 7.3.1 Additional requirements from the inventory

| ID | Priority | Requirement | Acceptance | Harness | Effort |
|---|---|---|---|---|---|
| TST-SUP-020 | must | **Hostile compressed input:** cargo-fuzz targets for `.zst` and `.gz` decoding as used by `file_source` (truncated frames, huge declared window, skippable frames, concatenated frames, gzip multi-member, trailing garbage, decompression bombs at 1000:1). zstd decoding uses `window_log_max` ≤ 27 (or a configurable cap). The ruzstd decode path is benchmarked as an option | No crash, no out-of-memory beyond the cap, clear error and non-zero exit; 1 h nightly fuzz with no findings; ruzstd decision recorded | `fuzz/fuzz_targets/{zstd_in,gzip_in}.rs` | M |
| TST-SUP-021 | should | **Native-code allow-list:** cargo-deny `bans` plus a CI script fails when a new crate with `links =`, a `cc`/`cmake`/`bindgen` build dependency, or `.c`/`.S` files enters the Linux graph. The allow-list today is `zstd-sys` and `ring` | Adding e.g. `libz-sys` fails CI | `deny.toml`, `scripts/native-code-check.sh` | S |
| TST-SUP-022 | must | **sonic-rs differential fuzz:** for arbitrary bytes, `sonic_rs::to_object_iter`/`get`/`to_array_iter` accept or reject as `serde_json` does, and return the same raw value spans (`LazyValue::as_raw_str`) on valid input. Run under ASan (`cargo +nightly fuzz -s address`) | 4 h nightly with no divergence or sanitizer report; any divergence is filed upstream and pinned | `fuzz/fuzz_targets/sonic_vs_serde.rs` | M |
| TST-SUP-023 | could | **Trim non-hot unsafe:** turn off reqwest `http2` (removes h2); put `pkcs8/encryption` behind a cargo feature; review indicatif/portable-atomic and env_logger/jiff features | geiger total drops; all tests pass | Cargo.toml | S |

---
## 8. JSON, NDJSON and URL injection review (TST-INJ)

**Scope.** This section covers every place where request or output JSON is assembled **by hand**, meaning raw byte splicing, `format!` or `push_str` rather than serialising a value. It also covers every place where a name ends up in a URL. I read the code only; nothing was built or run. Search used: `rg -n 'format!\(.*[\{]\\?"|format!\(r#|push_str\(|extend_from_slice\(b"|concat!\(' src`. Outside tests it matches only `pipeline/bulk.rs` (the bulk encoder) and an error-text helper in `http/retry.rs:239`.

**Paths checked and found serde/structured (safe):**

| Path | Code | Verdict |
|---|---|---|
| Search, scroll and PIT bodies | `retrieval/search_body.rs:25-28` parses `--searchBody`; `pit_search.rs:19-60` sets `pit` and `search_after` with `json!` and `sonic_rs::from_slice` (the raw `sort` from the server is reparsed, not spliced) | Safe. A hostile `sort` value can only become a `search_after` value |
| Meta types: settings, mappings, templates, aliases, policies, scripts; double-encoded lines | `meta/codec.rs` (serde_json encode/decode), `meta/alias.rs:72,89,121` (`json!`), `meta/policy/*` ILM→ISM on `serde_json::Value`, `lib.rs:282` | Safe from string breakout. Names still reach URL paths (INJ-F2) |
| `--renameIndex` substitutions | `rename.rs:122-140` regex replace, then `validate_index_name` **when the name changed** | The JSON is safe (the name goes into `push_json_str`/`json!`). Path issue: see INJ-F2 |
| `--headers` / auth | `http/client.rs:119,133` `HeaderValue::from_str` | **CRLF is rejected** by the `http` crate (it refuses bytes < 0x20 other than tab, and 0x7F), so header injection is blocked. Needs a regression test (INJ-011) |
| `--params`, scroll and PIT ids in query strings | `client.rs:197-205` `query_pairs_mut().append_pair` | Encoded, so it is safe |
| `_index`, `_type`, `version_type`, output index, prefix/suffix in the bulk action | `bulk.rs:275,289,315` `push_json_str` (sonic `to_writer`) | Escaped, so it is safe |

### 8.1 Findings: confirmed by code (exploitable)

**INJ-F1. High. Pretty-printed `_source` from the cluster is passed through without flattening, so the NDJSON framing breaks and a document can inject other bulk writes.** *(Origin: upstream `extract_batch` raw-hit design plus rewrite `BulkSink::process_batch`; confirmed from the code. The final live reproduction needs a container, which I was not allowed to start.)*

- `retrieval/extract.rs:231` writes `hit.as_raw_str()` **verbatim** and then `\n`. Elasticsearch and OpenSearch return `_source` byte for byte as it was indexed, including any newlines between tokens. Nothing on the retrieval side calls `single_line` (it is used only in `pipeline/file_source.rs:89`).
- `pipeline/bulk.rs:379-382` (`process_batch`) splits the batch on every `\n` and encodes each fragment as its own document. `Top::parse` (`bulk.rs:114-119`) uses `filter_map(|m| m.ok())`, so a **truncated fragment still yields** the members read before the error.
- `encode_doc` (`bulk.rs:270-279`) treats any fragment with `_id` or `_source` as a hit. Its `_index` (when no output index is set), `_id`, `_routing`/`_parent`/`version` (`meta()`, `bulk.rs:133-145`) and `_source` all come **from the fragment**.

*PoC (cluster→cluster copy, or a dump that is later restored).* An attacker who can index one document into the source index sends it pretty-printed. Any client does this; Kibana Dev Tools does it by default:

```
PUT src/_doc/x
{"a":
{"_index":"victim","_id":"admin","_source":{"role":"superuser"}}
}
```

The scroll/PIT response then contains `{"_index":"src","_id":"x",…,"_source":{"a":\n{"_index":"victim",…}\n}}`. Running `elasticdump-rs --input=http://a:9200 --output=http://b:9200 --type=data` (host-only multi-index copy) produces:

1. Fragment 1, `{"_index":"src","_id":"x","_score":1.0,"_source":{"a":`: `_source` fails to parse and is dropped, so the bulk request gets `{"index":{"_index":"src","_id":"x"}}` + `{}`. **Document x is overwritten with an empty document.**
2. Fragment 2: `{"index":{"_index":"victim","_id":"admin"}}` + `{"role":"superuser"}`. **This writes into an index the attacker chose.** With `--output-index dst`, it overwrites any `_id` in `dst`. With `--bulkAction delete`, it **deletes `victim/admin`**.
3. Fragment 3, `}}`: a plain doc with body `}}`. The item fails, but the run **still exits 0** (SRV-001).

Same root cause in file outputs:
- dump files contain multi-line records;
- doc counts are wrong (`retrieval/mod.rs:344`, `pipeline/sink.rs:43` count `\n`);
- `--fileSize` can cut a record across parts (`file_sink.rs:336`), and restoring the second part alone replays fragment 2;
- `--offset`/`--size` windows count lines (`pipeline/window.rs:40,63`);
- Node elasticdump cannot read the file;
- `--sourceOnly` (`sink.rs:15-35`) has the same problem.

*Fix:*
- In `extract_batch`, run `if memchr2(b'\n', b'\r', raw).is_some() { single_line(raw) }`. This is cheap because the common case is a single memchr miss. Do the same in `source_only_lines`.
- In `process_batch`, **reject** (count as failed, exit non-zero) any line that is not exactly one complete JSON object. Do not encode partial `Top` members: replace `filter_map(ok)` with "any error → reject the line".

**INJ-F2. Medium. Index names read from a file reach URL paths without validation: `*`, `_all` and `a,b` act on many indices, and `.`/`..` are silently dropped from the path.** *(Origin: rewrite.)*

- `rename.rs:131` validates the name **only when it changed** (`out != name || rule.is_some()`). Without `--prefix`, `--suffix` or a rename, a name taken from the dump file is used as it is (`meta/mod.rs:122-130`).
- `http/client.rs:188-196` pushes the name as one path segment. The `url` crate percent-encodes `/ ? # %` correctly, but `PathSegmentsMut::extend` **skips `"."` and `".."` completely** (`url-2.5.8/src/path_segments.rs:247`). So `PUT /{..}/_settings` becomes `PUT /_settings`.

*PoC (analyzer restore):* a file containing `{"*":{"settings":{"index":{"analysis":{…attacker analyzer…}}}}}`, restored with `--type=analyzer --closeIndex --output=http://b:9200`:
- `exists(["*"])` (`index_scoped.rs:428`) is true when any index exists;
- `close_put_open` sends **`POST /*/_close`**, then `PUT /*/_settings`, then `POST /*/_open` (`index_scoped.rs:466-490`).

On OpenSearch and on Elasticsearch 7.x (`action.destructive_requires_name` defaults to false), this **closes every index and rewrites its analysis settings**. ES 8+ refuses the wildcard close.

Same idea for aliases: `{"*":{"aliases":{"logs-read":{}}}}` → `POST /_aliases {"actions":[{"add":{"index":"*","alias":"logs-read"}}]}` adds the alias to **every index**. This can expose other tenants' data through an alias that applications read from.

Mapping and settings restores are **protected by accident**: they first `PUT /{name}`, which fails for `*`, `_all` and `,`, and that aborts the run. A name of `..` turns into `PUT /`, which is also an error.

*Fix:*
- Call `validate_index_name` on **every** name read from a file or server response before using it in a path or action, renamed or not;
- reject `*`, `?`, `,`, `_all`, empty, `.`, `..` and names with a leading `-_+`;
- make `client.url` return an error for `.`/`..`/empty segments instead of dropping them;
- `close_put_open` must only ever get a single concrete index name.

**INJ-F3. Low. Raw-spliced action fields are not type-checked** (`bulk.rs:279` `_id`, `bulk.rs:304-306` meta fields, `bulk.rs:312` `version`). *(Origin: rewrite.)*

A file doc `{"_id":{"$x":1},"_routing":[1,2],"_version":"9e99","_source":{}}` produces the action `{"index":{"_index":"o","_id":{"$x":1},"routing":[1,2],"version":"9e99"}}`. sonic validated each value, so it is one balanced JSON value. **That means no breakout and no sibling-key injection.** However, Elasticsearch fails the **whole bulk request** with a 400 parse error, so one bad document takes down the other 999 documents in its batch. Combined with whole-batch retry and BRK-NET-003, this is a cheap way to deny service to a restore.

*Fix:*
- `_id` and `_routing`/`_parent` must be JSON strings (numbers are converted with `to_string`);
- `version` must be an integer within u64;
- anything else is rejected per document.

**INJ-F4. Low (operator-controlled). `--parseExtraFields` names are spliced into the action line without escaping** (`lib.rs:221-226` → `bulk.rs:304-306`, which writes the name bytes raw). *(Origin: rewrite.)*

`--parseExtraFields 'x":1,"_index":"other'` injects keys into every action. Only someone who controls the command line can do this, so it is not a privilege boundary, but it can happen through scripts that build CLI args. *Fix:* require names to match `^[a-z][a-z0-9_]*$`, de-duplicate them against the defaults, and write them with `push_json_str`.

### 8.2 Defensible, but worth hardening

| # | Behaviour | Where | Why it is OK today | Hardening |
|---|---|---|---|---|
| H-INJ-1 | Duplicate top-level keys: **first wins** (`Top::get`, `bulk.rs:122-124`); Node's `JSON.parse` uses last-wins | bulk | Cannot break out of a string. But `{"_index":"a","_index":"victim"}` goes to `a` in Rust and to `victim` in Node, so a file can be built to behave differently in the two tools | Reject duplicate meta keys (INJ-008) |
| H-INJ-2 | `_source` is spliced raw without checking it is an object | `bulk.rs:541-548` | Within one line it cannot contain `\n` once INJ-F1 is fixed; ES rejects non-objects per item | Check that it is exactly one object (INJ-009) |
| H-INJ-3 | File input with a raw control byte (CR, NUL) inside a string | `json_split.rs:141` keeps the bytes inside strings | ES rejects it per item. A raw LF splits the value only in output that does not go through `file_source`, which joins values with the JSON-aware splitter | Reject during splitting with a clear error (BRK-IN-005) |
| H-INJ-4 | U+2028/U+2029, astral characters, `\ud800` escapes, big integers in `_id`/`version` | splicing keeps the original text | All valid JSON; ES handles them. A big `version` causes INJ-F3 | Covered by the escaping corpus (INJ-014) |
| H-INJ-5 | Invalid UTF-8 | sonic validates up front (§7.3) | The whole line is rejected | Count it and exit non-zero (SRV-001) |
| H-INJ-6 | A dump file decides target indices when no `--output-index` is given (`index_for`, `bulk.rs:250-260`) | by design, same as Node | Expected behaviour | `--allow-index` pattern plus a dry-run list of target indices for untrusted files (INJ-013) |
| H-INJ-7 | `--output-index ..` or an index decoded from `%2e%2e` in the URL path → `/_bulk` without an index | `endpoint.rs` decode + INJ-F2 | Only the operator can do this | Validated by the INJ-F2 fix |

### 8.3 Requirements

| ID | Priority | Requirement | Acceptance | Harness | Effort |
|---|---|---|---|---|---|
| TST-INJ-001 | must | **Hits are single-line:** a pretty-printed `_source` (LF, CRLF and tabs between tokens, and inside nested arrays) dumped from ES 7/8/9 and OpenSearch 1/2/3 produces exactly one line per hit in file, stdout and `--sourceOnly` output | Lines == hits; every line parses alone; Node can read the file | IT + MockEs golden | S |
| TST-INJ-002 | must | **Framing PoC regression (INJ-F1):** the PoC in §8.1, run cluster→cluster host-only, with `--output-index`, and with `--bulkAction delete` | `victim` is unchanged; `x` has its source byte-for-byte; no extra bulk items; exit code is 0 only if every write succeeded | IT (`it_injection.rs`) | S |
| TST-INJ-003 | must | **A bulk line is all or nothing:** a line that is not exactly one complete JSON object is never encoded (no partial `Top`) | Truncated or `}}` lines are counted as failed with a message and the exit code is non-zero | unit + MockEs | S |
| TST-INJ-004 | must | **Encoder differential proptest:** for arbitrary docs (`proptest` strategies over keys, escapes and value types), the output of `encode_doc` parsed as NDJSON by serde_json has exactly 2 lines per doc (1 for `delete`); action keys ⊆ {`_index`, `_id`, `_type`, `routing`, `parent`, `version`, `version_type`, extra fields}; `_index` equals the expected target; and it equals a reference encoder built with serde_json | 100k cases per CI run with no mismatch | `tests/prop_bulk.rs` | M |
| TST-INJ-005 | must | **Action field types (INJ-F3):** `_id`/`_routing`/`_parent` must be strings (numbers are converted); `version` must be an integer ≤ u64::MAX; anything else is rejected per doc | One bad doc never causes a 400 for the whole bulk request | unit + MockEs | S |
| TST-INJ-006 | must | **Validate every index name read from a file or server (INJ-F2):** `*`, `?`, `,`, `_all`, `""`, `.`, `..`, leading `-_+`, upper case, >255 bytes in data `_index` and all meta-type keys (index, settings, mapping, analyzer, alias), with and without rename | Clear error; **no HTTP request sent** (MockEs records nothing); exit non-zero | unit + MockEs | S |
| TST-INJ-007 | must | **URL segments never vanish:** `client.url` returns an error for `.`, `..` and empty segments; property test: number of path segments in == out for any input; `/`, `?`, `#`, `%`, space and Unicode are percent-encoded | Property test passes; golden URLs for every endpoint E1–E20 | `tests/prop_url.rs` | S |
| TST-INJ-008 | must | **Duplicate meta keys** (`_index`, `_id`, `_routing`, `_source`) in a hit are rejected (or follow Node's last-wins as decided in Q13) | Behaviour matches the decision; Node comparison test | unit + Node interop | S |
| TST-INJ-009 | must | **Raw `_source` validation:** exactly one JSON object; no trailing data; no raw control bytes in strings | Rejected per doc with a reason | unit + fuzz | S |
| TST-INJ-010 | must | **Record framing in file output:** `--fileSize`/`--fileCount` parts never split a record; `--offset`/`--size` count records; the progress count equals the number of records | Digest of the concatenated parts == digest of the unsplit dump at L0 with the `edge` profile, which includes pretty-printed sources | LOAD harness | S |
| TST-INJ-011 | must | **Header injection guard:** `--headers` values or names containing `\r`, `\n`, `\0` or non-visible ASCII → usage error | Usage-error exit code; the request is never sent; the secret is not echoed | cli_contract | S |
| TST-INJ-012 | should | **Query encoding:** `--params` values with `&`, `=`, `#`, `%`, `+`, Unicode; scroll and PIT ids with `=` padding are round-tripped exactly | Golden URLs; MockEs sees the decoded values unchanged | unit | S |
| TST-INJ-013 | should | **Target allow-list and dry run** for untrusted restores: `--allow-index '<pattern>'`, plus `--dry-run` listing the target indices and actions | A write outside the pattern is rejected before any request | new flags (Q13) | M |
| TST-INJ-014 | must | **Escaping corpus round-trip:** ids, names and values with `"`, `\`, control escapes, U+2028/U+2029, astral characters, lone-surrogate escapes, 2^64+1 and -0.0, 1e400 are identical byte for byte through file→ES→file, and match Node | Multiset digest equal; differences from Node are documented | edgen `edge` profile + IT | M |
| TST-INJ-015 | must | **Fuzz targets** (cargo-fuzz, nightly 1 h each, seeds from the corpus): `fuzz_encode_doc` (oracle INJ-004); `fuzz_file_to_bulk` (bytes → `ValueSplitter` → `single_line` → `process_batch` → NDJSON oracle: lines == 2 × accepted docs and every target is in the allowed set); `fuzz_extract_batch` (search response with arbitrary whitespace and escapes inside `_source` → each output line is exactly one JSON value); `fuzz_url_segments`; `fuzz_rename_output` (every rename output either passes `validate_index_name` or is rejected); `fuzz_meta_names` | No crash and no oracle violation; findings become unit tests | `fuzz/` | M |
| TST-INJ-016 | should | **`--closeIndex` safety:** `close_put_open` is only ever called with one concrete existing index name; MockEs asserts that no `_close` path contains `*`, `,` or `_all` | Guard test passes on ES 7.17 and OS 2.x live | MockEs + IT | S |
| TST-INJ-017 | could | **Node oracle on the injection corpus:** run Node elasticdump on the same PoC files and record its behaviour (Node `JSON.stringify`s hits, so INJ-F1 should not happen there) | Comparison table in TESTING-MATRIX.md | interop harness | S |

**Fuzz and proptest targets added by this section:**
- proptest: `prop_bulk` (INJ-004) and `prop_url` (INJ-007);
- cargo-fuzz: `fuzz_encode_doc`, `fuzz_file_to_bulk`, `fuzz_extract_batch`, `fuzz_url_segments`, `fuzz_rename_output` and `fuzz_meta_names` (INJ-015);
- cargo-fuzz from §7.3: `zstd_in`, `gzip_in` (SUP-020) and `sonic_vs_serde` (SUP-022).

Together with the 10 targets in MT-3, this makes **19 fuzz targets** in the nightly job.

**Top fixes, in order:**
1. INJ-F1: flatten hits in `extract_batch` and make `process_batch` reject partial lines;
2. INJ-F2: validate every file-derived name and make `client.url` refuse to drop segments;
3. INJ-F3: type-check spliced action fields;
4. INJ-F4: validate `--parseExtraFields` names.

All four are small changes, and their regression tests are INJ-002, INJ-006/007, INJ-005 and INJ-004.

---
## 9. Milestones

| Milestone | Contents | Estimate (1 engineer) | Exit criteria |
|---|---|---|---|
| **MT-0 Foundations** | INF-001, 002, 004, 005, 006, 007, 011, 012, 014; SUP-013 (CI hardening, quick), F-04/F-11 cleanup | 2.5 weeks | Router mock and verifier in use; TESTING-MATRIX.md exists; fixtures tracked; CI pinned |
| **MT-1 Golden contracts** | INF-003; API-001…041 | 3 weeks | Goldens recorded for 9 images; replay in PR CI; drift job nightly |
| **MT-2 Error paths + top-15 chaos + injection PoCs** | API-050…056; BRK top 15 (§6.3); INJ-001…003, 005, 006, 011 (the INJ-F1/F2 regressions first) | 3 weeks | Each top-15 item is a failing test (bug confirmed) or a passing guard; fixes filed |
| **MT-3 Property and fuzz** | API-080…086; BRK-IN-* not yet covered; INJ-004, 007…010, 012, 014…017; SUP-020, 022 | 3 weeks | 19 fuzz targets nightly; proptest suites (incl. `prop_bulk`, `prop_url`) in T0 |
| **MT-4 Interop and cross-version** | API-060…072 | 2 weeks | 72-import file matrix and live pairs match the expectation table |
| **MT-5 Generator, datasets, L0/L1** | INF-008, 009; DATA-001…006, 010; LOAD-001…012, 017 | 3 weeks | `edgen` reproducible; L0 in PR, L1 nightly with digest gates |
| **MT-6 Remaining chaos** | BRK-RES, CLI, SIG, ILM, XV not yet done | 2 weeks | All TST-BRK rows implemented |
| **MT-7 Supply-chain gates** | SUP-001…012, 014…019, 021, 023; INJ-013 (in parallel with MT-1…MT-4) | 3 weeks | Egress/strace jobs, cargo-vet, repro build, SBOM diff live |
| **MT-8 Soak and trends** | LOAD-013…020; INF-010, 013; DATA-007…009 | 1.5 weeks | Weekly L2 soak on a big runner; trend gates |

**Total:** about **23 engineer-weeks**. With two parallel streams (A: MT-0 → 1 → 2 → 3 → 6; B: MT-7 → 4 → 5 → 8), about **12–13 calendar weeks**. Add fixing time for the bugs MT-2 and MT-6 are expected to find; that is not included.

## 10. Open questions for Robert

1. **CI runners:**
   * Standard GitHub-hosted runners (4 vCPU / 16 GB / ~14 GB free disk [to confirm]) for L0 and a reduced L1?
   * Larger or self-hosted runners for full L1 and L2?
   * Where should the weekly 6–12 h soak (≥ 16 vCPU, 64 GB, 1 TB) run, and who pays for it?
2. **Storage and caching:**
   * May we keep about 8 GB of L1 corpora cached on this box? 35 GB is free today and the box is shared.
   * Should the CI cache (10 GB per repo on GitHub) hold the L1 corpora?
   * L2 needs ~120 GB of corpora plus ~250 GB working space: object storage or runner-local?
   * Should datasets be mirrored, or always fetched from rally-tracks.elastic.co / CloudFront / ITA / Zenodo?
3. **Exit-code policy when documents fail to write:**
   * Today it exits 0 with `Failed Writes: N`, matching Node.
   * Options: make non-zero the default (a deviation from Node), or add `--failOnWriteErrors`.
   * This decides how TST-BRK-SRV-001/002 assert.
4. **Resume for long dumps:** add checkpointed resume (last sort values per slice for PIT/search_after) so a network blip does not restart a multi-hour dump (BRK-NET-001)?
5. **Default timeouts:** introduce a default idle/read timeout and a connect timeout (a deviation from Node, which has none)?
6. **Non-idempotent bulk retries:** for auto-id, `create` and `update` batches, should the tool stop retrying whole batches, or generate ids client-side (BRK-NET-003)?
7. **Scope:**
   * Open Distro (`_opendistro` paths, ES 7.10-based)?
   * Live ES 6.8?
   * AWS OpenSearch Service compatibility mode (`override_main_response_version`)?
   * OpenSearch Serverless?
8. **Rally / OpenSearch Benchmark** as CI dependencies (Python toolchains), or only our converters plus their corpora?
9. **Dataset licences:** may we use the following in internal testing, and may results or derived files appear in CI artefacts?
   * Loghub (CC-BY-4.0 on Zenodo, but "research or academic work" in the repo README);
   * AIT-LDS v2 (CC-BY-NC-SA-4.0);
   * Kaggle (needs an account/API token);
   * Elastic License 2.0 tooling (corpus generator).
10. **Supply chain:**
    * cargo-vet (needs ongoing audit work) or cargo-crev?
    * Who approves new dependencies?
    * May we replace `rust-ini` (H-03) and reconsider `sonic-rs` (H-01)?
    * Drop the `reqwest` `http2` feature (H-05)?
11. **Upstream relationship:** keep pulling from `tenyears-tech/elasticdump-rs` (each pull then needs SUP-012 re-review), or hard-fork?
12. **Chaos on the shared box:** run TST-BRK suites here nightly (they restart containers, fill tmpfs, kill processes), or only on CI runners?
13. **Injection semantics:** for duplicate meta keys in a hit, reject them or copy Node's last-wins behaviour (INJ-008)? Should restores from untrusted files get new `--allow-index`/`--dry-run` flags (INJ-013), or is validating names (INJ-006) enough?
14. **Unsafe and native-code budget:** accept sonic-rs on the hot path once the differential fuzz (SUP-022) is clean, or ship serde_json as the default and keep sonic behind a feature? Should ruzstd handle decoding of untrusted `.zst` input (SUP-020)?

---

## Appendix A. Link verification log (2026-10-29, `curl -s -I -L -m 25`, browser User-Agent)

**200 OK.** Size from `Content-Length` where shown.
* **Rally `http_logs` corpora:** `documents-181998.json.bz2` (13.8 MB), `-191998` (49.5 MB), `-241998` (907 MB), `-181998-1k.json.bz2` (6.9 KB).
* **Other Rally corpora:**
  * `nyc_taxis/documents.json.bz2` (4.82 GB);
  * `geonames/documents-2.json.bz2` (265 MB);
  * `nested/documents.json.bz2` (696 MB);
  * `pmc/documents.json.bz2` (5.93 GB);
  * `noaa/documents.json.bz2` (995 MB);
  * `nested/documents-1k.json.bz2`, `geonames/documents-2-1k.json.bz2`;
  * `observability/logging/nginx/demo.elastic.co/nginx.access/raw/document-0.json.zst` (10.4 MB).
* **OSB corpora (CloudFront `dbyiw3u3rf9yr`):** `big5/documents-60.json.zst` (4.31 GB), `big5/documents-100.json.zst` (7.31 GB), `nyc_taxis/documents.json.zst` (4.81 GB), `http_logs/documents-181998.json.zst` (16.1 MB), `eventdata/eventdata.json.bz2` (793 MB).
* **GitHub repos and track directories:**
  * `elastic/rally-tracks` (+ `http_logs`, `nyc_taxis`, `geonames`, `nested`, `pmc`, `noaa`, `elastic/logs` trees);
  * `elastic/rally-eventdata-track`;
  * `opensearch-project/opensearch-benchmark-workloads` (+ `http_logs`, `big5`, `eventdata` trees);
  * `logpai/loghub`, `logpai/loghub-2.0`;
  * `mingrammer/flog`;
  * `elastic/elastic-integration-corpus-generator-tool`;
  * `elastic/makelogs`, `elastic/examples` (+ `apache_logs` raw file, 2.37 MB);
  * `elastic/rally`, `opensearch-project/opensearch-benchmark`;
  * `joke2k/faker`.
* **Zenodo API:** `zenodo.org/api/records/8196385` (Loghub, CC-BY-4.0) and `zenodo.org/api/records/5789064` (AIT-LDS v2, CC-BY-NC-SA-4.0).
* **ITA traces:** `NASA-HTTP.html`, `traces/NASA_access_log_Jul95.gz` (20.7 MB), `traces/NASA_access_log_Aug95.gz` (16.6 MB), `WorldCup.html`, `traces/WorldCup/` (249 day files; `wc_day46_1.gz` 45 MB).
* **SecRepo:** `secrepo.com`, `self.logs/` and `self.logs/access.log.2017-01-01.gz`.
* **Package registries and docs:**
  * Kaggle dataset page `eliasdabbas/web-server-access-logs` (3.3 GB, CC0 per page metadata);
  * PyPI `Faker`; npm registry `makelogs` (latest 4.3.0, Apache-2.0); crates.io API `fake` 5.1.0 (MIT OR Apache-2.0);
  * docs.opensearch.org `generate-data` and synthetic-data-generation pages;
  * elastic.co Logstash generator input docs.

**Not reachable (excluded or replaced):**
* `https://zenodo.org/records/8196385` and `/records/5789064` HTML pages returned **403** to curl; they are listed via the API, which returned 200. Open them in a browser.
* `https://www.npmjs.com/package/makelogs` returned **403** to curl; the registry API was used instead.
* `https://crates.io/crates/fake` returned **404** to curl without a User-Agent (crates.io requires one); the API was used instead.
* OSB `elb_logs` corpus `d270n0h488ttok.cloudfront.net/corpora/elb_logs/documents.json.bz2` returned **403** → **excluded**.
* `ftp://ita.ee.lbl.gov/traces/NASA_access_log_Jul95.gz` did not respond (status 000) → replaced by the `https://` path.
* A guessed `secrepo.com/self.logs/access.log.2015-01-01.gz` returned 404 (wrong name; real files are listed in the directory index).
* `rally-tracks.elastic.co/nested/documents-1k.json` returned 404 (the uncompressed name does not exist; `.bz2` does).

## Appendix B. Audit commands (read-only)

```bash
cd /workspace/elasticdump-rs
git log --format='%h %ad %an %s' --date=short            # upstream = b32b6e9 and earlier
for f in $(git ls-files src); do git blame --line-porcelain HEAD -- "$f"; done | grep '^author ' | sort | uniq -c
rg -n -o 'https?://[^\s"\)<>`]+' src ; rg -n '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' src
rg -n 'Command::new|std::process|env::var|home_dir|\.ssh|\.aws|\.kube|unsafe|include_bytes|TcpStream|lookup_host' src tests
cargo metadata --offline --locked --format-version 1        # build scripts / proc-macros inventory
cargo tree --offline --locked -e normal,build --target x86_64-unknown-linux-gnu --prefix none | sort -u
cargo tree --offline --locked -e features -i dlv-list --target x86_64-unknown-linux-gnu
git status --short --ignored                                 # found the ignored Node fixture (F-04)
# §7.3 unsafe inventory (no build; locked Linux graph, registry sources)
cargo tree --offline --locked -e normal,build --target x86_64-unknown-linux-gnu --prefix none --no-dedupe | awk '{print $1"-"substr($2,2)}' | sort -u > /tmp/active.txt
cd ~/.cargo/registry/src/index.crates.io-*/ && for c in $(cat /tmp/active.txt); do rg -c --type rust '\bunsafe\b' $c; rg -c --type rust 'extern\s+"C"|asm!|transmute|static mut|no_mangle' $c; find $c -name '*.c' -o -name '*.S'; done
# §8 injection review
rg -n 'format!\(.*[\{]\\?"|format!\(r#|push_str\(|extend_from_slice\(b"|concat!\(' src
rg -n 'single_line|memchr.*b.\\n' src
```
