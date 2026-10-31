# elasticdump-rs: Implementation Plan for Rust Parity with elasticdump (`data`, `mapping`, `settings`)

> **Audience:** a coding agent or Robert, working directly from this document.
> **Starting codebase:** `tenyears-tech/elasticdump-rs` @ `b32b6e9` (last commit 2026-07-05 07:19 EDT, MIT, public).
> **Reference implementation:** `elasticsearch-dump/elasticsearch-dump` @ `600db62` (main, 2026-10-18; `package.json` version 6.125.1, Apache-2.0).
> **Written:** 2026-10-28. Facts marked **[verified]** were checked against source code or by running both tools against OpenSearch 2.19.7 in podman on the box. Facts marked **[uncertain]** still need confirming during implementation.


> **Implementation status (2026-10-28, branch `rewrite`, local only):** M0–M6 implemented (M6 partially, see
> README "Performance" and the notes below). Scope addition: zstd compression (§3.4, M4).
> Not done: ES 7.17/8.x benchmark and CI jobs (no ES in scope/box), the ±5% regression benchmark against
> `b32b6e9` on the 2M-doc ES dataset, a release (fat-LTO/musl) build run, directory/glob input for split sets.

---

## 0. TL;DR

* **elasticdump-rs is real, public, and good, but narrow.** It has about 5.3k lines of Rust in `src/` (including inline unit tests) plus about 2.8k lines of integration and mock tests (82 unit tests pass on rustc 1.85.1 [verified]). It does exactly one thing: it dumps **ES → JSONL file/stdout** for `--type data`. That covers scroll and PIT, sliced parallel retrieval, basic auth, a CA file, `--insecure`, safe retries, atomic staged output, graceful Ctrl-C, and response-compression opt-in. It is fast. Its README benchmark shows ~2× Node unsliced and up to ~9.7× with sliced PIT.
* **Biggest gaps:**
  1. No **restore direction** at all: no file input, no ES output, no `_bulk` writer, no cluster→cluster.
  2. No `mapping`/`settings` types.
  3. No OpenSearch PIT. `--searchType pit` fails against OpenSearch with `no handler found for uri [/src/_pit]` [verified].
  4. No client-cert TLS (`--tlsAuth/--cert/--key/--pass`), no per-direction `--input-*/--output-*` options.
  5. No `fsCompress`/`compressionLevel`, `fileSize`/`maxRows` splitting, `offset`/`size`, `sourceOnly`, `headers`/`params`, `concurrency`/`throttleInterval` …
  6. It is built on the alpha-only `elasticsearch` crate (8.19.0-alpha.1), which hard-codes ES-only endpoints.
* **Surprises found while verifying:**
  * **Dead concurrency options.** In current elasticdump, `intervalCap`, `concurrencyInterval` and `carryoverConcurrencyCount` are **dead**. They still sit in the defaults table but have not been read since v6.119.0 (commit `6ace8af`, 2025-02-16, which removed `p-queue`), and they have been undocumented since v6.94.1. `concurrency` now only means *write (flush) concurrency*, and `throttleInterval` (default 1 ms) is a sleep after each enqueue. The plan implements all five with the historical p-queue semantics, so scripts keep working.
  * **Settings dump format.** elasticdump writes the `settings` dump as a **double-encoded JSON string** (`"{\"src\":{\"settings\":…}}"`), not an object [verified]. We must emit the same format and accept both formats on read.
  * **OpenSearch PIT data loss.** OpenSearch 2.19 rejects the `_shard_doc` sort (`No mapping found for [_shard_doc]`) [verified]. So Node's `--searchAfter --pit` default fails on OpenSearch. Worse, with a plain `["_doc"]` PIT sort, search_after **silently lost 17 of 1000 docs** on a 2-shard index [verified]. Adding a `_id` tiebreaker returned all 1000.
  * **Offset with scroll.** Node's `--offset N` (N > 0) against ES 7+/OpenSearch sends `from=N` on a scroll request, which the server rejects (`using [from] is not allowed in a scroll context`) [verified with curl].
  * **TLS flags.** Node ignores `--ca/--cert/--key` unless `--tlsAuth` is also set [verified: `--ca` alone → `unable to verify the first certificate`]. Node has no `--insecure` flag; users set `NODE_TLS_REJECT_UNAUTHORIZED=0`.
  * **Default retries.** Node effectively retries every ES request **5 times with a 5 s delay** on 5xx/network errors even when `--retryAttempts` is 0 (`maxAttempts: retryAttempts || 5`). It does **not** retry on 429.
* **Plan:** 7 milestones (M0–M6). Each has ordered tasks and acceptance criteria. Plus a podman integration harness that runs on every PR, against OpenSearch with security disabled and with security plus TLS/mTLS.

---

## 1. What the starting codebase contains (verified)

### 1.1 Layout and dependencies

```
Cargo.toml          edition 2024, rust-version 1.85, single package, lib + bin "elasticdump-rs"
docker-compose.yaml ES 7.17.28 single node (benchmark/integration target)
scripts/benchmark/compare-with-elasticdump.sh  (1.5k lines; Node vs Rust, taskset/cpulimit)
src/
  main.rs (6)            tokio::main → lib::run()
  lib.rs (55)            parse CLI → parse URL → build client → retrieval::dump_data
  cli.rs (171)           clap derive; camelCase long flags (searchBody, scrollTime …)
  elasticsearch.rs (362) URL parsing (base path, %-decoding, /_search suffix), client builder
                         (basic auth header, Accept-Encoding identity, timeout, caFile, insecure)
  output.rs (485)        staged atomic file writer (fsync, hard-link commit, mode preserve), stdout,
                         broken-pipe = exit 0
  retrieval/
    mod.rs (695)         orchestration: retrieval tasks → worker pool → single output task
    retrieval_task.rs    retry classifier (Idempotent vs RejectionOnly), 408/429/502/503/504,
                         exp. backoff capped 30 s, partial-response rejection
    scroll.rs, pit.rs, pit_search.rs, slice_state.rs   scroll/PIT + sliced, shared PIT coordinator
    extract.rs (549)     sonic-rs lazy one-pass extraction: copies each raw hit verbatim + '\n'
    search_body.rs       --searchBody JSON or @file; forces size = --limit
    worker.rs, messages.rs, context.rs, progress.rs (indicatif)
tests/integration_test.rs (2562)  needs live ES at hard-coded http://localhost:9200
tests/support/mock_es.rs (195)    in-process scripted HTTP server for failure/retry paths
```

Key deps: `tokio 1`, `elasticsearch 8.19.0-alpha.1` (rustls; pulls `reqwest 0.12.15`, `rustls 0.23`), `clap 4` derive, `sonic-rs 0.5.8`, `anyhow`, `indicatif`, `bytes`, `url`, `base64`, `bytesize`, `tokio-util` (CancellationToken).

### 1.2 Behaviour observed against OpenSearch 2.19.7 (podman, on the box)

| Scenario | Result |
|---|---|
| scroll dump, 1000 docs, 2 shards | ✅ 1000 lines. Raw hit bytes preserved, e.g. `{"_index":"src","_id":"0","_score":1.0,"_routing":"r0","_source":{…}}` |
| `--slices 2` scroll | ✅ 1000 lines |
| `--searchType pit` | ❌ `HTTP 400 no handler found for uri [/src/_pit]` (ES-only endpoint) |
| HTTPS + `--caFile root-ca.pem` + basic auth (security plugin) | ✅ |
| HTTPS without CA | ❌ TLS error (correct) |
| `--insecure` | ✅ |
| Node elasticdump restoring the Rust JSONL | ✅ 1000 docs. The output format is already compatible. |

### 1.3 Design choices in elasticdump-rs that conflict with drop-in parity

| Topic | elasticdump-rs today | elasticdump | Resolution in this plan |
|---|---|---|---|
| `--limit` default | 10000 | 100 | Default to **100** for parity. Print a one-time hint suggesting a larger `--limit` when ES is the input. (Open question Q1) |
| `--retryAttempts` default | 3, exponential from `--retryDelay` 1000 ms | 0 (but transport retries 5× at 5000 ms on 5xx/network) | See §6.8. Defaults `retryAttempts=0`, `retryDelay=5000`, with an "implicit transport retry" that mirrors Node. (Q2) |
| `--size` | intentionally rejected | supported | Implement |
| Unknown flags | rejected by clap | minimist accepts anything | Accept known-but-out-of-scope elasticdump flags with a warning (§5.3) |
| `--pitKeepAlive` default | 10m | 5m | 5m |
| PIT selection | `--searchType pit` | `--searchAfter true --pit true` | Accept both; they map to the same internal enum |
| Partial responses | hard fail (`allow_partial_search_results=false`) | silently continues | Keep the hard fail (safer); document it as an intentional divergence |
| Output byte format | raw hit bytes (`"_score":1.0`, server whitespace kept in `_source`) | `JSON.stringify` (`"_score":1`, compact) | Keep raw bytes: lossless, faster, semantically identical. The test harness compares JSON semantically, not byte-for-byte. |

---

## 2. elasticdump behaviour reference (in-scope parts, verified from source)

Source files read: `bin/elasticdump` (defaults table), `lib/argv.js`, `lib/help.txt`, `README.md`, `elasticdump.js`, `lib/processor.js`, `lib/ioHelper.js`, `lib/is-url.js`, `lib/parse-base-url.js`, `lib/add-auth.js`, `lib/request.js`, `lib/jsonparser.js`, `lib/parse-meta-data.js`, `lib/transports/{base,file,elasticsearch}.js`, `lib/transports/__es__/{_base,_data,_helpers,_mapping,_setting}.js`, `lib/splitters/streamSplitter.js`.

### 2.1 CLI parsing semantics (`lib/argv.js`, minimist)
* Both `--flag=value` and `--flag value` work. The strings `'true'`, `'false'`, `'null'` and `'Infinity'` are coerced. A bare `--flag` is `true`.
* `options[i] = argv[i] || defaults[i]`. **Falsy values fall back to the default.** For example, `--throttleInterval 0` → default 1 ms.
* `searchBody`, `headers`, `input-headers`, `output-headers`, `params`, `input-params` and `output-params` are JSON-parsed. Only `searchBody` (plus the s3/csv ones) supports `@file`, resolved relative to the CWD.
* `bulkAction` is validated against `/(index|update|delete|create)/`.
* If the output exists and is not `$`, not a URL, and `--overwrite` is not set, the tool exits 1 with ``File `X` already exists, quitting``.

