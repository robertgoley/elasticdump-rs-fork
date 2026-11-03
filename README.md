# elasticdump-rs-fork
AI powered rewrite of elasticdump based on previous partial work from the elasticdump-rs project.  Many new types and features added.  Aim is feature parity with elasticdump nodejs version with increased performance and a few add on features for usability or speed.

# elasticdump-rs

A Rust re-implementation of the parts of [elasticdump](https://github.com/elasticsearch-dump/elasticsearch-dump)
that move **data, mappings and settings** between Elasticsearch/OpenSearch clusters and files, in both
directions, with elasticdump's flag names and file formats so existing scripts keep working.

> Branch `rewrite`: local fork of `tenyears-tech/elasticdump-rs`. Plan: [`docs/PLAN.md`](docs/PLAN.md);
> parity checklist: [`docs/PARITY.md`](docs/PARITY.md).

## What it does

| Direction | `--type data` | `--type mapping` | `--type settings` |
|---|---|---|---|
| cluster → file / stdout | ✅ scroll, search_after, PIT (sliced) | ✅ | ✅ (elasticdump's string-literal format) |
| file / stdin → cluster | ✅ `_bulk` | ✅ (typed → typeless unwrap) | ✅ (creates the index) |
| cluster → cluster | ✅ | ✅ | ✅ (engine-specific settings auto-stripped) |
| file → file | ✅ (normalise, offset/size, sourceOnly) | – | – |

Plus: basic auth (URL, `--httpAuthFile`, `ELASTICDUMP_{INPUT,OUTPUT}_{USERNAME,PASSWORD}`), TLS with custom CA,
client certificates (encrypted PKCS#8 keys via `--pass`) and insecure mode per direction; gzip **and zstd**
files; file splitting; write concurrency, throttling and rate limiting; Node-style retries plus 429.

Tested against **OpenSearch 2.19.7 and 3.9.0** and **Elasticsearch 9.1.2 and 8.6.1** (`bitnamilegacy/elasticsearch`),
each plain and secured (basic auth + TLS), across engines (ES ↔ OpenSearch, files and cluster to cluster), and
against Node elasticdump 6.125.1 for file-format interop.

## Install

```bash
cargo build --release            # fat LTO; needs a few GB of RAM
cargo build --profile it         # lighter optimised build (no LTO), used by the test harness
# binary: target/release/elasticdump-rs (or target/it/elasticdump-rs)
```

Allocator: builds for glibc Linux (`*-linux-gnu`, the preferred release artifacts and the default container image)
use mimalloc as the global allocator; musl builds (`*-linux-musl`, the Alpine image) keep musl's own allocator.
Features: `--features system-allocator` builds glibc Linux with glibc's malloc (mimalloc's C code is still compiled
but not linked in), `--features jemalloc` uses jemalloc on any target, `--features mimalloc` forces mimalloc on any
target (also musl). `jemalloc` and `mimalloc` are mutually exclusive, and `system-allocator` excludes both.
Release builds (`.github/workflows/release.yml`, `cargo zigbuild`): `x86_64`/`aarch64-unknown-linux-gnu` against
glibc 2.28 (preferred), plus static `x86_64`/`aarch64-unknown-linux-musl`.
Each archive `elasticdump-rs-<target>.tar.gz` holds a directory of the same name with the binary, `LICENSE` and
`THIRD-PARTY-NOTICES.md`; the container images carry both files in `/usr/share/doc/elasticdump-rs/`.

## Migrating from elasticdump

Replace `elasticdump` with `elasticdump-rs`; the flags are spelled the same (`--input-index`, `--fsCompress`,
`--ignore-es-write-errors`, booleans with or without `=true`). A full backup/restore:

```bash
# backup
for t in settings mapping data; do
  elasticdump-rs --input=https://user:pw@os:9200/logs --output=logs.$t.json --type=$t --ca=ca.pem
done
# restore (order matters: settings create the index, then mapping, then data)
for t in settings mapping data; do
  elasticdump-rs --input=logs.$t.json --output=https://user:pw@os:9200/logs --type=$t --ca=ca.pem
done
```

Other common forms:

```bash
elasticdump-rs --input=http://os:9200/src --output=http://os:9200/dst --limit 1000 --concurrency 4  # copy
elasticdump-rs --input=http://os:9200/src --output=dump.json --fileSize=500mb --fsCompress           # gzip parts
elasticdump-rs --input=http://os:9200/src --output=dump.json --compressionType zstd                 # zstd (opt-in)   
elasticdump-rs --input=dump.json.zst --output=http://os:9200/dst                                     # auto-detected
elasticdump-rs --input=http://os:9200/src --output=$ --searchBody=@query.json | jq .                # stdout
elasticdump-rs --input=http://os:9200/src --output=d.json --searchAfter --pit --slices 4            # sliced PIT
elasticdump-rs --input=http://os:9200 --output=all.mapping.json --type=mapping                      # all indices
elasticdump-rs --input=all.mapping.json --output=http://other:9200 --type=mapping --prefix=copy_    # restore each
```

Restoring split dumps takes one invocation per part (as in Node): `for f in dump.split-*.json.gz; do … --input=$f …; done`.

## Flags

`elasticdump-rs --help` lists everything. Groups:

* **Endpoints/types:** `--input`, `--output` (URL, file, or `$`), `--input-index`/`--output-index` (`index[/type]`),
  `--type data|mapping|settings|index|analyzer|alias|template|component_template|index_template|policy|script`,
  `--overwrite`, `--prefix`/`--suffix`, `--quiet`, `--debug`, `--format human`.
* **Retrieval:** `--limit` (default 100), `--offset`, `--size`, `--searchBody` (JSON or `@file`),
  `--searchWithTemplate`, `--scrollTime`, `--scrollId`, `--searchAfter`, `--pit`, `--pitKeepAlive`,
  `--sourceOnly`, `--esCompress`, `--headers`/`--params` (+ `--input-*`/`--output-*`).
* **Writing:** `--bulkAction index|create|update|delete`, `--handleVersion`, `--versionType`, `--noRefresh`,
  `--ignore-errors`, `--ignore-es-write-errors` (default: continue past failed items), `--parseExtraFields`.
* **Files:** `--fsCompress`, `--compressionType gzip|zstd`, `--compressionLevel`, `--fileSize`, `--maxRows`.
  `--compressionType` wins over `--fsCompress` when both are given. `--offset` cannot be combined with `--slices`
  (sliced results interleave, so there is no stable "first N" to skip); `--skip-existing` (metadata restores) and
  `--overwrite` (file output) are independent.
* **Throughput:** `--concurrency`, `--throttleInterval`, `--intervalCap`, `--concurrencyInterval`,
  `--carryoverConcurrencyCount`, `--maxSockets` (a hard cap on requests in flight, and so on connections; it
  wins over a higher `--concurrency`), `--timeout` (ms of server *silence*, as in Node's `request`;
  a slow but progressing response is not cut off; overrides `--idleTimeout`).
* **Retries:** `--retryAttempts` (0 = Node's 4 implicit retries), `--retryDelay` (5000 ms).
* **Trusted roots:** `--ca`/`--input-ca`/`--output-ca` when given; otherwise the system store (honouring
  `SSL_CERT_FILE`/`SSL_CERT_DIR`). When that store has no usable certificate, the bundled webpki (Mozilla)
  roots are used; if `SSL_CERT_FILE`/`SSL_CERT_DIR` was set, this fallback is logged as a warning naming it.
* **Redirects** are never followed (Node follows them): a 3xx answer stops the run and names the target, so
  credentials and documents only go to the cluster you named. Use the final URL directly.
* **Auth/TLS:** `--httpAuthFile`, `--input-username`/`--input-password`, `--output-username`/`--output-password`
  (see [Credentials](#credentials)), `--tlsAuth`, `--ca`/`--cert`/`--key`/`--pass` and their `--input-*`/`--output-*` forms.
* **Metadata:** `--templateRegex`, `--filterSystemTemplates` (Node), and the extensions `--filterManaged`
  (default true), `--closeIndex`, `--excludeTemplateFields`, `--convertIlmToIsm`, `--lenientPolicyConversion`,
  `--metaFormat string|object`, `--renamePrefix`, `--renameIndex`, `--renameAliases`, `--renamePatterns`.

Rust-only extensions (all opt-in): `--compressionType zstd`, `--slices`, `--workers`, `--bufferSize`,
`--searchType scroll|pit`, `--skip-existing`, `--force` (allow the same file/index as input and output; the output index is compared after `--prefix`/`--suffix`, so `--input …/idx --output …/idx --suffix=-copy` is not refused, `--input …/p-idx --output …/idx --prefix p-` is; a host-only `--output` writes each input index to its own name after `--renamePrefix`/`--renameIndex` and the affixes, so `--input …/idx --output …` is refused unless an affix or rename rule changes the name, and a wildcard input is refused only with neither), `--excludeSettings`, `--maxBulkBytes`, `--retryOn`,
`--retryBackoff`, `--noImplicitRetry`, `--insecure`/`--input-insecure`/`--output-insecure`, `--username`/`--password`
(aliases of `--input-username`/`--input-password`), `--caFile`, `--requestTimeout` (seconds; a deadline for the whole request, body included), `--idleTimeout` (seconds of server silence before a request fails or is retried; default 300, 0 = never; unlike Node, a stalled server cannot hang a run), `--maxResponseMb` (largest response body read into memory, default 256, 0 = unlimited; a bigger page fails with a hint to lower `--limit` instead of an OOM kill), `--allowMerge` (let `--renamePrefix`/`--renameIndex` rename several source indices to one target; refused by default, before anything is read for a cluster source, and before the batch is sent for a file source), `--maxDocMb` (largest single document read from a file or stdin, default 100 like `http.max_content_length`, 0 = unlimited; a bigger one stops the run naming its position instead of being buffered whole), `--allowMissingSource` (dump hits that carry no `_source`: indices whose mapping has `_source.enabled: false`, or a `--searchBody` that turns `_source` off. By default the first hit without `_source` stops the dump before anything of that page is written, and the error names the cause; a search body with `"_source": false` is refused before the first search, naming the search body and suggesting to remove it or pass this flag. **Behaviour change (L-455b):** `"_source": false` used to be allowed without the flag. `stored_fields` without `_source` is refused the same way (L-455a). `"_source": []` is not refused: on OpenSearch 1.3-3.x and Elasticsearch 7.17-9.1 it returns the whole `_source`. **Behaviour change (L-455c):** an empty `_source` (`{}`) is refused like a missing one. A filter that excludes every field (`{"excludes":["*"]}`, which returns `{}` for every hit) is refused before the first search. Otherwise the first hit whose `_source` is `{}` stops the dump, and the error names the document's `_index` and `_id` and the likely cause: the search body's `_source` filter, or a document that is really empty. **A document that is genuinely empty (`{}`) in the source index also needs `--allowMissingSource`.** These dumps used to write `{}`), `--noProxy` (ignore `HTTP(S)_PROXY`/`ALL_PROXY`; when an
environment proxy is used, a warning names it).

Input safety: every hit is written as exactly one NDJSON line (pretty-printed `_source` is flattened),
a bulk input line that is not exactly one JSON object, has duplicate top-level keys, a non-object
`_source`, a non-string `_id`/`routing`/`parent`, a non-integer `version` or an invalid index name
(`*`, `?`, `,`, `_all`, `.`, `..`, leading `-_+`, upper case, > 255 bytes) is rejected on its own and
makes the run exit 1. `--headers` with CR/LF/NUL is a usage error.

Exit status: 0 success; 1 error; 2 usage error; **3 finished, but documents were not written**; 130 / 143 stopped by SIGINT / SIGTERM (both stop in order: the scroll or PIT is closed and staged output removed; a second signal force-quits)
(`Failed Writes` > 0: bulk item errors such as 429 storms, flood-stage or write blocks, mapping errors,
conflicts). Like Node, the run continues past failed items by default, but unlike Node it does not exit 0
then; pass `--ignore-es-write-errors` (or `--ignore-errors`) explicitly to accept failed writes with status 0,
or `--ignore-es-write-errors false` to stop at the first one with status 1. `--debug` masks query values
except a small allow-list (`scroll`, `size`, ...).

Metadata types: `index`, `analyzer`, `alias`, `template`, `component_template`, `index_template`, `policy`
and `script` (see [Types](#types)). Index renaming: `--renamePrefix`, `--renameIndex`, `--renameAliases`,
`--renamePatterns` (see [Renaming indices](#renaming-indices)).

Out of scope (rejected or ignored with a warning, see `docs/PARITY.md` §3.7): CSV, S3/MinIO, AWS signing,
transforms, `multielasticdump`, `--delete`.

Where this tool differs from Node because Node is incomplete or buggy, see [NODE-DIFFERENCES.md](NODE-DIFFERENCES.md);
for every place the tool changes what is loaded, and for the tested versions, see
[TESTING-MATRIX.md](TESTING-MATRIX.md). Security scanning: [SECURITY-CVE.md](SECURITY-CVE.md).

## Intentional differences from Node elasticdump

* **zstd**: `--compressionType zstd` writes `.zst` files/parts that Node cannot read. Default output stays uncompressed;
  `--fsCompress` keeps meaning gzip, and gzip stays the default codec (D-2); zstd is used only when asked for.
* **Compressed input is auto-detected** (gzip or zstd magic bytes) with or without `--fsCompress`. Decoding streams
  (memory stays bounded by `--maxDocMb`, also for 1000:1 and 30000:1 bombs); zstd frames that declare a window over
  128 MiB (`zstd --long=28` or more) are refused with a hint. Truncated input, trailing garbage and corrupt frames stop
  the run with exit 1 and nothing committed.
* **`--fileSize` counts UTF-8 bytes**; Node counts UTF-16 code units. Identical part names for ASCII data
  (verified against Node), slightly different boundaries for non-ASCII text.
* **Object key order is kept as the server sent it.** Node re-serialises parsed JSON, and JavaScript lists
  integer-like keys (`"0"`, `"15"`) first in numeric order; metadata lines with such keys are therefore
  equal in content but not byte-equal (found by the L-502 Node comparison; no other byte difference).
* **Split parts are staged and committed together** at the end of a successful run; a failed run (including a full disk) leaves nothing and an existing destination untouched. With `--overwrite` the files being replaced are moved aside first and restored if any part cannot be committed; older parts beyond the new set are left in place with a warning.
  Existing `*.split-*` files need `--overwrite`.
* **Certificate flags always apply**; Node ignores `--ca/--cert/--key` unless `--tlsAuth` is set.
* **Retries also cover HTTP 429**, and bulk items rejected with 429/503 are retried individually.
* **PIT is engine-aware**: Elasticsearch 7.10+ uses `POST /{index}/_pit` and the `_shard_doc` tiebreaker;
  OpenSearch uses `/_search/point_in_time` and `_doc` + `_id` (a `_doc`-only sort was verified to lose documents).
* **Scroll + `--offset`** skips client-side; Node sends `from=`, which ES 7+/OpenSearch reject.
* **`--scrollId` ids are never printed** (Node's `--debug` shows them, which its resume recipe relies on); resume
  a stopped file dump by rerunning it with `--overwrite`, or pass the id of a scroll you opened yourself (L-650).
* **`--throttleInterval`** only applies when given (Node's 1 ms default would add ~10 s per million docs).
* **`--intervalCap`/`--concurrencyInterval`/`--carryoverConcurrencyCount`** work (p-queue semantics) when set;
  they have been no-ops in Node since 6.119.
* **`--format human`** writes pretty-printed JSON (restorable), not Node's `util.inspect` output; it cannot be
  combined with splitting.
* **`--searchWithTemplate`** renders `{"id"|"source", "params"}` via `_render/template`, then searches normally.
* **Big integers and unicode** are preserved byte-for-byte in both directions (no `--support-big-int` needed).
* **Restored documents are sent as written in the file.** Node re-serialises every `_source` and `_id` (escape forms
  such as `\u00e9` become `é`, `1.0` becomes `1`, `1e21` becomes `1e+21`, integer-like keys move first, integers
  beyond 2^53 are rounded or turned into strings); the Rust tool passes the JSON text through unchanged. A numeric `_id`/`routing`/`parent` in the file becomes a string of exactly its text (`-0` → `"-0"`, `1e3` → `"1e3"`, `0.0` → `"0.0"`), where Node sends `"0"`, `"1000"`, `"0"`; a literal that is not a JSON number (`00`, `+1`, `NaN`) rejects that document (L-629a). Node also sends
  each bulk batch in reverse file order, so when one batch holds two documents with the same `_id` the earlier line
  wins in Node and the later line wins here (file order). Verified on 10,000 random documents by
  `scripts/node/bulk-diff.py` (L-506): every other byte difference is one of these.
* **Bulk uses POST**; `/index/type` URLs are not parsed (use `--output-index idx/type` for ES < 7).
* **Endpoints that look like mistyped URLs are refused** (exit 2, L-646); Node reads or writes them as files.
  Only `http://` and `https://` are clusters, in any case (`HTTP://` is a cluster here, a file in Node); everything else is a file path, but
  `http:/host`, `http:host`, `http//host`, an unsupported scheme such as `htp://`, and a URL with leading or trailing
  whitespace are usage errors, and a scheme-less `--output user:password@host:9200` is refused because it would write
  the password into a file name. A scheme-less `host:port` or `user:pw@host` that is used as a file prints a warning
  (password masked). Start the path with `./` when such a name really is a file. No message quotes a password.
* **`--username`/`--password` apply to the input only** (they are aliases of `--input-username`/`--input-password`;
  use `--output-username`/`--output-password` for the output).
* **`--httpAuthFile` values are taken literally:** `#`, `;` and `\` inside a value are kept (Node's INI parser cuts
  the password at `#`/`;`); surrounding spaces and quotes are removed as in Node.
* **Metadata types** fix several Node bugs (listed in [NODE-DIFFERENCES.md](NODE-DIFFERENCES.md)); intentional
  choices: `--filterManaged` (default on), `--output-index`/`--prefix`/`--suffix` apply to alias targets,
  cluster-level types ignore the output index (warning), the source is read before the target is contacted,
  `--closeIndex` is opt-in, analyzer restores do not wait for green.
* **PIT falls back to scroll** on clusters without it (ES OSS builds, OpenSearch < 2.4) and sliced PIT falls back
  to sliced scroll on ES < 7.15, with a warning; Node fails.
* **Cluster→cluster settings** drop engine-specific keys (ES `index.lifecycle` and
  `index.routing.allocation.include._tier_preference` going to OpenSearch; OpenSearch `index.replication.type`,
  `plugins.*`, `opendistro.*` going to ES) with a warning. File restores keep them (a file does not say which
  engine wrote it) but warn with the exact `--excludeSettings=` to use; Elasticsearch rejects OpenSearch keys
  outright (the error carries the same hint), while OpenSearch would otherwise store ES keys silently.

## Types

| `--type` | Dump (cluster → file) | Restore (file/cluster → cluster) |
|---|---|---|
| `data`, `mapping`, `settings` | as before | as before |
| `index` | `GET /{index}` (host-only: `GET /*`): settings + mappings + aliases, generated settings stripped | one `PUT /{index}` per index (`--skip-existing` skips existing ones); `data_stream` indices are skipped with a warning |
| `analyzer` | `{index}/_settings` (the analysis block is what matters) | new index: created with the analysis; existing index: error, `--skip-existing`, or `--closeIndex` (close → update → always reopen; the index is **offline** meanwhile; Ctrl+C / SIGTERM while it is closed abandons the update, reopens it and exits 130 / 143; a second Ctrl+C before the reopen finished force-quits and names the index that may still be closed, with the `POST /{index}/_open` to run) |
| `alias` | `GET {index\|*}/_alias/{alias\|*}` (`--input-index idx/alias*` filters) | one atomic `POST /_aliases`; targets follow `--output-index`/`--prefix`/`--suffix`/rename rules |
| `template` | legacy `_template` | `PUT _template/{name}` (deprecated on ES ≥ 8, warned once) |
| `component_template` | `_component_template` | `PUT _component_template/{name}` |
| `index_template` | `_index_template` | `PUT _index_template/{name}`; missing `composed_of` components fail with "restore component templates first" |
| `policy` | ES: `_ilm/policy`; OpenSearch: `_plugins/_ism/policies` (paged) | same engine only, unless `--convertIlmToIsm` (ES → OpenSearch); OpenSearch overwrites use `if_seq_no`/`if_primary_term` |
| `script` | stored scripts from `_cluster/state/metadata` (needs the `monitor` cluster privilege) | `POST _scripts/{id}` |

Files keep elasticdump's format (one JSON string literal per file, mappings as an object), so Node and this tool
read each other's dumps. A file dumped with one `--type` and restored with another fails with
"this input looks like a --type X dump".

**Full backup and restore order** (each step its own command, all with the same index pattern):
`component_template` → `index_template` → `template` → `policy` → `script` → `index` (or `settings` + `mapping`)
→ `alias` → `data`. Templates and policies first so new indices pick them up; aliases after the indices exist.
The `full_backup_and_restore_in_documented_order` integration test runs exactly this order on every profile.

**Filters.** `--templateRegex` (default elasticdump's `^(metrics|logs|.+_audit_log|.+-index-template|\..+)$`,
cluster dumps only, `--filterSystemTemplates=false` disables it) and `--filterManaged` (default on; drops
templates and ILM policies with `_meta.managed: true`, for cluster and file input). Elasticsearch 8/9 ship dozens of
managed built-ins (`*@template`, `*@mappings`, `*@settings`, `logs`, `metrics`, `synthetics`, APM/OTel templates,
`7-days-default` … ILM policies) that the Node regex misses; ES 7.17 marks fewer of them (`ilm-history`,
`synthetics`, the `logs-*`/`metrics-*` component templates and the default ILM policies).

**Cross-engine templates.** Cluster → cluster strips the other engine's fields with a warning
(ES: `allow_auto_create`, `ignore_missing_component_templates`, `deprecated`, `data_stream_options`,
`template.lifecycle`, `data_stream.{hidden,allow_custom_routing,failure_store}`; OpenSearch:
`data_stream.timestamp_field`, `context`) and the settings keys the target rejects. From a file they are only
warned about, with the exact `--excludeTemplateFields=` to use.

### ILM → ISM conversion (`--convertIlmToIsm`)

Off by default: restoring or copying ILM policies into OpenSearch fails with an error naming the flag, and
ISM → ILM is always refused. With the flag, each ILM policy becomes one ISM policy with one state per phase
(`hot` → `warm` → `cold` → `delete`, in that order; a non-zero first `min_age` adds a `pending` state). Every
dropped or approximated feature is logged as a warning. Unsafe features fail the policy unless
`--lenientPolicyConversion` is given, which drops them with a `DROPPED …` warning.

| ILM | ISM | Notes |
|---|---|---|
| phase `min_age` | transition `min_index_age` (after a rollover: `min_rollover_age`) | ILM counts from rollover, ISM from index creation unless `min_rollover_age` |
| `set_priority` | `index_priority` | `priority: null` dropped |
| `rollover` `max_age`/`max_docs`/`max_size`/`max_primary_shard_size` | `min_index_age`/`min_doc_count`/`min_size`/`min_primary_shard_size` | ISM needs `plugins.index_state_management.rollover_alias` on the index (warned) |
| `allocate` | `replica_count` + `allocation` (`require`/`include`/`exclude`, `wait_for: false`) | |
| `forcemerge` | `force_merge` | |
| `shrink` | `shrink` (`num_new_shards` / `max_shard_size`) | OpenSearch 1.x has no ISM `shrink`: refused there unless `--lenientPolicyConversion` |
| `readonly` | `read_only` | |
| `delete` | `delete` | `delete_searchable_snapshot` dropped (warned) |
| `migrate`, implicit tier migration | — | dropped with a warning (OpenSearch has no data tiers) |
| `_meta.description` (a string) | `description` | other `_meta` keys (nested ones too), a non-string `description` and `deprecated` dropped (warned) |
| `frozen` phase, `searchable_snapshot`, `downsample`, `unfollow`, `freeze`, `total_shards_per_node`, `index_codec`, `allow_write_after_shrink: true`, `max_primary_shard_docs`, `min_*` rollover conditions, a `rollover` / `forcemerge` / `shrink` without its size, unknown phases, actions, action options or phase keys | — | **refused** unless `--lenientPolicyConversion` |
| `wait_for_snapshot` (any phase) | — | **refused even with `--lenientPolicyConversion`**, naming the policy and phase: ISM has no equivalent, and dropping it would delete the indices without waiting for the snapshot (L-651a) |
| a `min_age` that is not a whole-number duration (`7`, `"soon"`, `null`, `1.5h`, which OpenSearch rejects) | — | **refused even with `--lenientPolicyConversion`**: read as 0 it would start the next phase (e.g. `delete`) at once (L-651) |

Converted policies are not attached to indices (ILM attaches through `index.lifecycle.name`, ISM through
`ism_template` or `POST _plugins/_ism/add/<index>`); the tool says so after converting. Tested on OpenSearch 1.3,
2.x and 3.x, from ES 7.17, 8.6, 8.19 and 9.1 exports and cluster to cluster. A policy with no convertible phase
(empty `phases`, `frozen` only) is refused even with `--lenientPolicyConversion`. Property oracle (L-507): 150 random
ILM policies × strict/lenient on OpenSearch 1.3.20, 2.19.7 and 3.9.0; every converted policy was accepted.

A set of policies is all or nothing at conversion: every policy is converted and checked before the first one is
written, so one refused policy writes none of them (`--ignore-errors` skips it and writes the rest). If the cluster
itself then rejects a policy partway through the set, the run is rolled back before it exits (L-651a): policies this
run created are deleted, policies it overwrote get their previous body back (newest first; nothing it did not write is
touched, and policies after the rejected one are never sent). The error names the rejected policy, what was rolled back
and whether the rollback succeeded; if a delete or restore fails, it names exactly which policies could not be put back
and how they were left. `--ignore-errors` skips the rejected policy and keeps the rest (no rollback). Live-tested on
OpenSearch 1.3.20, 2.19.7 and 3.9.0 with an unknown ISM action injected third of four. The first policy writes on a fresh OpenSearch
cluster can race another client creating the ISM config index; that rejection is retried, not read as an existing policy. L-651: a proptest over every phase × every ILM action (and unknown ones) × convertible and unconvertible
options (4,096 cases per run, 300,000 once) checks against an independent model that exactly the unsupported parts
are refused (strict) or dropped one `DROPPED` line each (lenient) and that the result is a well-formed ISM policy;
live, OpenSearch 2.x and 1.3.20 accept every one of the 322 converted policies of the exhaustive set.

Odd but valid policies (L-652): phases in any JSON order become states in lifecycle order with forward-only
transitions; a missing `min_age` means no delay; a `min_age` or rollover `max_age` keeps its value and unit as
written (`0ms`, `30s`, `90m`, `12h`, `1d`, `1000d`, `500micros`, `7nanos`; a zero age in any unit gives an
unconditional transition); a delete-only policy, a rollover-only hot phase, nested `_meta` and unicode policy names
(`politique-é-日本`) convert. Live, OpenSearch 1.3.20, 2.19.7 and 3.9.0 accept all 42 such policies and return the
states, transitions and units unchanged.

Elasticsearch built-in policies (L-653): with `--filterManaged false`, every built-in ILM policy of ES 7.10.2,
7.17.29, 8.6.1, 8.19.23 and 9.1.2 (6 to 46 per version: `logs`, `metrics`, `synthetics`, `*-days-default`,
`*@lifecycle`, `ilm-history-ilm-policy`, `slm-history-ilm-policy`, `watch-history-ilm-policy*`, APM, Fleet,
`.deprecation-indexing-ilm-policy`, `.monitoring-8-ilm-policy`) converts strictly, with nothing dropped, ids with a
leading `.` or an `@` kept as they are. Each one marked `_meta.managed: true` is warned as "Elasticsearch-managed
(built-in) policy: on OpenSearch it is an ordinary policy that nothing manages or updates". With the default
`--filterManaged` those are skipped and named instead (an info line, shown with `--debug`; only the lower
`Total Writes` shows it otherwise); ES 7.10 marks none of its built-ins managed, and
`.monitoring-8-ilm-policy` is not marked on 8.x/9.x, so those are always restored. OpenSearch 1.x has no ISM
`shrink`, so there the `*-days-default` / `*-days@lifecycle` built-ins (warm-phase shrink; 5 on 7.17/8.6, 10 on
8.19/9.1) are refused before any write, naming policy, phase and action, and `--lenientPolicyConversion` drops only
the shrink. Live, OpenSearch 1.3.20, 2.19.7 and 3.9.0 accept the 7.17.29, 8.19.23 and 9.1.2 sets. ISM → ILM (OpenSearch policies to Elasticsearch) is refused
before any write, with or without `--convertIlmToIsm`, `--lenientPolicyConversion` or `--ignore-errors`; the
error names the refused policies (the first 10, then "and N more") and says nothing was written.

OpenSearch reporting Elasticsearch 7.10.2 (L-654): with `compatibility.override_main_response_version: true`
(dynamic on OpenSearch 1.x and 2.x, rejected by 3.x), `GET /` reports `7.10.2` with no `version.distribution` and no
`build_flavor` (the tagline stays OpenSearch's). That `GET /`, or a `7.10.2` with neither field that a proxy
rewrote, is followed by `GET _nodes?filter_path=nodes.*.version,nodes.*.build_flavor`: OpenSearch nodes give the
real version (the oldest node counts) and the run continues as that OpenSearch with a warning; nodes with a
`build_flavor` are Elasticsearch. If `_nodes` does not answer, the OpenSearch tagline still means the OpenSearch 1.x
feature set (warned); with no tagline either the run fails before any dump request, naming the setting, instead of
guessing Elasticsearch. Live on OpenSearch 1.3.20 and 2.19.7 with the setting on, a `--searchType pit` dump detects the real
version and never uses the Elasticsearch PIT (1.3.20 falls back to scroll; 2.19.7 uses the OpenSearch PIT).

Field types without an equivalent (L-655): restoring a mapping or index (`--type mapping`/`index`) checks every
field (`properties`, nested objects, multi-fields, `runtime` and `dynamic_templates`) against the target's engine
and version before any write. Types the target has no handler for, for example `flattened`, `dense_vector`,
`semantic_text` or `histogram` on OpenSearch and `flat_object`, `knn_vector`, `semantic` or `xy_shape` on
Elasticsearch, X-Pack types on the Elasticsearch OSS build, or a type newer than the target, are refused with one
error that lists each field as `path` (type, why, closest type) and says nothing was written; the mapping is not
converted. With `--ignore-errors` only the indices with such fields are skipped (counted as failed). A
`sparse_vector` is told apart by its definition (OpenSearch's has `method`). A cluster-to-cluster data copy between
engines whose target index does not exist is refused the same way before any read or write, because the target
would create it with dynamic mappings and guess other types; create the index from a converted mapping first.
That copy asks the source for its engine (`GET /`) before searching; a source that does not answer skips the check.
Plugin and unknown types are left to the server. Not checked: data restored from a file (its types are unknown),
an existing target index, and index templates. The API-071 expectation table is unchanged (its fixture has none of
these types). Live, the refusals match what the server rejects on OpenSearch 1.3.20, 2.19.7, 3.9.0 and
Elasticsearch 7.10.2 (OSS and default), 7.17.29, 8.6.1, 8.19.23 and 9.1.2.

Features the target does not have (L-656): restoring templates (`--type template`/`index_template`/
`component_template`), settings or indices checks them against the target before any write. A composable template
with `data_stream` on the Elasticsearch OSS build (data streams are X-Pack) or before 7.9, and settings the target
rejects as unknown (`index.lifecycle.*` on OpenSearch, which attaches ISM policies with
`index.plugins.index_state_management.policy_id`; `index.lifecycle.*`, `index.frozen` and `index.xpack.*` on the
OSS build) are refused with one error naming each item, the feature, the reason and the exclusion that drops it
(`--excludeTemplateFields=data_stream`, `--excludeSettings=...`); nothing is written. With `--ignore-errors` only
those items are skipped (counted as failed). Templates are all prepared before the first PUT. ILM policies are
refused on the OSS build and on OpenSearch (use `--convertIlmToIsm`), `--searchType pit` falls back to scroll with a
warning where there is no PIT, and credentials sent to the OSS build (which has no security) give a warning, since
it does not check them. Writing to a data stream with the default `--bulkAction index` fails per document; the
first such failure prints a hint to use `--bulkAction create`. Checked live on Elasticsearch 7.10.2 (OSS and
default), 7.17.29, 8.6.1, 8.19.23 and 9.1.2, and OpenSearch 1.3.20, 2.19.7 and 3.9.0.

## Renaming indices

`--renamePrefix old:new` (repeatable) and `--renameIndex '<regex>:<replacement>'` (repeatable, Rust `regex`
syntax, `$1`/`${name}` captures; write `${1}abc` when letters follow) rename every index a run writes, for
`data`, `mapping`, `settings`, `index`, `analyzer` and `alias` (the indices the aliases point at). The first
matching rule wins (`--renamePrefix` rules are tried first); unmatched names stay as they are.

* Order: rename first, then `--prefix`/`--suffix`.
* With an explicit single `--output-index` (or an index in the output URL) rename rules are a usage error, as is a
  file output.
* `--renameAliases` also renames alias names (alias, index and template types) with the same rules.
* `--renamePatterns` also renames template `index_patterns` (and legacy `template`) and ISM `ism_template`
  patterns; without it they are left alone.
* Each rename is logged once (`Renaming index 'a' -> 'b' (rule)`); invalid results (uppercase, illegal
  characters, leading `-_+`, `.`/`..`, > 255 bytes) fail before anything is written.

```bash
# dump every logstash-* index, restore it as logstash-srctag-*
elasticdump-rs --input http://es:9200/logstash-* --output idx.json --type index
elasticdump-rs --input http://es:9200/logstash-* --output data.json
elasticdump-rs --input idx.json  --output http://os:9200 --type index --renamePrefix logstash-:logstash-srctag- --renameAliases
elasticdump-rs --input data.json --output http://os:9200 --renamePrefix logstash-:logstash-srctag-
```

## Container image

`Dockerfile` is a multi-stage build: a Rust builder (release profile, `CARGO_BUILD_JOBS=2` by default
because the fat-LTO link peaks at ~1.5-2 GB) and one of two runtime stages. Builder and runtime always use the
same libc. Every build uses the same Dockerfile through the `BUILDER_IMAGE`/`BUILDER_TAG`/`BASE_IMAGE`/`BASE_TAG`
build args and a `--target` (bases are pinned by multi-arch index digest in the Makefile):

| Make target | Tag | Builder | Runtime (`--target`) | libc / allocator | Size | HIGH/CRITICAL CVEs (2026-11-01) |
|---|---|---|---|---|---|---|
| `make image` (**default**) | `:<ver>`, `:latest` | `dhi.io/rust:1.99.1-debian13-dev` | `dhi.io/static:20250419-glibc-debian13` (`runtime`, COPY-only, no shell) | glibc 2.41 / mimalloc | 24 MB | grype 5 High (glibc, libgcc-s; won't-fix / not-fixed upstream), trivy 0 |
| `make image-dhi-alpine` | `:<ver>-alpine` | `dhi.io/rust:1.99.0-alpine3.23-dev` | `dhi.io/alpine-base:3.24-alpine3.24-dev` + apk `bash ca-certificates libgcc` (`runtime-shell`) | musl / musl | 21 MB | **0** (0 at any severity) |
| `make image-dhi-debian` | `:<ver>-debian-dev` | `dhi.io/rust:1.99.1-debian13-dev` | `dhi.io/debian-base:trixie-debian13-dev` (`runtime-shell`) | glibc 2.41 / mimalloc | 90 MB | 15 (grype) / 9 (trivy) distinct CVEs, none fixable (2026-10-29) |
| `make image-alpine` | `:<ver>-alpine-public` | `docker.io/library/rust:1.99.1-alpine3.24` | `docker.io/library/alpine:3.24.3` + apk `bash ca-certificates libgcc` (`runtime-shell`) | musl / musl | 20 MB | 0 (2026-10-29) |
| `make image-public` | `:<ver>-debian-public` | `docker.io/library/rust:1.99-trixie` | `docker.io/library/debian:trixie-slim` (`runtime-shell`) | glibc 2.41 / mimalloc | 90 MB | 54 High matches (grype) / 43 (trivy), none fixable (2026-10-29) |

The default is the glibc + mimalloc build on the dhi.io `static` glibc base (L-808b). Of the dhi.io glibc bases it is
the smallest (16 MB: glibc, libgcc_s, the CA bundle, tzdata and a `nonroot` 65532 user; no shell, no package manager),
has the fewest CVEs (grype 5 High / 24 distinct against 14 / 46 for `debian-base:trixie-debian13` and 46 / 71 for the
`-dev` base) and is published for amd64 and arm64; the comparison is in `results/local/L-808b.md` and
[SECURITY-CVE.md](SECURITY-CVE.md). Without a shell the default image cannot run wrapper scripts inside the container;
use the Alpine variant (`:<ver>-alpine`, bash, musl's allocator, 0 CVEs) for that. On Alpine runtimes the Dockerfile
runs `apk upgrade`, then adds `bash`, `ca-certificates` and `libgcc`. Alpine-packaged rustc links `libgcc_s`
dynamically.

**Image name:** set the full repository and tag of the built image with `scripts/build-image.sh --imagename registry.example.com/team/elasticdump-rs:1.2.3`, or with the `ELASTICDUMP_IMAGE` environment variable, which also works for every `make image*` target (`ELASTICDUMP_IMAGE=registry.example.com/team/elasticdump-rs:1.2.3 make image-dhi-debian`). It replaces the default tags; the script adds `:<version>` to a bare repository name.

**Prerequisites:** podman (or docker with the buildx plugin). No Rust is needed on the host: crates are vendored inside a container from the pinned builder image (`VENDOR_ON_HOST=1` uses host cargo >= 1.85 instead). `scripts/build-image.sh` checks these and the dhi.io login up front and stops with a clear message. `dhi.io` (Docker Hardened Images) needs a pull login: `podman login dhi.io` or `docker login dhi.io` with a Docker
account. Anonymous pulls return `authentication required`. The login stays in the container tool's auth file and
never enters a build layer.

**Offline build (L-808i).** Crates are fetched first and the image build runs offline: `scripts/build-image.sh` and
every `make image*` target first run `scripts/vendor-crates.sh`, which runs `cargo vendor --locked` inside a throwaway
container from the pinned builder image (repo mounted at `/src`, files stay owned by you) into the gitignored `.vendor/`
(about 270 MB, refreshed on each build; `rm -rf .vendor` removes it). The builder copies it in and runs
`cargo build --locked --offline` from the vendored sources, in a `RUN --network=none` step, so the build needs no DNS or
crates.io access. The host needs no Rust (`VENDOR_ON_HOST=1` uses host cargo instead). Behind a proxy, `HTTPS_PROXY`,
`HTTP_PROXY`, `NO_PROXY` and `CARGO_HTTP_PROXY` are passed into the vendoring container; the container tool still
pulls the base images, and the Alpine stages still run `apk`, with its normal network.

Every image contains `/usr/local/bin/elasticdump-rs` plus an `elasticdump` symlink, runs as uid
65532 (non-root), has a CA bundle (`SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt`), uses
`/data` as its writable working directory, and carries OCI labels (version, revision, created,
base name).

### `scripts/build-image.sh`

Builds the two main variants locally (nothing is pushed unless `--push` is given):

```bash
scripts/build-image.sh                              # glibc + mimalloc on dhi.io/static: elasticdump-rs:<ver>, :latest
scripts/build-image.sh --variant alpine             # Alpine/musl, musl allocator: elasticdump-rs:<ver>-alpine
scripts/build-image.sh --platform both              # linux/amd64 + linux/arm64 manifest list (podman)
scripts/build-image.sh --platform linux/arm64 --tag registry.example.com/team/elasticdump-rs:0.1.0-arm64
scripts/build-image.sh --tag registry.example.com/team/elasticdump-rs:0.1.0 --push
scripts/build-image.sh --dry-run --variant alpine   # print the podman commands only
scripts/build-image.sh --help
```

Options: `--variant glibc|alpine` (default `glibc`), `--platform linux/amd64|linux/arm64|both` (default: the host's),
`--tag REF` (repeatable; replaces the default tags), `--name REPO` (default `localhost/elasticdump-rs`), `--jobs N`
(`CARGO_BUILD_JOBS`, default 2), `--push`, `--dry-run`. The base pins are read from the Makefile. A foreign platform
runs the Rust build under emulation, so the host needs qemu-user-static (binfmt_misc) and it is slow; `both` builds a
podman manifest list. `--push` uses only the container tool's existing registry login (`podman login <registry>`);
the script never reads, takes or stores credentials. `CONTAINER=docker` works for single-platform builds.

```bash
make image                 # default: dhi.io static glibc + mimalloc (after `podman login dhi.io`)
make image-dhi-alpine      # dhi.io Alpine/musl (after the login)
make image-dhi-debian      # dhi.io Debian 13 -dev runtime with a shell (after the login)
make image-public          # public Debian 13 bases, no login
make image-alpine          # public Alpine/musl bases, no login
make test-image            # --version, --help, non-root, symlink, CA bundle, --read-only, executables, base digest
make test-image IMAGE_VARIANT=dhi-alpine   # the same for :<ver>-alpine (variants: dhi-static, dhi-alpine,
                                           # dhi-debian, public, alpine)
make sbom scan             # syft SBOM (CycloneDX + SPDX), grype + trivy (fail on HIGH/CRITICAL) of IMAGE_VARIANT
make image REGISTRY=registry.example.com/team IMAGE_NAME=elasticdump-rs IMAGE_TAG=0.1.0

# podman
podman run --rm -v "$PWD/dumps:/data:Z" localhost/elasticdump-rs:0.1.0 \
  --input=http://es:9200/my_index --output=/data/my_index.json --type=data
# docker (same image, Node-style entrypoint name)
docker run --rm -v "$PWD/dumps:/data" --entrypoint elasticdump localhost/elasticdump-rs:0.1.0 \
  --input=/data/my_index.json --output=http://es:9200/my_index --type=data
```

Nothing in the Makefile pushes; tag and push yourself if you want the image in a registry.
Mounted host directories must be writable by uid 65532 (or use `--user`).

## Reliability

* Partial search results (`timed_out`, failed shards) are errors, not silently written.
* Retries: transport errors and 5xx/429 are retried at `--retryDelay`; scroll continuations only on 429
  (re-sending one after a network error could skip a page). Bulk writes with explicit ids are idempotent;
  a 20-run fault-injection suite (connections cut mid-response) restores exact copies.
* File output is staged, fsynced and committed atomically (hard link without `--overwrite`, so a file created
  meanwhile is never clobbered). `--output` must be a regular file path: a directory, a symbolic link (the link and
  its target are both left alone; pass the target path to write there), a FIFO or device (`/dev/stdout`: use
  `--output $`) or a missing parent directory is a usage error (exit 2) before anything is read. With
  `--overwrite`, the replaced file's mode is kept.
  Symlinked directories in the parent path are followed (`linkdir/out.json` with `linkdir -> /real/dir` writes
  `/real/dir/out.json`); only the final file itself may not be a link. A dangling parent link is a usage error
  that names the link and its missing target (L-644a).
* A run that is killed outright (`kill -9`, the OOM killer, a power cut) cannot clean up, but it never leaves a
  partial file at the output path: the output (each split part) is absent, the old file, or complete. A split set
  killed mid-commit can be torn, but the parts not yet moved into place are still staged whole
  (`.d.split-N.json.part-PID-TS`) and files being replaced with `--overwrite` were moved aside whole
  (`.d.split-N.json.edrs-replaced-PID`). These hidden files are **left in place, never deleted**: the next run to
  the same output prints a warning naming each one with its size (D-5); check and remove them yourself.
  `--overwrite` replaces only the output itself (and its split parts), never these files; with `--overwrite` the
  warning says so (L-805a). A killed
  restore never changes its input file; the cluster got the first documents of the file, without gaps (with the
  default `--concurrency`). Checked by `kill -9` at random points, 50 runs each for dump, split and restore (L-648).
* A second Ctrl+C (or SIGTERM) while the first one is still being cleaned up force-quits at once (130 / 143). It
  never commits output: an unfinished `--output` stays uncommitted, the exit message says so, and its hidden staging
  files are named by the next run. Anything else it could not clean up (an index `--closeIndex` closed) is named
  too (L-649).
* Ctrl+C clears the scroll / closes the PIT (ES input pipelines).
* Ctrl+C (or SIGTERM) during a restore into a cluster (file -> cluster or cluster -> cluster) stops reading,
  lets the bulk requests already sent finish (none is cut off), prints `Total Writes` and exits 130 / 143 with
  the `--offset` to resume from, e.g. `rerun the same command with --offset 1230` (plus `--size N` when the run
  had a `--size`). Every input document before that offset was sent (failed ones are counted in `Failed Writes`
  and not retried). The offset is exact for a file input; for a cluster input only while the source index is
  unchanged and its search order is stable (a sort in `--searchBody`). A sliced dump (`--slices` > 1) has no
  single order: it says so and is simply rerun. File output still removes its staged files (L-647).
* Resuming a stopped run (L-650, checked end to end with random INT / TERM / `kill -9` and digest-equal results):
  * **Restore into a cluster:** rerun with the printed `--offset` (and `--size`). Chained over several stops it
    is exact, with any `--concurrency` (bulk requests finishing out of order included): every document is
    written once, none is skipped, and the result equals an uninterrupted run (by `_id`, so a rerun after
    `kill -9` overwrites rather than duplicates; with `--offset` picked by hand after `kill -9`, count from what
    the cluster has).
  * **Dump to a file (or split set):** rerun the same command with `--overwrite`. Without it the rerun refuses
    (`already exists, quitting`). A stopped run leaves the earlier output and no staging files; a killed one
    may leave hidden files, which the rerun names. A Ctrl+C during the setup, before any document is read, now
    also removes the staged output (before L-650 it force-quit and left the `.part` file behind); the same holds
    for a metadata dump (`--type mapping`, `settings`, ...) to a file (L-650a). A signal that comes after the
    commit, while the process is still exiting, still exits 130 / 143, with the complete new output.
  * **`--scrollId`** continues the scroll *from where the cluster's cursor is*: pages already fetched, by an
    earlier request or a killed run, are not returned again. It is meant for a scroll you opened yourself (keep
    the first page from your own `_search?scroll=` request, then pass its `_scroll_id`); this tool never prints
    scroll ids (redaction). A scroll cleared by Ctrl+C, expired after `--scrollTime`, or lost with its node
    cannot be resumed: the run exits 1 with `--scrollId: the cluster no longer has this scroll ...`, commits
    nothing, and says to rerun without `--scrollId`.

## Security notes

* Passwords never appear in logs, `--debug` output, error messages or panic reports, in any form (as given,
  percent-encoded, JSON-escaped, or inside the `Authorization: Basic` token), even when a server echoes the request
  back. Prefer `--httpAuthFile` or the environment variables so they also stay out of shell history and `ps`.
  Values shorter than 4 characters are only masked inside the Basic token (masking them everywhere would mangle
  messages).

### Credentials

Each direction resolves its basic-auth username and password on its own (L-645). Highest first:

1. `--input-username`/`--input-password` (aliases `--username`/`--password`) and
   `--output-username`/`--output-password`; each flag overrides one field and they never apply to the other
   direction.
2. `user:password@` in the `--input`/`--output` URL (percent-decoded).
3. `--httpAuthFile` (INI with `user=` and `password=`, used for both directions), otherwise
   `ELASTICDUMP_{INPUT,OUTPUT}_{USERNAME,PASSWORD}` (when its `_USERNAME` is set). These are used only when 1 and 2
   together do not give both a username and a password; like Node, the auth file's pair replaces a URL that has only
   a username. A flag still wins over them.

* A trailing CR/LF is removed from every value (an auth file saved with CRLF line ends, `$(cat file)`), so it never
  turns into a confusing 401. An empty password is sent as is (`user:`).
* In a URL, `#`, `?`, `/` and `@` inside the password must be percent-encoded (`%23`, `%3F`, `%2F`, `%40`); `:`,
  spaces and unicode work as they are. An unencoded `#`/`?`/`/` is a usage error (exit 2) that says so without
  quoting the password, instead of a dump to the wrong host; or pass the password with `--input-password` /
  `--output-password`, where any character works.
* A `--httpAuthFile` readable by every user on the machine is used, with a warning to `chmod 600` it.
* `--insecure` / `NODE_TLS_REJECT_UNAUTHORIZED=0` disable certificate verification and print a warning.
* Encrypted client keys: PKCS#8 only (convert legacy keys with `openssl pkcs8 -topk8`).

## Memory

Peak RSS ≈ `(bufferSize + slices + concurrency) × limit × avg_doc_size × ~3 + ~15 MB` (**changed in L-802, from
× 1.5 + 10 MB, pending lead review**: the old estimate was exceeded by up to 127% by concurrent restores and sliced
copies at 1M documents; `results/local/L-802.json`). Measured with glibc's malloc before L-808b (Elasticsearch 7.17, ~1 KB docs): 24 MB dumping at
`--limit 5000`, 30–80 MB sliced (`--slices 2`–`16`), 180–330 MB restoring at `--limit 5000 --concurrency 4`–`8`,
85–160 MB copying. Peak RSS grows slowly with the number of documents in concurrent restores and copies (restore
`--concurrency 8 --limit 5000`: 196 MB at 100k docs, 268 MB at 1M, 346 MB at 10M with `MALLOC_ARENA_MAX=2`), and
stops growing on long runs when glibc is limited to two malloc arenas (`MALLOC_ARENA_MAX=2`). With glibc's default
arenas the same 10M-doc restore reached 537 MB and was still growing slowly, so set `MALLOC_ARENA_MAX=2` for long
concurrent restores in tight memory (builds with glibc's malloc, `--features system-allocator`; restores and copies ran at the same speed within 10%, sliced
dumps within this box's noise; musl/Alpine builds were not measured). File-to-file runs stay at 14–40 MB for ~1 KB
docs at any size up to 10M docs.
Allocators (L-808b): glibc Linux builds (the release `*-linux-gnu` binaries and the default container image) use
mimalloc; musl builds (the `*-linux-musl` binaries and the `:<ver>-alpine` image) use musl's own allocator. The
glibc-arena figures above (`MALLOC_ARENA_MAX=2`) and the `MALLOC_*` tuning below apply only to builds with glibc's malloc
(`cargo build --features system-allocator`); mimalloc and musl ignore those variables. Measured with mimalloc (L-802a,
`results/local/L-802a.md`, confirmed for the default build in `results/local/L-808b.md`; ~1 KB docs,
`--concurrency 8 --limit 5000`): peak RSS 312 / 335 / 364 MB restoring at 100k / 1M / 10M docs, 278 / 388 / 419 MB
copying, 68 / 82 / 131 MB dumping and 176 / 148 / 212 MB for `--slices 4`, at the same speed as glibc's malloc. That is
below glibc's default arenas on long concurrent restores and copies (520 / 684 MB at 10M) and above them on dumps;
the × 3 estimate above still covers every mimalloc run except the 10M copy (≈ 7% over, inside the 25% tolerance of
`scripts/load/memcheck.py`).
musl builds use the least memory (154 / 222 / 18 / 34–44 MB restore / copy / dump / sliced at 10M) with plain dumps
15–20% slower. With `--features system-allocator`, `MALLOC_MMAP_THRESHOLD_=131072` gives about the same footprint
as musl (158 / 226 / 20 / 40 MB at 10M, flat from 1M to 10M; glibc default 520 / 684 / 75 / 165 MB,
`MALLOC_ARENA_MAX=2` 318 / 432 / 50 / 65 MB) with the same dump slowdown (137–140k docs/s against 162–178k), and
`MALLOC_ARENA_MAX=2` stops the slow growth of long concurrent restores. `--features jemalloc` ran at the same speed.
When the first batch shows that the estimate (still computed as × 1.5 + 10 MB in `src/memplan.rs` until the lead
reviews the change above) exceeds the memory available (the cgroup's `memory.max` or
`MemAvailable`, whichever is smaller), a warning names the flags to lower; under real cgroup caps of 64–256 MB every
run inside the estimate completed (`scripts/res/low-memory.sh`, `results/local/L-635.json`).

## Performance

`scripts/benchmark/quick-vs-node.py` against a single-node OpenSearch 2.19.7 (512 MB heap, podman, same host,
100k docs ≈ 42 MB, release build; medians of 3 runs, L-808). Raw numbers: [`docs/bench/`](docs/bench/).

| Scenario | Node 6.125.1 | elasticdump-rs | Speed-up | CPU-s Node / Rust |
|---|---|---|---|---|
| dump, defaults (`--limit 100`) | 2.85 s | 1.30 s | 2.2× | 2.07 / 0.32 |
| dump `--limit 5000` | 1.00 s | 0.41 s | 2.4× | 1.22 / 0.19 |
| dump `--limit 5000 --fsCompress` | 3.32 s | 0.62 s | 5.4× | 3.41 / 0.74 |
| dump `--limit 5000 --compressionType zstd` | – | 0.36 s | – | – / 0.27 |
| restore `--limit 5000 --concurrency 4 --noRefresh` | 2.29 s | 0.76 s | 3.0× | 2.55 / 0.44 |
| copy `--limit 5000 --concurrency 4 --noRefresh` | 1.77 s | 0.92 s | 1.9× | 1.96 / 0.54 |

Restore and copy are bound by the single 512 MB OpenSearch node here; the CPU column (4.6–6.5× less, 3.6× for
copy) is the more portable result.

At 1M docs of about 1 KB (1.02 GB NDJSON, edgen `logs`, Elasticsearch 7.17 on a 2-shard index, no CPU cap;
`scripts/load/nodecmp.py`, medians of 3 runs, [`results/local/L-808.md`](results/local/L-808.md)), the same flags
give 1.6× (defaults), 2.0× (`--limit 5000`), 6.7× (`--fsCompress`), 1.9× (restore) and 1.3× (copy). Sliced
dumps, which Node cannot do, give 3.5× (`--slices 2` = shards) and 6.4× (`--searchAfter --pit --slices 8`). The
CPU ratio is 2.5–5.5× less. Node output and elasticdump-rs output are digest-equal in every dump scenario.

Compression levels at 1M docs (1.01 GB of NDJSON, edgen `logs` profile, es717-plain; medians of 3 runs, zstd 19 one run;
[`results/local/L-805.md`](results/local/L-805.md)). Per-core speed is the uncompressed MB divided by the CPU-s the codec
adds over an uncompressed file-to-file run:

| `--compressionType` / `--compressionLevel` | gzip 1 | gzip 6 (default) | gzip 9 | zstd 1 | zstd 3 (default) | zstd 9 | zstd 19 |
|---|---|---|---|---|---|---|---|
| ratio | 5.2 | 9.9 | 9.6 | 8.2 | 8.0 | 10.5 | 13.6 |
| size vs gzip 6 | +90% | – | +3.5% | +21% | +25% | −5% | −27% |
| MB/s per core | ~1,000 | ~200 | ~120 | ~700–1,200 | ~530–580 | ~100 | ~1.8 |

A 1M-doc dump (about 7 s, bound by the cluster) took no longer with gzip 1/6 or zstd 1/3; gzip 9 and zstd 9 added
30–40%, and zstd 19 took 9.5 minutes. With the zlib-rs backend, gzip level 9 writes a slightly *larger* file than level
6, so it isn't worth using. zstd 9 gives the smallest file at a practical speed. Because of this, `--compressionLevel 9` with gzip and
levels 19 and up with zstd print a warning on stderr (the level is still used; L-805a).

### Upstream benchmark (Elasticsearch 7.17, pre-rewrite `b32b6e9`)

The maintainer benchmark (`scripts/benchmark/compare-with-elasticdump.sh`) exports a full index to JSONL with both `elasticdump-rs` and the original Node.js `elasticdump`, using each tool's Scroll and PIT modes. Setup: **2,000,000** synthetic log documents (an analyzed ~256-byte `message` plus typical keyword/date/integer fields), Elasticsearch 7.17, index of 2 primary shards / 0 replicas, force-merged to a single segment with the default codec (modelling a static, read-optimized dump target). Host: 32-core x86-64 Linux. Figures are the mean of 2 measured runs after 1 warmup, and throughput is wall-clock (documents ÷ elapsed time).

#### Drop-in default (unsliced), pinned to 1 vs 4 CPU cores

Both tools in their default configuration; each timed run is pinned to *N* CPU cores with `taskset` (the pinning applies to the dump process — Elasticsearch keeps all cores).

Throughput (higher is better):

| Export mode | 1 CPU core | 4 CPU cores |
|---|---|---|
| **elasticdump-rs**, scroll | 375,587 docs/s · 252.3 MiB/s | 339,271 docs/s · 227.9 MiB/s |
| **elasticdump-rs**, PIT | 390,625 docs/s · 269.5 MiB/s | 343,938 docs/s · 237.3 MiB/s |
| elasticdump (Node.js), scroll | 190,749 docs/s · 127.8 MiB/s | 212,314 docs/s · 142.2 MiB/s |
| elasticdump (Node.js), PIT | 191,663 docs/s · 128.4 MiB/s | 210,970 docs/s · 141.3 MiB/s |

Speedup of `elasticdump-rs` over `elasticdump`:

| Metric | 1 CPU core | 4 CPU cores |
|---|---|---|
| Wall-clock, scroll | **1.97× faster** | **1.60× faster** |
| Wall-clock, PIT | **2.04× faster** | **1.63× faster** |
| CPU time consumed, scroll | 5.1× less | 5.2× less |
| CPU time consumed, PIT | 4.4× less | 4.2× less |

Notes:

- **The unsliced `elasticdump-rs` run is bound by Elasticsearch round-trips, not the CPU.** It spends only \~2 s of CPU on a \~5 s dump driving a single sequential cursor, so extra cores do not raise throughput *in this mode* — the small differences between the 1- and 4-core columns are run-to-run variance. Slicing (below) is what engages more cores. The \~4–5× lower CPU cost leaves the machine free for other work either way.
- **`elasticdump` is CPU-bound.** It needs ~10 s of CPU per dump; pinned to a single core it serialises to \~191k docs/s, and only recovers to \~211k docs/s once it can spread across more cores.

#### Sliced retrieval (`--slices`)

The tables above run `elasticdump-rs` with a single retrieval cursor. Passing `--slices` splits the dump into parallel per-slice cursors and changes the picture entirely — same 2M-doc index, no CPU pinning:

| Export mode | Throughput | vs Node.js elasticdump |
|---|---|---|
| **elasticdump-rs**, PIT, `--slices 16` | 2,030,457 docs/s · 1.4 GiB/s | **9.66× faster**, 2.7× less CPU |
| **elasticdump-rs**, scroll, `--slices 2` | 788,955 docs/s · 530.0 MiB/s | **3.77× faster**, 5.4× less CPU |
| elasticdump (Node.js), PIT | 210,084 docs/s · 140.7 MiB/s | — (no slicing support) |
| elasticdump (Node.js), scroll | 209,096 docs/s · 140.1 MiB/s | — (no slicing support) |

Reproduce with e.g. `scripts/benchmark/compare-with-elasticdump.sh --rs-slices 16 --search-types pit`.

Tuning guidance from a slice sweep on this host (2-shard index; the multipliers below are relative to the sweep's own unpinned unsliced baseline of ~380–395k docs/s, not the core-pinned tables above):

- **PIT tolerates — and rewards — more slices than shards.** Throughput rose through 4/8/16 slices (~2.9×/4.2×/5.1×) and levelled off around 16 on this host (24 slices measured flat, 32 slightly slower); Elasticsearch slices PIT searches with an efficient partitioning strategy, and `elasticdump-rs` already defaults PIT to the recommended `_shard_doc` sort.
- **Scroll slicing should match the primary-shard count.** `--slices 2` on the 2-shard index scaled near-perfectly (1.94×), but over-slicing scroll is a trap: 4 slices on 2 shards measured *slower than unsliced* (0.98×), because each shard then evaluates a per-document `_id`-hash slice filter.
- **Keep `--workers` ≥ `--slices`.** The tool clamps workers to `min(workers, slices)`; under-provisioning workers at 8 slices cost ~6% (4 workers) to ~26% (2 workers). The benchmark script matches workers to slices automatically.
- Sliced output interleaves across slices — global document order is not preserved (see the ordering note above).

Absolute numbers depend heavily on your Elasticsearch cluster, network, disk, and page-cache state. Treat the ratios as the portable result and re-run the script in your own environment.

## Testing

Tests are split into tiers (`.config/nextest.toml`; each tier writes JUnit to `target/nextest/<tier>/junit.xml`):

```bash
cargo nextest run --workspace --profile unit       # T0 unit tests (all crates)
cargo nextest run --workspace --profile contract   # T1 CLI contract / injection / secrets / tool tests (mocks only)
cargo nextest run --workspace --profile it --features elasticdump-rs/it      # T2, needs scripts/it/up.sh
cargo nextest run --workspace --profile load --features elasticdump-rs/load  # T3 live long tests
scripts/load/run.sh L0 dump-restore-logs           # load harness (scripts/load/README.md)
cargo test --lib --bins --test cli_contract        # plain cargo test still works (T0 + part of T1)
scripts/it/run.sh all                  # podman: builds first, then every profile below
scripts/it/run.sh opensearch-plain     # one profile: opensearch-{plain,secure}, elasticsearch-{plain,secure}
OS_TAG=3 scripts/it/run.sh opensearch  # both OpenSearch variants against 3.x (default OS_TAG=2)
scripts/it/run.sh elasticsearch8       # both ES variants against the pinned 8.6.1 regression tag
scripts/it/run.sh cross                # ES <-> OpenSearch cluster-to-cluster (two nodes at once)
```

One engine-aware suite runs against every profile; expectations that differ per engine (PIT request shape and
tiebreaker, default settings keys, typeless APIs) are asserted per engine rather than duplicated.

| Profile | Image | Notes |
|---|---|---|
| `opensearch-plain` / `-secure` | `opensearchproject/opensearch:$OS_TAG` | secure = security plugin, basic auth, TLS, client-cert (PKI) auth |
| `elasticsearch8-plain` / `-secure` | `bitnamilegacy/elasticsearch:8.6.1-debian-11-r0` (`ES8_TAG`) | same setup as below; regression target for ES 8 |
| `elasticsearch-plain` / `-secure` | `bitnamilegacy/elasticsearch:9.1.2-debian-12-r0` (`ES_TAG`, latest tag) | bitnami env (`ELASTICSEARCH_HEAP_SIZE`, `ELASTICSEARCH_ENABLE_SECURITY`, `ELASTICSEARCH_ENABLE_REST_TLS`, PEM certs from a generated CA), data dir owned by the image's non-root UID 1001 |
| `es717-plain` / `-secure` | `docker.elastic.co/elasticsearch/elasticsearch:7.17.29` (official) | latest 7.17 patch; bitnamilegacy stops at 7.17.26 |
| `es710-plain` | `bitnamilegacy/elasticsearch:7.10.2` | the OSS build (no X-Pack: no PIT, ILM, security) |
| `es710-secure` | `docker.elastic.co/elasticsearch/elasticsearch:7.10.2` (official) | default distribution: security, PIT, ILM |
| `es819-plain` / `-secure` | `docker.elastic.co/elasticsearch/elasticsearch:8.19.23` (official) | bitnamilegacy stops at 8.18.0 |
| `os13-plain` / `-secure` | `opensearchproject/opensearch:1.3.20` | demo security config (`admin`/`admin`) |
| `cross` | one plain node of each | needs `CROSS_MIN_MB` (default 2200) MB free, otherwise SKIP |

`scripts/it/run.sh matrix` runs the four older/extra versions (plain and secure, one container at a time);
`run.sh es717` etc. run one version. Exact images, digests and the latest counts are in
[TESTING-MATRIX.md](TESTING-MATRIX.md). Minimum supported: Elasticsearch 7.x, OpenSearch 1.x (ES 6.8 is out of
scope; its typed paths remain unit- and mock-tested only).

The secured Elasticsearch profile uses basic auth over HTTPS and verifies client certificates
(`client_authentication: optional`; a rogue certificate is rejected), but cannot authenticate *by* certificate:
the PKI realm is a paid X-Pack feature with no bitnami switch, so the client-certificate tests send basic auth plus
the certificate there. Certificate-only auth is covered by the OpenSearch secure profile.

Cross-engine file dump/restore runs one container at a time: each plain profile exports a zstd dump plus the
expected result (one directory per engine version, e.g. `elasticsearch-8.19.23`), and the other engine restores every
such export later in the same `all` run. Cluster-to-cluster needs both nodes at
once (256m heaps, lean JVM flags, ~1.3 GB RSS together plus headroom); on hosts without that much free memory it
is skipped with a message instead of risking the OOM killer.

Heaps default to 512m (`OS_HEAP`; `ES_HEAP` defaults to the same). Elasticsearch 9 at 512m is ~950 MB RSS; on
hosts with ~1.5 GB free use `ES_HEAP=320m IT_LEAN_JVM=1` (smaller code cache, direct memory and thread stacks).

`scripts/it/run.sh` compiles everything before starting a container (an optimised build next to a running
node can exhaust memory on small hosts) and always removes its containers and data directories. The Node interop tests
run when `/tmp/ed/node_modules/.bin/elasticdump` exists.

## Acknowledgements

elasticdump-rs is a compatible re-implementation of
[elasticsearch-dump](https://github.com/elasticsearch-dump/elasticsearch-dump) (Apache-2.0). Its behaviour and
defaults were studied from that project's source. Some test fixtures are reused unchanged under Apache-2.0; see
[`tests/fixtures/node`](tests/fixtures/node/SOURCE.md).

## License

MIT. See `LICENSE`. Third-party material (the reused test fixtures, the bundled mimalloc and jemalloc C code, the
Rust crates) and its licenses are listed in [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md); regenerate the
crate list with `scripts/third-party-notices.py` after a dependency change.
