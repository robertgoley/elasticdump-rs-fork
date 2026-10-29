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
elasticdump-rs --input=http://os:9200/src --output=dump.json --compressionType zstd                 # zstd (preferred)
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
  `--ignore-errors`, `--ignore-es-write-errors` (default true), `--parseExtraFields`.
* **Files:** `--fsCompress`, `--compressionType gzip|zstd`, `--compressionLevel`, `--fileSize`, `--maxRows`.
* **Throughput:** `--concurrency`, `--throttleInterval`, `--intervalCap`, `--concurrencyInterval`,
  `--carryoverConcurrencyCount`, `--maxSockets`, `--timeout`.
* **Retries:** `--retryAttempts` (0 = Node's 4 implicit retries), `--retryDelay` (5000 ms).
* **Auth/TLS:** `--httpAuthFile`, `--tlsAuth`, `--ca`/`--cert`/`--key`/`--pass` and their `--input-*`/`--output-*` forms.
* **Metadata:** `--templateRegex`, `--filterSystemTemplates` (Node), and the extensions `--filterManaged`
  (default true), `--closeIndex`, `--excludeTemplateFields`, `--convertIlmToIsm`, `--lenientPolicyConversion`,
  `--metaFormat string|object`, `--renamePrefix`, `--renameIndex`, `--renameAliases`, `--renamePatterns`.

Rust-only extensions (all opt-in): `--compressionType zstd`, `--slices`, `--workers`, `--bufferSize`,
`--searchType scroll|pit`, `--skip-existing`, `--excludeSettings`, `--maxBulkBytes`, `--retryOn`,
`--retryBackoff`, `--noImplicitRetry`, `--insecure`/`--input-insecure`/`--output-insecure`, `--username`/`--password`
(input), `--caFile`, `--requestTimeout`.

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
  `--fsCompress` keeps meaning gzip.
* **Compressed input is auto-detected** (gzip or zstd magic bytes) with or without `--fsCompress`.
* **`--fileSize` counts UTF-8 bytes**; Node counts UTF-16 code units. Identical part names for ASCII data
  (verified against Node), slightly different boundaries for non-ASCII text.
* **Split parts are staged and committed together** at the end of a successful run; a failed run leaves nothing.
  Existing `*.split-*` files need `--overwrite`.
* **Certificate flags always apply**; Node ignores `--ca/--cert/--key` unless `--tlsAuth` is set.
* **Retries also cover HTTP 429**, and bulk items rejected with 429/503 are retried individually.
* **PIT is engine-aware**: Elasticsearch 7.10+ uses `POST /{index}/_pit` and the `_shard_doc` tiebreaker;
  OpenSearch uses `/_search/point_in_time` and `_doc` + `_id` (a `_doc`-only sort was verified to lose documents).
* **Scroll + `--offset`** skips client-side; Node sends `from=`, which ES 7+/OpenSearch reject.
* **`--throttleInterval`** only applies when given (Node's 1 ms default would add ~10 s per million docs).
* **`--intervalCap`/`--concurrencyInterval`/`--carryoverConcurrencyCount`** work (p-queue semantics) when set;
  they have been no-ops in Node since 6.119.
* **`--format human`** writes pretty-printed JSON (restorable), not Node's `util.inspect` output; it cannot be
  combined with splitting.
* **`--searchWithTemplate`** renders `{"id"|"source", "params"}` via `_render/template`, then searches normally.
* **Big integers and unicode** are preserved byte-for-byte in both directions (no `--support-big-int` needed).
* **Bulk uses POST**; `/index/type` URLs are not parsed (use `--output-index idx/type` for ES < 7).
* **`--username`/`--password` apply to the input only.**
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
| `analyzer` | `{index}/_settings` (the analysis block is what matters) | new index: created with the analysis; existing index: error, `--skip-existing`, or `--closeIndex` (close → update → always reopen; the index is **offline** meanwhile) |
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
| `shrink` | `shrink` (`num_new_shards` / `max_shard_size`) | |
| `readonly` | `read_only` | |
| `delete` | `delete` | `delete_searchable_snapshot` dropped (warned) |
| `migrate`, implicit tier migration | — | dropped with a warning (OpenSearch has no data tiers) |
| `_meta.description` | `description` | other `_meta` keys and `deprecated` dropped (warned) |
| `frozen` phase, `searchable_snapshot`, `downsample`, `wait_for_snapshot`, `unfollow`, `freeze`, `total_shards_per_node`, `index_codec`, `allow_write_after_shrink: true`, `max_primary_shard_docs`, `min_*` rollover conditions, unknown actions | — | **refused** unless `--lenientPolicyConversion` |

Converted policies are not attached to indices (ILM attaches through `index.lifecycle.name`, ISM through
`ism_template` or `POST _plugins/_ism/add/<index>`); the tool says so after converting. Tested on OpenSearch 1.3,
2.x and 3.x, from ES 7.17, 8.6, 8.19 and 9.1 exports and cluster to cluster.

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

`Dockerfile` is a two-stage build: a Rust builder (release profile, `CARGO_BUILD_JOBS=2` by default
because the fat-LTO link peaks at ~1.5-2 GB) and a runtime with `bash`. Builder and runtime always use the
same libc. All four builds use the same Dockerfile through the `BUILDER_IMAGE`/`BUILDER_TAG`/`BASE_IMAGE`/`BASE_TAG`
build args:

| Make target | Builder | Runtime | libc | Size | HIGH/CRITICAL CVEs (2026-10-29) |
|---|---|---|---|---|---|
| `make image` (**default**) | `dhi.io/rust:1.99.0-alpine3.23-dev` | `dhi.io/alpine-base:3.24-alpine3.24-dev` + apk `bash ca-certificates libgcc` | musl | 20 MB | **0** (0 at any severity) |
| `make image-dhi-debian` | `dhi.io/rust:1.99.1-debian13-dev` | `dhi.io/debian-base:trixie-debian13-dev` | glibc 2.41 | 90 MB | 15 (grype) / 9 (trivy) distinct CVEs, none fixable |
| `make image-alpine` | `docker.io/library/rust:1.99.1-alpine3.24` | `docker.io/library/alpine:3.24.3` + apk `bash ca-certificates libgcc` | musl | 20 MB | 0 |
| `make image-public` | `docker.io/library/rust:1.99-trixie` | `docker.io/library/debian:trixie-slim` | glibc 2.41 | 90 MB | 54 High matches (grype) / 43 (trivy), none fixable |

The default is the dhi.io Alpine build. The dhi.io Debian `-dev` base ships apt, perl, util-linux and other packages
with open CVEs that Debian marks as won't-fix. Details: [SECURITY-CVE.md](SECURITY-CVE.md). On Alpine runtimes the Dockerfile
runs `apk upgrade`, then adds `bash`, `ca-certificates` and `libgcc`. Alpine-packaged rustc links `libgcc_s`
dynamically.

`dhi.io` (Docker Hardened Images) needs a pull login: `podman login dhi.io` or `docker login dhi.io` with a Docker
account. Anonymous pulls return `authentication required`. The login stays in the container tool's auth file and
never enters a build layer. The `-dev` variants are used on purpose, so that a shell is available for wrapper
scripts.

The image contains `/usr/local/bin/elasticdump-rs` plus an `elasticdump` symlink, runs as uid
65532 (non-root), has a CA bundle (`SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt`), uses
`/data` as its writable working directory, and carries OCI labels (version, revision, created,
base name).

```bash
make image                 # dhi.io Alpine bases, the default (after `podman login dhi.io`)
make image-dhi-debian      # dhi.io Debian 13 bases (after the login)
make image-public          # public Debian 13 bases, no login
make image-alpine          # public Alpine/musl bases, no login, fewest CVEs
make test-image            # --version, --help, bash present, non-root, symlink, CA bundle
make sbom scan             # syft SBOM (CycloneDX + SPDX), grype + trivy (fail on HIGH/CRITICAL)
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
  meanwhile is never clobbered).
* Ctrl+C clears the scroll / closes the PIT (ES input pipelines).

## Security notes

* Credentials in URLs are redacted in every log line; prefer `--httpAuthFile` or the environment variables so
  they stay out of shell history and `ps`.
* `--insecure` / `NODE_TLS_REJECT_UNAUTHORIZED=0` disable certificate verification and print a warning.
* Encrypted client keys: PKCS#8 only (convert legacy keys with `openssl pkcs8 -topk8`).

## Memory

Peak RSS ≈ `(bufferSize + slices + concurrency) × limit × avg_doc_size × ~1.5 + ~10 MB`. Measured (100k docs,
~420 B each): 11 MB dumping at `--limit 100`, 25–35 MB at `--limit 5000` (sliced or not), 61 MB restoring and
74 MB copying at `--limit 5000 --concurrency 4`.

## Performance

`scripts/benchmark/quick-vs-node.py` against a single-node OpenSearch 2.19.7 (512 MB heap, podman, same host,
100k docs ≈ 42 MB, `it` profile build). Raw numbers: [`docs/bench/`](docs/bench/).

| Scenario | Node 6.125.1 | elasticdump-rs | Speed-up | CPU-s Node / Rust |
|---|---|---|---|---|
| dump, defaults (`--limit 100`) | 3.75–4.28 s | 1.36–1.51 s | 2.8× | 2.6 / 0.26 |
| dump `--limit 5000` | 1.14 s | 0.28 s | 4.1× | 1.43 / 0.13 |
| dump `--limit 5000 --fsCompress` | 3.22 s | 1.02 s | 3.2× | 3.36 / 1.09 |
| dump `--limit 5000 --compressionType zstd` | – | 0.29 s | – | – / 0.18 |
| restore `--limit 5000 --concurrency 4 --noRefresh` | 2.50 s | 0.98 s | 2.6× | 2.65 / 0.23 |
| copy `--limit 5000 --concurrency 4 --noRefresh` | 1.92 s | 1.13 s | 1.7× | 2.08 / 0.24 |

Restore and copy are bound by the single 512 MB OpenSearch node here; the CPU column (9–11× less) is the
portable result. gzip level 6 compresses ~80 MB/s per core on this text (zlib-rs backend); level 1 ~500 MB/s;
zstd level 3 ~480 MB/s (file ~18% larger than gzip -6 on this text; raise `--compressionLevel` for smaller files).

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

```bash
cargo test --lib                       # unit tests
cargo test --test cli_contract         # CLI contract tests against an in-process mock server
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

## License

MIT. See `LICENSE`.