### 2.2 Endpoint detection and URL parsing
* `isUrl`: starts with `http://` or `https://` → Elasticsearch. `$` → stdin/stdout. Anything else is a file (csv:// and s3:// are out of scope).
* `parseBaseURL(url, {index, prefix, suffix})`:
  * Strip trailing `/`, then split on `/`.
  * **≤3 parts** (`http://host:9200`) → no index.
  * **4 parts** → `.../index`.
  * **>4 parts** → `.../index/type`. **Node does not support proxy base paths**: `http://host/proxy/idx` is read as index `proxy`, type `idx`. elasticdump-rs supports base paths. Keep the Rust behaviour and add `--input-index`/`--output-index` to disambiguate.
  * `--input-index`/`--output-index` (`index/type`) override the URL. `--prefix`/`--suffix` wrap the **output** index name.
* Auth sources, in precedence order:
  1. `user:pass@` in the URL.
  2. `--httpAuthFile` (INI with `user=` and `password=`; applied only if the URL lacks credentials; both keys required, otherwise `Malformed Auth File`).
  3. Env vars `ELASTICDUMP_{INPUT|OUTPUT}_USERNAME` / `_PASSWORD`, applied only if the URL has no basic auth.

### 2.3 Processing loop (`lib/processor.js`)
* Reads are strictly sequential (`IterableMapper` concurrency 1), with a prefetch queue of `max(5, 2*min(concurrency,20))`.
* Writes go through `IterableQueueMapperSimple` with `concurrency` (default 1). `throttleInterval` ms of sleep follows every enqueue.
* An empty batch means end of input. The loop waits for in-flight writes, then calls `set([])` to close the output.
* `ignore-errors` keeps the loop going after a write error.
* On SIGINT/SIGTERM (`lib/shutdown.js`), it stops reading and waits for in-flight writes.

### 2.4 Data: ES input (`_data.js`, `_helpers.js`, `_base.js`)
* `GET /` first, to detect the version. OpenSearch is treated as ES `force-os-version` (default 7.10.2), with `IsOpenSearch=true`.
* Default searchBody: `{"query":{"match_all":{}},"stored_fields":["*"],"_source":true}` (ES ≥ 5).
* **Scroll** (default):
  * Initial request: `GET|POST {url}/_search?scroll={scrollTime}&from={offset}{&params}`, with `size = min(size, limit)` when `size ≥ 0`.
  * Continuation: `POST /_search/scroll {"scroll","scroll_id"}`.
  * When a response has `terminated_early && _shards.failed>0`, Node sleeps `scrollRetryDelay` (15000) and continues. That page is dropped (Node bug).
  * `--scrollId` resumes an existing scroll.
* **search_after** (`--searchAfter`):
  * Without `--pit`: `POST {url}/_search`, with `sort` defaulting to `['_shard_doc']`. That sort is only valid with PIT, so in practice the user must supply one.
  * With `--pit`, opening the PIT:
    * ES: `POST {url}/_pit?keep_alive=…`, reading `id`.
    * OpenSearch: `POST {host}/{index|*}/_search/point_in_time?keep_alive=…`, reading `pit_id`.
  * Searching: `POST {host}/_search` with `pit:{id,keep_alive}` and `search_after` set to the last hit's `sort`.
  * Closing:
    * ES: `DELETE {url}/_pit {"id"}`.
    * OpenSearch: `DELETE {url}/_search/point_in_time {"pit_id":[…]}`.
* `--size` is a total cap (−1 = unlimited).
* `--offset` skips N hits client-side (`elementsToSkip`) **and** passes `from=`, which breaks on ES 7+ (see §0).
* `--delete` / `--delete-with-routing` delete each source doc after reading it. Deferred (§9).

### 2.5 Data: ES output (`setData`)
* Request: `PUT {url}/_bulk{?params}` (POST for serverless), `Content-Type: application/x-ndjson`.
* Action line per doc: `{bulkAction:{_index: outputIndex || elem._index, _id, …}}`, plus:
  * `_type: outputType || elem._type`, only for ES < 7;
  * `version` and `version_type`, when requested;
  * meta fields.
* Body line: `_source`, or `{"doc":_source}` for `update`. `delete` still emits a body line (Node bug); Rust must emit none.
* Meta fields are copied from `elem.X` or `elem._X` (and from `elem.fields`). The defaults are `routing,parent,timestamp,ttl`, plus `--parseExtraFields`.
* `--handleVersion` copies `_version`/`version`. `--versionType` adds `version_type`.
* `--esCompress` gzips the bulk body (`Content-Encoding: gzip`).
* Response: items with status < 400 count as writes. For failed items, `ignore-es-write-errors` (**default true**) prints the item to stderr; when it is false, the batch fails.
* After **every** bulk call, Node sends `POST {url}/_refresh` unless `--noRefresh` is set.

### 2.6 Mapping (`_mapping.js`)
* **Get:** `GET {url}/_mapping{?params}`. One payload element (an object keyed by index), written as one JSON object line.
* **Set:**
  1. `PUT {url}` to make sure the index exists. Errors are ignored.
  2. ES < 7: for each index in the file and each mapping key, `PUT {url}/{type}/_mapping`, with parent/child ordering for `_parent`.
  3. ES ≥ 7: wrap the data as `{key: data}`. Exception: if the key is `properties` and the data has no nested `properties`, use `{properties: data}`. This is the fix for issues #667/#1007.
  4. If the output URL has no index, the path is `/{indexFromFile}`.
* Errors during set are mostly swallowed. A response body `error` is passed to the final callback.

### 2.7 Settings (`_setting.js`)
* **Get:** `GET {url}/_settings`, then `_.omit(output[index], settingsExclude)`. The exclude list:
  * `settings.index.version`
  * `settings.index.creation_date`
  * `settings.index.uuid`
  * `settings.index.provided_name`
  * `settings.index.resize`
  * `settings.index.routing.allocation.initial_recovery`

  The payload is `JSON.stringify(output)`, so the file line is a **JSON string literal** [verified]. When the input URL has no index, the omit targets `output[undefined]` and nothing is filtered (Node bug).
* **Set:** parse the string. For each `(name, index)`, send `PUT {output url}` with `_.omit(index, settingsExclude)`, i.e. `{"settings":{…}}`.
  * This **creates** the index, so it fails if the index already exists.
  * ES < 7 with `type=settings` unwraps `.settings`.
  * **The index name in the file is ignored**; the output URL's index is used.
* Verified OpenSearch dump line: `"{\"src\":{\"settings\":{\"index\":{\"replication\":{\"type\":\"DOCUMENT\"},\"number_of_shards\":\"2\",\"analysis\":{…},\"number_of_replicas\":\"0\"}}}}"`.

### 2.8 File transport (`file.js`, `base.js`, `streamSplitter.js`)
* **Read:**
  * `JSONStream.parse()` with no path yields **every top-level JSON value** in the stream, so NDJSON and concatenated JSON both work, and blank lines are fine. `$` means stdin.
  * `--fsCompress` puts gunzip in the pipe (`Z_SYNC_FLUSH`, lenient).
  * Batches hold `limit` values. `offset` skips N values.
  * Quirk: `completeBatch` pops, which **reverses order within a batch**. Not replicated.
* **Write:**
  * One JSON value per line, plus `os.EOL`.
  * `sourceOnly` writes only `_source`. `format=human` uses `util.inspect`.
  * `--fsCompress` gzips at `compressionLevel` (default zlib −1 = level 6).
  * The write path honours `drain` backpressure.
* **Split** (`fileSize` or `maxRows`):
  * `fileSize` is parsed with the `bytes` module (`10mb` = 10·1024²), but it counts the **JavaScript string length of the uncompressed line**, not bytes.
  * Files are named `{dir}/{base}.split-{n}{ext}`, plus `.gz` when compressed. Verified: `out.split-0.json.gz … out.split-5.json.gz`.
  * A part closes once its counter reaches the limit, so a part can exceed `fileSize` by one line.
  * **There is no automatic multi-file read.** Restoring a split dump takes one invocation per part.
* `--overwrite` is checked at both the CLI and the transport level.

### 2.9 HTTP layer (`lib/transports/elasticsearch.js`, `lib/request.js`)
* Default headers: `User-Agent: elasticdump` and `Content-Type: application/json`, then `--headers`, then `--{input|output}-headers`.
* `timeout` (ms). `gzip: esCompress`.
* Retry settings (`requestretry`):
  * `maxAttempts: retryAttempts || 5`, `retryDelay: retryDelay || 5000`.
  * Strategy `HTTPOrNetworkError`: 5xx, or the network errors `ECONNRESET, ENOTFOUND, ESOCKETTIMEDOUT, ETIMEDOUT, ECONNREFUSED, EHOSTUNREACH, EPIPE, EAI_AGAIN`.
  * Fixed delay, no backoff.
* TLS applies only when `--tlsAuth` is set. Node reads `{dir}-cert`, `{dir}-key`, `{dir}-pass` and `{dir}-ca`, then the shared `cert/key/pass/ca`. The shared ones are assigned second, so they win when both are set.
* `--maxSockets` sets the global agent socket cap.

---

## 3. Parity matrix

Legend:
* **Status** (elasticdump-rs today): ✅ done · 🟡 partial · ❌ missing · ➖ N/A or out of scope.
* **Ms**: the milestone that delivers it.
* "Default" is elasticdump's default.

### 3.1 Core, endpoints and types

| elasticdump option | Default | Status | Planned Rust implementation | Ms |
|---|---|---|---|---|
| `--input` | required | 🟡 ES URL only | `Endpoint::parse` → `Es{…}` (http/https), `Stdio` (`$`), `File(path)`. Host-only URL = all indices (`/_search`, `/_mapping`, `/_settings`) | M1 |
| `--output` | required | 🟡 file/stdout | Same enum. ES output via `BulkSink` / `MappingSink` / `SettingsSink` | M1–M3 |
| `--input-index` / `--output-index` | from URL | ❌ | `index[/type]` override; `type` used only for ES < 7 | M1 |
| `--type data` | data | ✅ | keep | – |
| `--type mapping` | – | ❌ | `MappingSource`/`MappingSink` (§6.4) | M3 |
| `--type settings` | – | ❌ | `SettingsSource`/`SettingsSink` (§6.5) | M3 |
| `--type index` | – | ❌ | *Stretch*: `GET /{idx}` → `PUT /{idx}` (settings+mappings+aliases), reusing the settings code | M6 (opt) |
| `--overwrite` | false | ✅ | Keep the staged atomic writer. Also applies to split parts | – |
| `--prefix` / `--suffix` | '' | ❌ | Applied to the output index name only | M2 |
| `--help` / `--version` | – | ✅ (clap) | Also add elasticdump-style long help text | M1 |
| `--debug` | false | ✅ | Also log each ES request line (method, redacted URL), like Node | M1 |
| `--quiet` | false | ✅ | keep | – |
| `--toLog` | true (false for `$`) | ❌ | Logs always go to stderr, so stdout output is never polluted. Accept the flag; `false` suppresses info logs | M1 |
| `--format human` | '' | ❌ | Pretty-print each doc. Low priority | M5 |

### 3.2 Data retrieval (ES input)

| Option | Default | Status | Planned | Ms |
|---|---|---|---|---|
| `--limit` | 100 | 🟡 default 10000 | Default 100. Page size for ES, batch size for file reads, bulk size for writes | M1 |
| `--size` | -1 | ❌ (rejected today) | Total cap. Shrink the final page's `size`, then stop and clear the scroll/PIT. With slices: a shared atomic budget, truncating the last batch | M2 |
| `--offset` | 0 | ❌ | Client-side skip of the first N docs. **Never** send `from=` with scroll (breaks ES 7+/OS). File input: skip N values | M2 |
| `--searchBody` (+`@file`) | match_all + `stored_fields:["*"]` + `_source:true` | 🟡 no default `stored_fields` | Keep JSON and `@file`. Apply Node's default body when none is given. `@` resolves relative to the CWD. `size` is overridden with `limit` (or with `--size` when smaller) | M1 |
| `--searchWithTemplate` | false | ❌ | `POST /_render/template {id,params}` → use `template_output` as the search body | M5 |
| `--searchBodyTemplate` | – | ➖ | Out of scope (JS code). Error | – |
| `--scrollTime` | 10m | ✅ | keep | – |
| `--scrollId` | null | ❌ | Resume: skip the initial search and start with `POST /_search/scroll`. Incompatible with slices | M2 |
| `--scroll-with-post` | false | 🟡 (always POST) | Accepted. Requests stay POST either way (GET with a body breaks some proxies) | M1 |
| `scrollRetryDelay` (undocumented) | 15000 | ➖ | Accepted and ignored. Partial pages are a hard error in Rust | M1 |
| `--searchAfter` | false | 🟡 (PIT only) | Without PIT: `search_after` on `{index}/_search`. **Requires a user `sort` with a unique tiebreaker** (error if missing) | M2 |
| `--pit` | false | 🟡 (`--searchType pit`) | With `--searchAfter` (or `--searchType pit`): flavor-aware PIT (§6.3), reusing the existing shared-PIT slice coordinator | M2 |
| `--pitKeepAlive` | 5m | 🟡 default 10m | Default 5m | M1 |
| `--esCompress` (responses) | false | ✅ | keep (`Accept-Encoding: gzip`, transparent decode) | – |
| `--support-big-int`, `--big-int-fields` | false | ✅ by design | Raw passthrough is lossless in both directions (§6.2). Accepted as no-ops | M1 |
| `--parseExtraFields` | '' | ❌ | Extra meta fields copied into the bulk action (§6.6) | M2 |
| `--sourceOnly` | false | ❌ | Write only the raw `_source` bytes | M2 |
| rs-only `--slices`, `--workers`, `--bufferSize`, `--searchType`, `--requestTimeout`, `--username/--password`, `--caFile`, `--insecure` | – | ✅ | Keep as documented extensions. `--caFile` becomes an alias of `--input-ca` | – |

### 3.3 Data writing (ES output)

| Option | Default | Status | Planned | Ms |
|---|---|---|---|---|
| bulk write | – | ❌ | `BulkSink`: NDJSON builder from raw docs, `PUT {index or host}/_bulk` | M2 |
| `--bulkAction` | index | ❌ | `index` / `create` / `update` (`{"doc":…}`) / `delete` (no body line). Value validated | M2 |
| `--handleVersion` / `--versionType` | false / null | ❌ | Copy `_version` (or `version`) → `version`; pass `version_type` through | M2 |
| `--noRefresh` | false | ❌ | Default: `POST {index}/_refresh` after each bulk call (parity). `--noRefresh` skips it | M2 |
| `--ignore-es-write-errors` | **true** | ❌ | Failed items are logged to stderr as JSON lines and counted. `false` → fail on the first failed item | M2 |
| `--ignore-errors` | false | ❌ | Batch-level errors (an HTTP failure after retries) are logged and skipped, and the loop continues | M2 |
| `--skip-existing` | false | ➖ (Node uses it for analyzer only) | Extension: settings/mapping sinks treat `resource_already_exists_exception` as success | M3 |
| `--esCompress` (requests) | false | ❌ | gzip the bulk body, `Content-Encoding: gzip` | M2 |
| `--params`, `--input-params`, `--output-params` | null | ❌ | JSON object → query string. Input params go on search/scroll, output params on bulk. `params` is the fallback for both and is also used on `_mapping` GET/PUT | M1 |
| `--headers`, `--input-headers`, `--output-headers` | `{"User-Agent":"elasticdump"}` | ❌ | Merge order: defaults → `headers` → direction headers. Applied to every request in that direction | M1 |

### 3.4 Files and compression

| Option | Default | Status | Planned | Ms |
|---|---|---|---|---|
| file / stdin input (`$`) | – | ❌ | `FileSource`: streaming reader of top-level JSON values (NDJSON fast path, concatenated-JSON fallback), batches of `limit` | M1 |
| file / stdout output | – | ✅ | Keep the staged writer, generalised behind `Sink` | – |
| `--fsCompress` | false | ❌ | Write through gzip. Read through gunzip, plus auto-detect the gzip magic `1f 8b` even without the flag (extension; logs a notice) | M4 |
| `--compressionLevel` | -1 (zlib default 6) | ❌ | 0–9; −1 maps to 6 | M4 |
| `--fileSize` | -1 | ❌ | `Splitter` sized in uncompressed **bytes** (Node counts UTF-16 units; documented divergence). Same grammar as the `bytes` module (`b,kb,mb,gb,tb,pb`, base 1024, decimals) | M4 |
| `--maxRows` | -1 | ❌ | `Splitter` by line count | M4 |
| `--compressionType` (extension) | none | ❌ | **In scope (user request, 2026-10-28; zstd is the preferred codec).** `gzip` or `zstd`. `--fsCompress` alone keeps meaning gzip (drop-in parity); `--compressionType zstd` turns compression on by itself. Default stays uncompressed for Node compatibility | M4 |
| zstd write | – | ❌ | Each file / split part is one complete zstd stream (content checksum on). `--compressionLevel` 1–22, −1 maps to 3 | M4 |
| zstd read | – | ❌ | Auto-detected from the magic `28 b5 2f fd`, like gzip; concatenated frames read in sequence. A `.zst`/`.zstd` file that is not zstd is an error. Node cannot read these files (documented) | M4 |
| split naming | `{base}.split-{n}{ext}[.gz]` | ❌ | Identical naming. Each part staged and committed atomically. zstd parts use `.zst` | M4 |
| reading split sets | n/a in Node | ❌ | Extension: if `--input` is a directory or glob, read the parts in numeric `split-N` order | M4 (opt) |

### 3.5 Auth and TLS

| Option | Default | Status | Planned | Ms |
|---|---|---|---|---|
| URL basic auth | – | ✅ | keep (percent-decoding, redacted logs) | – |
| `ELASTICDUMP_INPUT_USERNAME/PASSWORD`, `ELASTICDUMP_OUTPUT_…` | – | ❌ | Used when the URL has no credentials | M1 |
| `--httpAuthFile` | null | ❌ | INI `user=`/`password=`. Applies to both endpoints when the URL lacks credentials. Error `Malformed Auth File` | M1 |
| `--tlsAuth` | false | ❌ | Accepted. Rust **always** honours `--ca/--cert/--key` when given; Node silently ignores them without `--tlsAuth` (divergence). Warn when cert flags are set without `--tlsAuth` | M1 |
| `--ca`, `--input-ca`, `--output-ca` | null | 🟡 (`--caFile`, input only) | PEM bundle per direction. The shared `--ca` wins over the direction flag when both are set (Node assignment order) | M1 |
| `--cert` / `--key` (+ input/output) | null | ❌ | PEM client cert chain and key (PKCS#8 / PKCS#1 / SEC1) → rustls client auth | M1 |
| `--pass` (+ input/output) | null | ❌ | Decrypt an encrypted PKCS#8 key (`pkcs8` crate, `encryption` + `pem`). Legacy `Proc-Type: 4,ENCRYPTED` keys are unsupported: clear error with an `openssl pkcs8 -topk8` hint [uncertain whether users rely on them] | M1 |
| insecure | via `NODE_TLS_REJECT_UNAUTHORIZED=0` | ✅ (`--insecure`) | Keep `--insecure`, add `--input-insecure`/`--output-insecure`. Also honour `NODE_TLS_REJECT_UNAUTHORIZED=0`, with a loud warning | M1 |
| AWS SigV4, socks proxy | – | ➖ | Out of scope. Error | – |

### 3.6 Concurrency, throttling, timeouts, retries

| Option | Default | Status | Planned | Ms |
|---|---|---|---|---|
| `--concurrency` | 1 | ❌ | Max in-flight **write** batches. File sinks are forced to 1 to keep order | M5 |
| `--throttleInterval` | 1 ms | ❌ | Sleep after each batch is enqueued for writing | M5 |
| `--intervalCap` | 5 (dead in Node ≥ 6.119) | ❌ | Rate limiter: at most `intervalCap` request starts per `concurrencyInterval` window (p-queue semantics). **Off unless set explicitly** (Q3) | M5 |
| `--concurrencyInterval` | 5000 ms (dead) | ❌ | Window length for `intervalCap` | M5 |
| `--carryoverConcurrencyCount` | true (dead) | ❌ | If true, requests still in flight at a window boundary count against the next window | M5 |
| `--maxSockets` | ∞ | ❌ | Connection-pool cap plus a global semaphore on in-flight HTTP requests | M5 |
| `--timeout` | null (ms) | 🟡 (`--requestTimeout`, seconds) | `--timeout` in **ms** (parity). `--requestTimeout` kept as an alias in seconds. `--timeout` wins if both are set | M1 |
| `--retryAttempts` | 0 | 🟡 | See §6.8 | M5 |
| `--retryDelay` | 5000 | 🟡 | See §6.8 | M5 |
| `--retryDelayBase`, `--customBackoff` | – | ➖ (S3 only) | Accepted and ignored | – |

### 3.7 Explicitly out of scope

The following are accepted with a warning, or rejected, according to the policy below:

* the `analyzer`, `alias`, `template`, `component_template`, `index_template`, `policy` and `script` types;
* CSV (`csv://`, `--csv*`);
* S3/MinIO (`s3://`, `--s3*`);
* AWS (`--aws*`, `--sessionToken`);
* `--transform`, `--searchBodyTemplate`, `--inputTransport`/`--outputTransport`;
* `multielasticdump`;
* `--inputSocksProxy/Port` and `--outputSocksProxy/Port`;
* `--filterSystemTemplates`, `--templateRegex`;
* `--heapSnapshot`;
* `--openSearchServerless` and `--force-os-version` (serverless offset/limit mode);
* `--delete` and `--delete-with-routing` (deferred; see §9);
* `--jsonLines` (dead in Node: it exists only in the defaults table).

**Policy:**
* A `--type` outside `data|mapping|settings[|index]` → **error** (exit 2) naming the unsupported type.
* A flag that only tunes an out-of-scope feature (e.g. `--s3Region`, `--csvDelimiter`, `--templateRegex`) → warning, then ignored, so existing wrapper scripts still run.
* A flag that would change the data if silently ignored (`--transform`, `--searchBodyTemplate`, `--inputTransport`, `--outputTransport`, `--delete`, s3/csv URLs, AWS signing, socks proxy) → **error**.

---

## 4. Target architecture

### 4.1 Big decision: replace the `elasticsearch` crate with a thin `reqwest` client
Reasons (all verified):
1. The crate hard-codes the ES PIT path `/{index}/_pit`, which breaks OpenSearch.
2. It has only ever shipped `-alpha` releases on crates.io.
3. We need full control over:
   * per-direction headers and query params;
   * gzip **request** bodies;
   * NDJSON bulk bodies;
   * raw response bytes (already used);
   * encrypted client keys;
   * URL base paths.
4. Its `Credentials` enum is single-valued (cert *or* basic), whereas we want both. Today's code already works around this by adding the auth header by hand.

The new `EsClient` keeps the hot path: one-pass sonic-rs extraction over raw `Bytes`.

### 4.2 Dependencies (target)
```toml
tokio             = { version = "1", features = ["rt-multi-thread","macros","fs","io-std","io-util","sync","time","signal"] }
reqwest           = { version = "0.12", default-features = false, features = ["rustls-tls-native-roots","gzip","stream","http2"] }
rustls            = "0.23"        # explicit ClientConfig: custom CA, insecure verifier, client identity
rustls-pemfile    = "2"
pkcs8             = { version = "0.10", features = ["encryption","pem","std"] }  # --pass
sonic-rs          = "0.5"         # keep: lazy extraction, raw slices
serde             = { version = "1", features = ["derive"] }
serde_json        = { version = "1", features = ["raw_value","arbitrary_precision"] }  # settings/mapping only
async-compression = { version = "0.4", features = ["tokio","gzip"] }  # fsCompress streaming
flate2            = "1"           # gzip bulk request bodies and gzip file output
zstd              = { version = "0.13", default-features = false }  # zstd file output/input (scope addition, M4)
clap              = { version = "4", features = ["derive","env","wrap_help"] }
tokio-util        = { version = "0.7", features = ["io","codec"] }   # LinesCodec, CancellationToken
rust-ini          = "0.21"        # --httpAuthFile
semver            = "1"
thiserror         = "2"
# kept: bytes, url, percent-encoding, base64, bytesize, anyhow, log, env_logger, indicatif, console
[dev-dependencies]
tempfile, assert_cmd = "2", predicates, wiremock = "0.6", proptest = "1", criterion = "0.5"
```
* rustls (pure Rust) is the default, which allows static musl builds. An optional `native-tls` feature covers the OS trust store and legacy keys.
* A custom `--ca` **replaces** the default roots, matching Node `tls` `ca` semantics [uncertain whether "append" would be friendlier; keep Node behaviour].

### 4.3 Module layout (single package: library + binary)
```
src/
  main.rs                 // exit codes: 0 ok, 1 runtime error, 2 usage error, 130 forced double Ctrl-C
  lib.rs                  // run(): parse → resolve Config → build Source/Sink → pipeline::run
  cli/
    mod.rs                // clap derive RawArgs: every elasticdump flag name, verbatim spelling
    normalize.rs          // minimist-compat pre-pass (§5.2)
    config.rs             // RawArgs → validated Config { input: EndpointCfg, output: EndpointCfg, kind, pipeline, retry, … }
    deprecated.rs         // out-of-scope flag table → Warn | Error
  endpoint/
    mod.rs                // enum Endpoint { Es(EsTarget), File(PathBuf), Stdio }
    url.rs                // today's parse_input_url, generalised: index/type, base path, prefix/suffix
    auth.rs               // URL creds → httpAuthFile → env vars; --username/--password override
  http/
    client.rs             // EsClient { reqwest::Client, base: Url, headers, params, auth, gzip_req, limiter }
    tls.rs                // rustls ClientConfig: roots | custom CA | insecure verifier | identity (+ --pass)
    retry.rs              // RetryPolicy, classify(), backoff (generalises retrieval_task.rs)
    limiter.rs            // concurrency semaphore + IntervalLimiter (intervalCap)
    info.rs               // ClusterInfo { flavor: Elastic|OpenSearch, version } via GET /
  model/
    doc.rs                // RawDoc { line: Bytes } + lazily extracted DocMeta (§6.2)
    batch.rs              // enum Batch { Docs(Vec<RawDoc>), Ndjson{buf: Bytes, count}, Mapping(Value), Settings(Value) }
  source/
    mod.rs                // enum Source { EsData, EsMapping, EsSettings, File }
    es_data/              // MOVED from retrieval/: scroll.rs, pit.rs (flavor-aware), pit_search.rs,
                          //   search_after.rs (non-PIT), slice_state.rs, extract.rs, worker.rs
    es_mapping.rs, es_settings.rs
    file.rs               // FileSource (gunzip, offset, limit batching, stdin)
  sink/
    mod.rs                // enum Sink { File, Bulk, Mapping, Settings }
    file.rs               // staged writer (output.rs), sourceOnly, human format
    split.rs              // Splitter (fileSize / maxRows), staged parts
    compress.rs           // gzip writer adapter
    bulk.rs               // NDJSON builder, response parser, refresh
    es_mapping.rs, es_settings.rs
  pipeline/
    mod.rs                // prefetch, write pool, throttle, offset/size accounting, ignore-errors,
                          //   cancellation, progress
    stats.rs              // counters: read, written, failed items, bytes
  output.rs               // existing staged writer, extended to multi-part commits
  progress.rs, error.rs   // error.rs: thiserror UsageError, EsError{status,body}, BulkItemError, Io…
```

**Why enums instead of `dyn Trait`:** the set of transports is closed, and async-fn-in-trait with `dyn` still needs boxing. Each variant implements internal traits, and the enums delegate to them:
```rust
pub(crate) trait DataSource { async fn next_batch(&mut self) -> Result<Option<Batch>>; async fn close(&mut self) -> Result<()>; fn total_hint(&self) -> Option<u64>; }
pub(crate) trait DataSink   { async fn write(&mut self, b: Batch) -> Result<WriteReport>; async fn finish(self) -> Result<()>; fn ordered(&self) -> bool; }
```

### 4.4 Data flow and backpressure
```
           ┌──────── ES data source (existing engine, kept) ───────┐
 slices ──►│ retrieval tasks → bounded mpsc(bufferSize) →          │
 / PIT     │ extract workers (sonic-rs, raw hits)                  │──► Batch::Ndjson (ES→file fast path)
           └────────────────────────────────────────────────────────┘    Batch::Docs   (ES→ES, sourceOnly)
                                   │ bounded channel (prefetch = max(5, 2*min(concurrency,20)))
                                   ▼
       pipeline: offset skip → size cap → throttleInterval → limiter.acquire()
                                   ▼
       write pool (JoinSet, ≤ concurrency in flight; file sinks: 1, ordered)
                                   ▼
       FileSink (gzip? split?) | BulkSink (+refresh) | Mapping / Settings sinks
```
* **Bounded memory.** Every stage uses a bounded `tokio::mpsc`. Memory ≈ `(bufferSize + prefetch + concurrency) × limit × avg_doc_size`; document this formula.
* **Fast path.** ES → file without `sourceOnly` keeps today's zero-copy `ExtractedOutputBatch` (`Batch::Ndjson`), with no per-doc allocation. This must not regress (§8).
* **ES → ES.** The extractor emits `Batch::Docs(Vec<RawDoc>)`, with `Bytes` slices of each hit. `BulkSink` builds NDJSON directly from the raw slices.
* **File → ES.** `FileSource` reads lines with `LinesCodec`, guarded by a maximum line length (extension `--maxLineBytes`, default 512 MiB).
  * Fallback for pretty-printed or concatenated JSON: `serde_json::Deserializer::into_iter::<Box<RawValue>>()` on a blocking thread fed through `SyncIoBridge`.
  * Fallback trigger: the first non-blank line does not parse as a complete value.
  * Settings files hold a JSON **string** value; it is decoded as nested JSON.
* **Ordering.** Order is preserved when `slices ≤ 1 && workers == 1 && concurrency == 1`. Otherwise order is not guaranteed (already documented upstream).

### 4.5 Errors, exit codes, logging
* Errors use `thiserror` enums at module boundaries; `anyhow` appears only in `main` and `lib::run`.
* Exit codes:

  | Code | Meaning |
  |---|---|
  | 0 | success |
  | 1 | runtime failure (any non-ignored error) |
  | 2 | usage or config error |
  | 130 | forced double Ctrl-C (existing behaviour) |

* The final summary always goes to stderr: `Total Writes: N` (Node's wording, so scripts that grep it keep working), plus `failed items: M` when M > 0.
* Logs always go to stderr, so stdout dumps (`$`) stay clean.
* Never log secrets:
  * keep `redacted_url`;
  * never print the values of `authorization`, `cookie` or `x-api-key` headers;
  * never print `--pass`.

---

## 5. CLI design

### 5.1 Flag naming
* Every elasticdump flag keeps its **exact spelling**, including the mixed styles (`--searchBody`, `--input-index`, `--ignore-es-write-errors`, `--scroll-with-post`, `--fsCompress`). Use an explicit `#[arg(long = "…")]` per field.
* Booleans accept an optional value, because Node scripts pass `--fsCompress=true` or `--noRefresh true`. Declare them as `num_args = 0..=1, default_missing_value = "true"` with a loose parser accepting `true|false|1|0|yes|no`.
* Existing elasticdump-rs flags stay as documented extensions.

### 5.2 minimist compatibility pre-pass (`cli/normalize.rs`)
* Accept `--flag=value` and `--flag value`.
* For booleans, a following `true`/`false` token is taken as the value.
* `'null'` → unset. `'Infinity'` → maximum.
* Do **not** replicate the "falsy means default" bug in general. For example, `--throttleInterval 0` really means 0 in Rust. List this under documented divergences.
* For repeated flags, keep the last value (as minimist does) and warn.

### 5.3 Out-of-scope flags
The table in `cli/deprecated.rs` holds `(name, takes_value, Warn|Error, message)` for every flag in §3.7. These are hidden clap args: parsing succeeds, and the action is applied after parsing.

### 5.4 Validation (usage errors, exit 2, unless noted)
* `--type` is not one of `data`, `mapping`, `settings` (or `index`, if implemented).
* Both endpoints are ES-less (file↔file) and `--type` is not `data`.
* `--fileSize` or `--maxRows` with an ES or `$` output.
* `--fileSize` and `--maxRows` together. Node silently prefers `fileSize`; we error.
* `--scrollId` with `--slices > 1`, `--searchAfter` or PIT.
* `--searchAfter` without PIT and without a `sort` in the search body.
* `--bulkAction` not in `index|create|update|delete`.
* Warnings only, not errors:
  * `--sourceOnly` with an ES output: no metadata, so ids are auto-generated.
  * `--offset`, `--size` or splitting with `mapping`/`settings`: ignored.

---

## 6. Feature design notes

### 6.1 Cluster info and capabilities
* Send `GET {base}/` once per ES endpoint, lazily, and cache the result.
* Flavor detection:
  * `version.distribution == "opensearch"` → `OpenSearch(version)`;
  * anything else → `Elastic(version)`;
  * no `version` field → Elastic 5.0.0, with a debug log (Node parity);
  * `server: aoss-amazon` → error: Serverless is out of scope.
* Capability helpers:

  | Helper | Elastic | OpenSearch |
  |---|---|---|
  | `has_types()` | version < 7 | – |
  | `pit_api()` | ≥ 7.10 → `EsPit` | ≥ 2.4 → `OsPit` |
  | `supports_shard_doc_sort()` | ≥ 7.12 | **false** (verified on 2.19.7; [uncertain] for 3.x, so probe) |

* Never send `compatible-with` Accept headers; they break OpenSearch. Plain `application/json` works on ES 8.

### 6.2 Document model and lossless JSON
* `RawDoc { line: Bytes }`. `meta()` uses sonic-rs `get_many` over `_index, _type, _id, _routing, routing, _parent, parent, _version, version, _source, fields`, plus the parseExtraFields keys.
* **Never re-serialise `_source`.**
  * The bulk body line is `meta.source.as_raw_str()`.
  * For `update`, it is `{"doc":` + raw + `}`.
  * Big integers, float formatting, escapes and key order are therefore preserved exactly. Node needs `--support-big-int` for this, so that flag and `--big-int-fields` become accepted no-ops (debug log).
* File lines without `_source` (e.g. a `sourceOnly` dump) are treated as the whole object being `_source`, with no metadata, and a one-time warning. Node would send `undefined`; this is a documented improvement.
* `fields` is used only as a fallback source of meta values (e.g. `fields._routing` on old ES).

### 6.3 Retrieval modes (ES input)
1. **Scroll (default).** Keep the current engine, and add:
   * `--input-params` on the initial search;
   * `--scrollId` resume (total unknown, so the progress bar becomes a spinner);
   * the `--size` cap and client-side `--offset` skip;
   * the existing safety rule: scroll continuations retry only on 429. When `--retryAttempts > 0`, they also retry on **connect-phase** errors, where the request never reached the server, which is still data-safe.
2. **PIT + search_after.** Enabled by `--searchType pit`, or `--searchAfter` with `--pit`. Generalise `pit.rs` with `enum PitApi { Es, Os }`:

   | Step | Elasticsearch | OpenSearch |
   |---|---|---|
   | Open | `POST /{index}/_pit?keep_alive=` → `id` | `POST /{index}/_search/point_in_time?keep_alive=` → `pit_id` (verified) |
   | Search | `POST /_search` with no index; body `pit:{id,keep_alive}` + `search_after` | same; refreshed `pit_id` returned in each response (verified) |
   | Close | `DELETE /_pit {"id"}` | `DELETE /_search/point_in_time {"pit_id":[…]}` (verified) |

   **Default sort:**
   * Elastic: `["_shard_doc"]` (today's behaviour).
   * **OpenSearch: `[{"_doc":"asc"},{"_id":"asc"}]`**, with a warning about the cost of sorting on `_id`. A sort of `["_doc"]` alone lost docs (983/1000, verified).
   * User-supplied sort: kept, with a warning if its last key is not a unique tiebreaker.
   * [uncertain: `_id` sort performance on very large OpenSearch indices; benchmark in M6 (Q4).]

   Sliced PIT works on OpenSearch (`slice:{id,max}` verified on a single request). Reuse the shared-PIT coordinator.
3. **search_after without PIT** (`--searchAfter` alone). `POST /{index}/_search` with the user's sort. This is not snapshot-consistent, so warn.

**Partial responses:** keep `allow_partial_search_results=false`, and reject responses with `_shards.failed > 0` or `timed_out` (works on OpenSearch, verified).

**Multi-index / host-only input:** `--input http://host:9200` searches `/_search` across all indices. Each hit keeps its `_index`, so a restore recreates the original names unless `--output-index` is set. No implicit filtering of dot-indices (parity). Users can pass `--input-index '*,-.*'`.

### 6.4 Mapping
* **Dump.** `GET {base}/{index?}/_mapping{?params}`, written as **one line** holding the raw response object. Gzip is honoured; splitting is ignored.
* **Restore:**
  1. Read the first JSON value from the file or source and parse it with `serde_json` (`arbitrary_precision`).
  2. For each `(src_index, def)`, pick the target index: the output URL index (with prefix/suffix) if present, else `src_index`.
  3. Make sure the target exists with `PUT /{target}`. Accept 200, or 400 with `resource_already_exists_exception`. Any other error is fatal (Node ignores all errors here; `--ignore-errors` relaxes ours).
  4. Elastic ≥ 7 and OpenSearch:
     * **Detect a typed 6.x export.** It has exactly one key that is not a mapping parameter (`properties, dynamic, _source, _routing, _meta, dynamic_templates, date_detection, numeric_detection, runtime, enabled, _field_names, _all`), and that key's value is an object containing `properties`. Unwrap it.
     * Strip `_all` and `include_in_all`, with a warning.
     * Port the #667/#1007 handling for a field literally named `properties`.
     * Send `PUT /{target}/_mapping` with the typeless body.
  5. Elastic < 7: per type, `PUT /{target}/{type}/_mapping`, parents first (port the `_parent` ordering).
  6. A 200 response whose body contains `error` is treated as an error.
* **Conflicts** (e.g. a field type change): surface the server's `illegal_argument_exception` verbatim. No partial retry.

### 6.5 Settings
* **Dump:**
  1. `GET {base}/{index?}/_settings{?params}`.
  2. For **every** index key (this fixes Node's multi-index bug), remove the exclude paths: `index.version` (the whole object), `index.creation_date`, `index.uuid`, `index.provided_name`, `index.resize`, `index.routing.allocation.initial_recovery`.
  3. Write the result as a **JSON string literal** line, for format parity with Node.
  * Extension: `--settingsFormat object` writes a plain JSON object instead.
* **Restore:**
  1. Accept either a string literal (decode it first) or an object.
  2. Strip the exclude paths again; the file may come from another tool.
  3. Strip keys that cannot be set at create time: `index.history.uuid`, `index.verified_before_close`, `index.store.snapshot.*` [uncertain list; extend from integration failures].
  4. Cross-flavor stripping, via the extension `--settingsStripIncompatible`:
     * Elastic → OpenSearch: drop `index.lifecycle.*` and `index.routing.allocation.include._tier_preference`.
     * OpenSearch → Elastic: drop `index.replication.type`, `index.plugins.*` and `index.opendistro.*`.
     * Default: on for cluster→cluster when the flavors differ; off for file restores, where a failure prints a hint (Q5).
  5. Create each index with `PUT /{target}` and body `{"settings":…}`. Target selection:
     * the output URL index (with prefix/suffix) if present, otherwise the name from the file;
     * a file holding several indices plus an explicit output index → error. Node would PUT them all to the same index.
  6. Elastic < 7 with `type=settings`: unwrap `.settings` (parity).
  7. If the index already exists → error, unless `--skip-existing` (extension). Optional P2: then apply only the dynamic subset via `PUT /{target}/_settings`.
* **Full-restore order: settings → mapping → data.** Document this in the README, and exercise exactly this order in the integration tests.

### 6.6 Bulk writer
* **URL.** `PUT {base}/{outputIndex}/_bulk` when the output index is known, else `PUT {base}/_bulk`. The query string is `--output-params`, falling back to `--params`.
* **Action line.** Hand-built into a `Vec<u8>`; string values are escaped with sonic-rs.
  * Shape: `{"<action>":{"_index":…,"_id":…[,"_type"][,"routing"][,"version","version_type"][,extras]}}`.
  * `_index` = `--output-index` (percent-decoded) or `doc._index`, with prefix/suffix applied in both cases.
  * `_type` only when the target is Elastic < 7. **It is dropped for ≥ 7 and OpenSearch**, which is what makes 6.x → 7+/OS restores work.
  * Meta fields `routing`, `parent`, `timestamp`, `ttl` and the extras come from `doc.X`, then `doc._X`, then `doc.fields.X`, then `doc.fields._X`. Use the bare names in the action line: ES ≥ 7 and OpenSearch reject `_routing` there.
  * `timestamp`/`ttl` are dropped for ES ≥ 5 targets, with a one-time warning.
  * `delete` gets no body line.
* **Body size.** A batch holds `--limit` docs. Extension `--maxBulkBytes` (default 100 MiB, under the default `http.max_content_length`) splits oversized batches into several bulk calls.
* **Compression.** With `--esCompress`, gzip the body; bodies over 1 MiB are compressed on `spawn_blocking`.
* **Response parsing.** Read `errors` first, with a sonic-rs pointer lookup.
  * `errors:false` → writes = number of items.
  * Otherwise, walk `items[*].<action>.status`:
    * status < 400 → written;
    * **429 or 503** (`es_rejected_execution_exception`) → item-level retry with backoff when retries are enabled. This is data-safe: only the rejected items are resent, and rejected items were never indexed. Extension, on by default when `retryAttempts > 0`;
    * any other status ≥ 400 → if `ignore-es-write-errors` (default true), print the item JSON to stderr (parity) and count it as failed; otherwise fail with the first item error.
* **Accounting.** Each batch returns `WriteReport { written, failed, retried }`, aggregated into the final summary. P2 extension: `--failedItemsFile <path>` writes the original lines of failed docs as NDJSON for replay.
* **Refresh.** Unless `--noRefresh`, send `POST /{index|_all}/_refresh` after each bulk call (parity). With `concurrency > 1`, keep at most one refresh in flight per index.

### 6.7 Files, compression, splitting
* **Writer stack:** `StagedFile` → `BufWriter` (1 MiB) → optional gzip encoder at `compressionLevel` → line writer.
  * The commit is atomic per file.
  * On failure, every staged part is removed and existing destinations are left untouched (extends today's guarantee).
* **Splitter:**
  * Counts uncompressed bytes (line + newline) or rows, and rotates **after** the line that crosses the threshold (parity).
  * Part names follow Node's `path.join(dir, base + ".split-" + n + ext + (gz ? ".gz" : ""))`.
  * Without `--overwrite`, it refuses if **any** `base.split-*` file already exists.
* **Reader:**
  * Streaming gunzip with `multiple_members(true)`, so `cat`-concatenated parts work.
  * Gzip is auto-detected from the magic bytes.
  * A truncated gzip stream is an error. Node accepts it silently; `--ignore-errors` downgrades it to a warning.
* **stdin/stdout:** match Node exactly. `fsCompress` gunzips stdin on read [verified in `file.js`: stdin goes through the same pipe]. On write, Node only gzips when the output is a file, not `$` [verified: the `$` branch sets `process.stdout` without gzip].
* **Line endings:** `\n` everywhere (the tool is Unix-only).

### 6.8 Retries
* One `RetryPolicy { attempts, base_delay, backoff: Fixed | Exponential{cap}, retry_on }` is used by every request.
* **Defaults (parity):**
  * `--retryAttempts 0`, `--retryDelay 5000` ms, **fixed** delay.
  * **Implicit transport retry**, mirroring `requestretry`'s `maxAttempts: retryAttempts || 5`. Even at 0, the safe-to-repeat requests are retried on network errors or 5xx, up to 5 attempts in total. Those requests are: `GET /`, `_mapping` and `_settings` GETs, the initial search, PIT pages and bulk.
  * Extension `--noImplicitRetry` turns this off (Q2).
* **Retry set.** Node's list maps to:
  * connect, DNS and TLS-handshake failures;
  * connection reset and broken pipe;
  * timeouts;
  * HTTP 5xx.

  We add **429 and 408** (Node does not retry 429, the most important case for bulk). Extension `--retryOn network,timeout,5xx,429,408,502,503,504` customises the set; this covers the "retry on specific errors" requirement.
* **Data-safety rules** (kept from elasticdump-rs):
  * **Scroll continuations** retry only on 429 or connect-phase errors.
  * **Bulk** whole-request retry:
    * safe for `index` and `delete` with ids, and for `update`;
    * for `create`, a 409 on a replay counts as success only when that replay came from our own retry, tracked per batch;
    * for `index` without `_id`, retrying after an ambiguous failure can duplicate docs. Retry anyway (Node does), and warn once.
* **Backoff.** Fixed by default. `--retryBackoff exponential` (30 s cap, full jitter) is the setting recommended in the docs.

### 6.9 Concurrency and throttling
* **`--concurrency N`:** a `JoinSet` with at most N in-flight sink writes. File sinks force N = 1 and warn.
* **`--throttleInterval ms`:** sleep after each enqueue (default 1 ms, parity).
* **`--intervalCap` / `--concurrencyInterval` / `--carryoverConcurrencyCount`:** an `IntervalLimiter`.
  * A fixed window resets every `concurrencyInterval`, and `acquire()` waits while `started_in_window ≥ intervalCap`.
  * With carryover, requests still in flight at the reset are counted against the new window.
  * It applies to HTTP requests on both endpoints, as p-queue did.
  * It is active **only when one of these flags is passed explicitly**. Applying the defaults (5 per 5 s) implicitly would throttle everything to 1 request/s.
* **`--maxSockets`:** cap the connection pool and add a `Semaphore(n)` around `send()`.
* **Tuning guide.** Read parallelism (`--slices`/`--workers`) is separate from `--concurrency`, which only controls writes. The README gets a recommended-settings table for ES→ES: `--slices` = shard count, `--concurrency 4`, `--limit 1000–5000`, `--noRefresh`.

### 6.10 Auth and TLS
* **Resolution per direction:** `TlsCfg { ca, cert, key, pass, insecure }`, taken from the shared flag, else the direction flag, else the rs alias. The shared flag wins (Node parity); warn if both are set and differ.
* **Server verification:**
  * Roots are the custom CA bundle (every cert in the PEM) or the native roots.
  * `insecure` installs an accept-all `ServerCertVerifier` and prints a warning.
* **Client auth:** `with_client_auth_cert(chain, key)`. Encrypted PKCS#8 keys are decrypted with `EncryptedPrivateKeyInfo::decrypt(pass)`.
* **Hostname verification** is always on. The OpenSearch demo cert's SANs include `localhost` and `127.0.0.1` (verified).
* **Basic auth plus a client cert** together are supported (auth header plus TLS identity).

---

## 7. Testing strategy

### 7.1 Unit tests (`cargo test --lib`)
* **CLI:**
  * every flag parses in both `=` and space form;
  * loose booleans, JSON flags, `@file` relative to the CWD;
  * out-of-scope flag actions;
  * a table-driven set of validation errors.
* **Endpoint and URL:**
  * cases ported from elasticdump's `test/parse-base-url.tests.js` and `test/is-url.tests.js` (including percent-encoded URLs);
  * base paths, prefix/suffix, `--input-index` overrides, `$`.
* **Auth:** precedence (URL, then httpAuthFile, then env); port `test/add-auth.tests.js` together with its `httpAuthTest.ini` and `malformedHttpAuth.ini` fixtures.
* **Extraction:**
  * the existing tests;
  * `RawDoc::meta` for every meta-field spelling;
  * elasticdump's `bigint*.json` fixtures, which must round-trip **byte-identically**.
* **Bulk builder:**
  * golden NDJSON for each bulkAction × {ES6 typed, ES7, OS} × {routing, parent, version, versionType, extras} × prefix/suffix;
  * `delete` has no body line; `update` wraps the source in `doc`.
* **Bulk response parser:** the `errors:false` fast path, mixed statuses, 429-item retry selection, ignore vs fail.
* **Settings:**
  * exclude-path removal;
  * string-literal encode/decode round-trip;
  * cross-flavor stripping;
  * ES < 7 unwrap.
* **Mapping:**
  * typed vs typeless detection, including a field literally named `properties` (the #667/#1007 cases and `properties_mapping.json`);
  * parent ordering for ES < 7.
* **Splitter:**
  * naming, with and without an extension and with `.gz`;
  * rotation thresholds;
  * the bytes grammar (`10mb`, `1.5gb`, `512kb`, `100b`).
* **Gzip:** write→read round-trip, multi-member streams, truncated streams.
* **Retry:** a classifier matrix (mode × outcome × policy); backoff timing under `tokio::time::pause()`.
* **IntervalLimiter:** exact admission counts per window, with and without carryover, under paused time.
* **proptest:** random JSON docs (unicode, escapes, big ints, deep nesting) → file sink → file source → bulk builder; `_source` must come out raw-equal.

### 7.2 Mock-server contract tests (no containers)
Grow `tests/support/mock_es.rs` into a router that records method, path, query, headers and body (or switch to `wiremock`). Assert:
* the exact endpoints per flavor: PIT open/close, `_bulk` path and `Content-Type`, refresh calls, mapping PUTs, settings PUT bodies;
* per-direction headers and params;
* request gzip under `--esCompress`;
* retry counts and delays;
* early stop for `--size` and `--offset`;
* scroll clear and PIT close on success, on error and on SIGINT.

These run in every CI job.

### 7.3 Podman integration harness
**Layout**
```
scripts/it/
  up.sh <profile>   # start containers, wait for health, write $IT_DIR/env
  down.sh           # remove containers by label (always via trap)
  certs.sh          # copy demo certs out of the container; optionally generate our own CA/client with openssl
  opensearch-security/config.yml   # clientcert_auth_domain.http_enabled: true (non-admin mTLS test, P2)
  compose/opensearch-plain.yaml, opensearch-secure.yaml, elasticsearch-7.yaml, elasticsearch-8.yaml   # for `podman compose` users
tests/it/            # behind cargo feature "it"; reads IT_* env vars
  fixtures/          # mapping_typeless.json, mapping_typed_6x.json, settings_analyzer.json,
                     # docs_adversarial.ndjson, bigint.ndjson, routing.ndjson, docs_6x_with_type.ndjson
  roundtrip.rs  tls.rs  compression.rs  split.rs  concurrency.rs  retry.rs  parity_vs_node.rs
```

**Containers** (verified on the box: podman 5.4.2, rootless, overlay). `up.sh` uses plain `podman run`, so `podman-compose` is not required.

Plain OpenSearch:
```bash
podman run -d --name it-os-plain-$RUN_ID --label elasticdump-rs-it=1 -p 127.0.0.1:${PORT:-19200}:9200 \
  -e discovery.type=single-node -e DISABLE_SECURITY_PLUGIN=true -e DISABLE_INSTALL_DEMO_CONFIG=true \
  -e OPENSEARCH_JAVA_OPTS="-Xms512m -Xmx512m" docker.io/opensearchproject/opensearch:${OS_TAG:-2}
```

Secure OpenSearch (security plugin with demo TLS):
```bash
podman run -d --name it-os-sec-$RUN_ID --label elasticdump-rs-it=1 -p 127.0.0.1:${PORT:-19201}:9200 \
  -e discovery.type=single-node -e OPENSEARCH_INITIAL_ADMIN_PASSWORD="$IT_ADMIN_PW" \
  -e OPENSEARCH_JAVA_OPTS="-Xms512m -Xmx512m" docker.io/opensearchproject/opensearch:${OS_TAG:-2}
podman cp it-os-sec-$RUN_ID:/usr/share/opensearch/config/root-ca.pem  "$IT_DIR/root-ca.pem"
podman cp it-os-sec-$RUN_ID:/usr/share/opensearch/config/kirk.pem      "$IT_DIR/client.pem"
podman cp it-os-sec-$RUN_ID:/usr/share/opensearch/config/kirk-key.pem  "$IT_DIR/client-key.pem"
openssl pkcs8 -topk8 -v2 aes-256-cbc -in "$IT_DIR/client-key.pem" -out "$IT_DIR/client-key-enc.pem" -passout pass:secret
```

Notes on the secure profile:
* The admin password must pass the strength policy; OpenSearch 2.12 and later require it.
* `kirk` is the demo **admin DN** and authenticates over REST by certificate alone (verified with curl and with Node `--tlsAuth`).
* A non-admin client-cert test needs our own CA and client cert plus a `config.yml` with `clientcert_auth_domain.http_enabled: true` (P2).

Harness behaviour:
* **Health:** poll `/_cluster/health?wait_for_status=yellow&timeout=60s` for up to 120 s. The security plugin needs a few extra seconds after the port opens (observed).
* **Memory:** budget about 1 GiB per node. Run the profiles **sequentially** on small runners. Build the binary **before** starting containers, using a no-LTO `it` profile: on the box, a fat-LTO release build running next to OpenSearch hit the 15.6 GB OOM limit and OpenSearch was killed.
* **Isolation:** bind to 127.0.0.1 on configurable ports. Suffix names with `$RUN_ID` and clean up by label.
* **Fault injection (P2):** a `ghcr.io/shopify/toxiproxy` container in front of OpenSearch for the retry tests.

**Seed data**
* Index `src`: 2 shards, 0 replicas, a custom analyzer.
* Mappings: keyword, long, analyzed text, date, nested, object, geo_point, a field literally named `properties`, and one `dynamic: strict` object.
* Custom `routing` on a subset of docs.
* 10k deterministic docs (seeded RNG), plus adversarial docs: unicode, escapes, integers > 2^53, empty objects, deep nesting, one 1 MB doc.

**Round-trip suite** (runs per profile)
1. `settings` → file: a single string-literal line; once decoded, it contains no `uuid`, `creation_date`, `version` or `provided_name`.
2. `mapping` → file.
3. `data` → file, in several variants: scroll, PIT, sliced PIT, `--fsCompress`, `--fileSize 64kb`, `--maxRows 1000`. `--sourceOnly` gets its own check (no metadata in the output).
4. Restore into `dst` in order (settings, mapping, then data from every part or gz file). Variants: `--limit 500 --concurrency 4`; `--noRefresh` plus an explicit refresh at the end; `--bulkAction create`.
5. **Compare** source and destination:
   * `_count` is equal;
   * `_mapping` is equal (after substituting the index name);
   * `_settings` is equal modulo the excluded and server-assigned keys;
   * the docs are equal. Fetch both indices sorted by `_id` with the harness's own client and compare `(_id, _routing, _source)` as parsed JSON using `arbitrary_precision`;
   * a `term` query returns the same count on both, and `_analyze` with the custom analyzer gives identical tokens.
6. **Cluster → cluster:** secure → plain and plain → secure, with per-direction CA/cert flags and per-direction headers.
7. **Filtering:** `--searchBody` inline and via `@file` with a range query; the restored count equals `_count` for the same query.
8. **Limits:**
   * `--size 1234` → exactly 1234 docs;
   * `--offset 100` on file input skips the first 100 lines;
   * `--offset 100` on ES input skips 100 docs (count check).
9. **Retries** through toxiproxy: inject `reset_peer`, `timeout` and latency. The run succeeds with retries on and fails under `--noImplicitRetry --retryAttempts 0`. A 429-bulk test using `thread_pool.write.queue_size: 1` is P2 and `#[ignore]` by default, since it is flaky.
10. **TLS negatives:**
    * no CA → fails with a clear message;
    * wrong CA → fails;
    * `--insecure` → works;
    * `NODE_TLS_REJECT_UNAUTHORIZED=0` → works;
    * mTLS-only user without a cert → 401;
    * encrypted key with the wrong `--pass` → clear error.
11. **Differential parity vs Node** (optional job, `IT_NODE=1`). Install with `npm i elasticdump@6.125.1` in a temp dir (works on the box). Run identical command lines through both tools and compare semantically:
    * settings: string equality after decoding;
    * mapping: equality;
    * data: the set of `(_index, _id, _source)` tuples.

    Also cross-restore Node dumps with Rust and Rust dumps with Node. The Rust-data → Node direction was verified manually.

**Version matrix**
* OpenSearch `2` (2.19.7 on the box) and `3` [uncertain: confirm the tag], each × {plain, secure}.
* Optional ES `7.17` / `8.x` profiles.
* ES 6.x `_type` handling is covered with **fixture files**; no ES 6 container is needed.

**Running locally**
```bash
cargo build --profile it
scripts/it/up.sh opensearch-plain  && cargo test --features it --test it; scripts/it/down.sh
scripts/it/up.sh opensearch-secure && IT_PROFILE=secure cargo test --features it --test it; scripts/it/down.sh
just it        # wraps both profiles sequentially
```

**CI** (GitHub Actions; ubuntu-24.04 runners include podman):

| Job | Runs | Content |
|---|---|---|
| `unit` | every PR | fmt, clippy `-D warnings`, lib and mock tests |
| `it` | every PR | matrix `os_tag: [2, 3]` × `profile: [plain, secure]`; caches cargo, and the image via `podman save`/`load` |
| `parity-node` | nightly | the differential parity suite |
| `bench` | manual dispatch | §8 benchmarks |

On failure, upload the logs and `podman logs`.

### 7.4 Existing tests
Replace the hard-coded `http://localhost:9200` in `tests/integration_test.rs` with `IT_ES_URL` (same default) so the suite also runs against podman OpenSearch. Gate ES-only assertions behind capability checks.

---

## 8. Performance targets and benchmarking

| Scenario | Target |
|---|---|
| ES → file, defaults, unsliced | **No regression** vs `b32b6e9` (±5%) on the existing benchmark; ≥ 1.8× Node docs/s |
| ES → file, sliced (scroll with slices = shards, or PIT with 8–16 slices) | ≥ 3× Node |
| file → ES (`--limit 5000 --concurrency 4 --noRefresh`) | ≥ 3× Node with the same flags (cluster-bound: also report the tool's CPU) |
| ES → ES | ≥ 2.5× Node at equal `--limit` / `--concurrency` |
| `--fsCompress` write | ≥ 150 MB/s uncompressed input per core (level 6) |
| Peak RSS | ≤ `(bufferSize + prefetch + concurrency) × limit × avgDoc × 1.5 + 50 MB`; 2M docs × ~700 B at defaults stays under 300 MB |
| CPU | ≥ 3× fewer CPU-seconds than Node for the same work |

**Method**
* Extend `scripts/benchmark/compare-with-elasticdump.sh` with:
  * `--engine podman` and `--target opensearch`;
  * `--scenario dump|restore|copy|dump-gzip|dump-split`.
* Reuse the script's 2M-doc dataset, its warmup/runs logic and `taskset` pinning.
* Record wall time, docs/s, MiB/s, CPU seconds and max RSS (`/usr/bin/time -v`).
* Criterion microbenchmarks: batch extraction, NDJSON bulk build (10k docs), bulk response parse (clean and mixed), gzip, splitter.
* Store results as JSON artifacts from the manual `bench` CI job and compare them against main.

---

## 9. Milestones (ordered; each is a mergeable PR series with green CI)

### M0: Foundation and harness skeleton (~3–4 days)
1. Add `rust-toolchain.toml`, a `justfile`, an `it` Cargo profile (no LTO, `codegen-units = 16`) and an `it` feature.
2. Write `scripts/it/{up,down,certs}.sh` for the `opensearch-plain` and `opensearch-secure` profiles.
3. Parameterise the existing integration tests with `IT_ES_URL` and run them against OpenSearch. PIT tests are marked as expected failures until M2.
4. Add CI jobs `unit` and `it` (plain profile only for now).
5. Commit §3 as `docs/PARITY.md` with checkboxes.

**Acceptance criteria:**
* `just it` starts OpenSearch, runs the existing scroll tests green, and tears down, both on ubuntu-24.04 CI and on the box.
* CI is green.

### M1: Core refactor, HTTP client, CLI and endpoints (~1.5–2 weeks)
1. Build `http::EsClient` on reqwest and rustls. Port search, scroll, clear-scroll and PIT open/close to it, then remove the `elasticsearch` crate.
2. Add `http::info` (flavor and version, fetched lazily).
3. Add `http::tls`: CA, insecure, client cert and key, `--pass`, `NODE_TLS_REJECT_UNAUTHORIZED`.
4. Add `endpoint::{url,auth}`: Node URL parity plus base paths, `--input-index`/`--output-index`, env-var credentials, `--httpAuthFile`.
5. Rewrite the CLI:
   * all in-scope flags with exact spellings, plus the minimist pre-pass;
   * the out-of-scope table and validation;
   * `--timeout` in ms.
6. Add headers and params, globally and per direction. Default `User-Agent: elasticdump`.
7. Make the default searchBody match Node (`stored_fields:["*"]`, `_source:true`), with `@file` resolved relative to the CWD.
8. Build the `Source`/`Sink`/`pipeline` skeleton. ES→file runs through it unchanged, keeping the `Batch::Ndjson` fast path.
9. Add `FileSource` (NDJSON plus the concatenated-JSON fallback, stdin) and `FileSink`, so file→file works.
10. Change the defaults: `limit = 100`, `pitKeepAlive = 5m` (pending Q1).

**Acceptance criteria:**
* The existing tests are green (with their defaults adjusted), and the benchmark is within ±5%.
* On secure OpenSearch, each of these passes a dump test: `--input-ca`; `--input-cert`/`--input-key` (kirk); the encrypted key with `--input-pass`; `--insecure`; URL credentials; env credentials; `--httpAuthFile`.
* Out-of-scope flags warn or error per the table.
* A mock test shows `--headers` applied to every request.
* `elasticsearch` no longer appears in `Cargo.lock`.

### M2: Data restore and cluster→cluster (~2 weeks)
1. Implement `RawDoc` and `DocMeta` extraction.
2. Implement `BulkSink`:
   * all four bulkActions, with `_type` decided by the target version;
   * prefix/suffix and `--output-index`;
   * request gzip and `--maxBulkBytes`;
   * the response parser, both ignore flags, and refresh/`--noRefresh`;
   * (P2) `--failedItemsFile`.
3. Wire ES→ES: the extractor emits `Batch::Docs`.
4. Make PIT flavor-aware, with the OpenSearch default sort (`_doc` + `_id`). Map `--searchAfter`/`--pit` onto it, and add non-PIT search_after with a required sort.
5. Add `--size` (including with slices), `--offset`, `--scrollId`, `--sourceOnly`, and accept `--scroll-with-post`.
6. Integration tests:
   * file→OS and OS→OS data round-trips;
   * sliced PIT on OpenSearch;
   * size and offset;
   * bulk errors: a `strict` mapping causes rejections; assert counts, stderr items and exit codes under both ignore settings.

**Acceptance criteria:**
* Data round-trips (§7.3 step 5) are equal, plain and secure, through a file and cluster→cluster: 10k docs plus the adversarial set, routing preserved, big ints byte-identical.
* A 6.x `_type` fixture restores into OS 2.x.
* PIT on OpenSearch, sliced and unsliced, returns `_count` docs.
* `--size 1234` returns exactly 1234 docs.

### M3: Mapping and settings (~1 week)
1. Implement the mapping source and sink (§6.4), including the typed→typeless unwrap and the `properties` edge cases.
2. Implement the settings source and sink (§6.5):
   * the string-literal format and exclusions on all indices;
   * cross-flavor stripping;
   * `--skip-existing`;
   * multi-index handling.
3. Make both file formats interchangeable with Node.
4. Integration test: a full restore (settings → mapping → data), comparing mappings, settings, the analyzer and docs.

**Acceptance criteria:**
* A fresh-index restore reproduces `_mapping` exactly, `_settings` modulo excluded keys, a working custom analyzer, and equal docs.
* Rust settings/mapping files restore with Node elasticdump 6.125.1, and Node's files restore with Rust (`IT_NODE=1`).
* A host-only multi-index mapping/settings dump and restore works.

### M4: Compression and splitting (~4–5 days)
1. Add gzip write (`--fsCompress`, `--compressionLevel`) for single files and for split parts, and gzip read with auto-detect and multi-member support.
1a. **Scope addition (2026-10-28):** zstd as a second codec (`--compressionType zstd`, level 1–22), for single files, split parts (`.zst`, one complete stream per part) and mapping/settings files; zstd read auto-detected by magic bytes.
2. Add the splitter (`--fileSize`, `--maxRows`): Node naming, atomic multi-part commit, overwrite rules.
3. Optional: directory/glob input for split sets.

**Acceptance criteria:**
* With `--fileSize 64kb` (with and without gzip):
  * part names match a Node run exactly;
  * every non-final part is ≥ the threshold and < threshold + one line;
  * restoring all parts gives full equality.
* Node reads Rust's `.gz` files and vice versa.
* zstd: dump (single file and `--fileSize` split) → restore gives full equality; every part decodes on its own.
* Gzip throughput meets §8.

### M5: Concurrency, throttling and retries (~1 week)
1. Unify `RetryPolicy` across all requests: implicit Node-style retry, `--retryOn`, `--retryBackoff`, and the bulk item-level 429 retry.
2. Add the write pool (`--concurrency`), `--throttleInterval`, `IntervalLimiter` (`--intervalCap`, `--concurrencyInterval`, `--carryoverConcurrencyCount`) and `--maxSockets`.
3. Add `--searchWithTemplate` and `--format human`.
4. Integration tests:
   * the toxiproxy suite;
   * ES→ES with `--concurrency 8` gives full equality;
   * `--intervalCap 2 --concurrencyInterval 1000` over 10 batches takes ≥ 4 s.

**Acceptance criteria:**
* Fault-injection tests pass 20 runs out of 20.
* No duplicates with explicit ids under retries.
* Limiter and throttle timings are within ±10%.

### M6: Hardening, performance, docs, release (~1 week)
1. Benchmark dump, restore and copy against Node on OpenSearch and ES 7.17, verify §8, and profile the `_id`-tiebreaker cost (Q4).
2. Add the OpenSearch 3.x profile and the optional ES 7.17/8.x jobs to the CI matrix.
3. Rewrite the README: migration guide, flag compatibility, the list of intentional divergences, restore order, security notes, the memory formula.
4. Release workflow: static musl x86_64/aarch64 builds. Nothing is published without Robert's approval.
5. Stretch: `--type index`, `--delete`.

**Acceptance criteria:**
* §8 targets are met or documented with numbers.
* The CI matrix is green.
* Every in-scope row in `docs/PARITY.md` is checked.

**Rough total:** 7–9 engineer-weeks. After M2, M3 and M4 can proceed in parallel.

---

## 10. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Hot-path regression during the M1 refactor | Loses the main selling point | Leave the `Batch::Ndjson` path untouched; benchmark gate in M1 acceptance |
| OpenSearch PIT tiebreaker semantics (verified loss with `_doc` only) | Silent data loss | `_id` tiebreaker on OpenSearch; count-equality tests with ≥ 2 shards and with slices; scroll stays the default |
| `_id` sorting slow, or disallowed (ES 8 forbids `_id` fielddata by default; OpenSearch could change) | Slow or failing PIT | OpenSearch only; probe at startup; fall back to an error suggesting a user sort or scroll |
| Settings that cannot be applied on create (cross-version or cross-flavor) | Restore fails | Exclude list, cross-flavor stripping, errors that name the offending key; grow the list from IT failures |
| Mapping incompatibilities across majors (`string`, `_all`, types) | Restore fails | Typed unwrap and `_all` stripping; anything else surfaces the server error verbatim (auto-conversion is out of scope) |
| Bulk retries duplicating docs that lack `_id` | Duplicates | Warning; item-level retry only for rejected items; docs recommend `create` with ids |
| Legacy encrypted PEM keys unsupported | mTLS users blocked | Clear error with an openssl conversion command; optional `native-tls` feature |
| Podman CI flakiness (memory, startup, rootless networking) | Flaky CI | Sequential profiles, 512 MB heaps, health polling, no-LTO builds; retry container startup only, never tests |
| Node-quirk compatibility vs correctness | Surprise either way | Prefer correctness; list every divergence with its rationale in the README |
| Scope creep (templates, aliases, S3) | Delays | Hard out-of-scope table with explicit errors and warnings |

---

## 11. Open questions for Robert
1. **Q1, `--limit` default:** Node's 100 (strict drop-in) or elasticdump-rs's 10000 (fast)? The plan assumes **100** plus a hint.
2. **Q2, retry defaults:** mirror Node's hidden "5 attempts × 5 s on 5xx/network even at `--retryAttempts 0`", or keep elasticdump-rs's explicit 3 × exponential from 1 s? The plan assumes the Node-style implicit retry, plus 429 retry as an extension.
3. **Q3, dead throttling flags:** `intervalCap`, `concurrencyInterval` and `carryoverConcurrencyCount` have done nothing in Node since v6.119.0. Implement them with the historical semantics, active only when passed (the plan), or accept them as no-ops?
4. **Q4, OpenSearch PIT default sort:** `_doc` + `_id` tiebreaker (correct, maybe slower), require a user sort, or refuse PIT on OpenSearch and steer to scroll?
5. **Q5, cross-flavor settings stripping** (ES ↔ OpenSearch keys): on by default for cluster→cluster when flavors differ (the plan), opt-in for file restores?
6. **Q6, target versions:** OpenSearch 2.x/3.x only, or also Elasticsearch 7.17/8.x (and live ES 6.x clusters) in CI?
7. **Q7, fork strategy:** fork `tenyears-tech/elasticdump-rs` (MIT, single maintainer, self-described as AI-written and experimental) and upstream the work, or hard-fork under a new name? Should the binary also install as `elasticdump` for true drop-in use?
8. **Q8, Rust-only extensions in v1** (`--failedItemsFile`, `--retryOn`, `--maxBulkBytes`, split-set directory input, `--settingsFormat object`), or strict parity first?
9. **Q9:** confirm that `--delete` and `--type index` stay out of v1 (the plan has them as M6 stretch goals).

---

## Appendix A: Verification log (run on the box, 2026-10-28)
* **elasticdump-rs:** anonymous `git clone https://github.com/tenyears-tech/elasticdump-rs` worked (the repo is public). HEAD is `b32b6e9`, and `cargo test --lib` reports **82 passed**.
* **elasticdump:** cloned `elasticsearch-dump` with full history and read the files listed in §2.
  * `git tag --contains 6ace8af` gives v6.119.0, the p-queue removal.
  * `dc001c2`, which removed the concurrency docs, first shipped in v6.94.1.
* **The `elasticsearch` 8.19.0-alpha.1 crate:** it sends `Accept/Content-Type: application/json` (no compatibility header), and `Credentials` is a single-valued enum.
* **Plain OpenSearch** (`opensearchproject/opensearch:2`, which resolved to 2.19.7): seeded 1000 docs on 2 shards with routing.
  * elasticdump-rs scroll and sliced dumps: OK.
  * elasticdump-rs PIT: HTTP 400 on `/_pit`.
  * Node 6.125.1 settings, mapping and data dump and restore: OK. The settings file is a JSON string literal.
  * Node `--fileSize=20kb --fsCompress` produced `out.split-{0..5}.json.gz`.
  * OpenSearch PIT: `_shard_doc` sort is an error; `_doc` returns 983/1000 docs; `_doc` + `_id` returns 1000/1000.
  * Scroll with `from=10` is rejected (`using [from] is not allowed in a scroll context`).
* **Secure OpenSearch** (security plugin, demo TLS):
  * elasticdump-rs with `--caFile` and basic auth: OK. Without a CA: TLS error. With `--insecure`: OK.
  * Node with `--tlsAuth --cert kirk.pem --key kirk-key.pem --ca root-ca.pem`: OK.
  * Node with `--ca` but no `--tlsAuth`: `unable to verify the first certificate`.
* **OOM:** OpenSearch was killed while a fat-LTO `cargo build --release` ran at the same time (15.6 GB box limit).
