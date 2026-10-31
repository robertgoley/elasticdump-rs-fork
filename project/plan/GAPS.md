# elasticdump-rs: remaining gaps vs Node elasticdump (excluding `--type` values)

Sources checked on 2026-10-29: Node clone `/workspace/elasticsearch-dump` (`bin/elasticdump` and `bin/multielasticdump` defaults, `lib/help.txt`, `lib/transports/*`, `lib/request.js`, `lib/processor.js`), and Rust branch `rewrite` (`src/cli.rs` including the `OUT_OF_SCOPE` pre-scan, `README.md`, `docs/PARITY.md`). Missing `--type` values (analyzer, alias, template, component_template, index_template, policy, script, index) are covered in another document and left out here. Only the `--type`-specific flags that sit next to them (`--templateRegex`, `--filterSystemTemplates`) are listed, so their CLI status is on record.

Status key:
- **missing**: the flag is rejected with an error, or the feature does not exist.
- **accepted-but-ignored**: the flag parses, or is removed with a warning, and has no effect.
- **partial**: only part of the Node behaviour is there.
- **deviates**: it works, but differently from Node.

Effort: S = under a day, M = a few days, L = a week or more. This file does not recommend what to build.

## 1. Document transformation and query templating
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `--transform` (inline JS, repeatable) | Runs a JS snippet on every `doc` before writing | missing (error) | L | Often used for field renames/anonymising during migration. A JS engine (or a substitute DSL) adds weight and security surface. |
| `--transform=@module.js` (e.g. `transforms/anonymize.js`) | Loads a JS module transform with arguments | missing (error) | L | Same as above. A Rust plugin or WASM substitute would not run existing user scripts as-is. |
| `--searchBodyTemplate` (inline or `@module`, e.g. `templates/dates.js`) | JS that builds or rewrites `searchBody` at runtime | missing (error) | M | Used for date-windowed dumps. Can be worked around by templating the shell command. |
| `--inputTransport` / `--outputTransport` | Custom JS transport file | missing (error) | L | Rare. Node-specific plugin API. |
| `--toLog` | Whether log lines go into a custom output transport stream | deviates | S | Rust always logs to stderr. Only matters together with custom transports. |

## 2. CSV
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `csv://path` input/output transport | Reads and writes CSV through fast-csv, mapped to `_source` plus `@id`/`@index`/`@type` columns | missing (`csv*` flags are removed with a warning; the URL is not recognised) | L | Used for spreadsheet export/import, not for cluster migration. |
| `--csvConfigs` (JSON or @file) | Passes every fast-csv option | accepted-but-ignored (warns) | M | Comes with the CSV transport. |
| `--csvDelimiter`, `--csvRowDelimiter`, `--csvQuoteChar`, `--csvEscapeChar` | Format control | accepted-but-ignored | S each, once CSV exists | Format fidelity. |
| `--csvFirstRowAsHeaders`, `--csvCustomHeaders`, `--csvRenameHeaders`, `--csvWriteHeaders` | Header handling | accepted-but-ignored | S | Same. |
| `--csvIdColumn`, `--csvIndexColumn`, `--csvTypeColumn`, `--csvIgnoreAutoColumns` | Map metadata to and from columns | accepted-but-ignored | S | Needed for round-tripping ids. |
| `--csvHandleNestedData` | Flattens and unflattens nested objects (opinionated) | accepted-but-ignored | M | Behaviour that is tricky to match exactly. |
| `--csvIgnoreEmpty`, `--csvSkipLines`, `--csvSkipRows`, `--csvMaxRows`, `--csvTrim`, `--csvLTrim`, `--csvRTrim`, `--csvDiscardUnmappedColumns`, `--csvIncludeEndRowDelimiter` | Parse and write tweaks | accepted-but-ignored | S | Low risk. |

## 3. S3 / MinIO
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `s3://bucket/key` input/output | Streams a dump to or from S3 (multipart upload) | missing (the URL is not recognised; `s3*` flags warn) | L | Common in cloud backup jobs. Workaround: `$` piped through `aws s3 cp -`. |
| `--s3AccessKeyId`, `--s3SecretAccessKey`, `--s3SessionToken`, `--s3Region` | S3 credentials and region | accepted-but-ignored | M | Comes with S3. |
| `--s3Endpoint`, `--s3ForcePathStyle`, `--s3SSLEnabled` | MinIO, Ceph, Swift and other S3-compatible stores | accepted-but-ignored | S | Needed for on-prem MinIO. |
| `--s3Compress` | gzip before upload | accepted-but-ignored | S | `--fsCompress` would likely cover the same need. |
| `--s3ServerSideEncryption`, `--s3SSEKMSKeyId`, `--s3ACL`, `--s3StorageClass` | Upload parameters | accepted-but-ignored | S | Compliance-sensitive buckets may require them. |
| `--s3Options`, `--s3Configs` (JSON or @file) | Raw SDK options | accepted-but-ignored | S | Escape hatch. |
| `--retryDelayBase`, `--customBackoff` | S3 SDK retry backoff | accepted-but-ignored (hidden) | S | Only meaningful with S3. |

## 4. AWS / cloud authentication and networking
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `--awsAccessKeyId`, `--awsSecretAccessKey`, `--sessionToken` | SigV4-signs requests to Amazon ES/OpenSearch | missing (error) | M | **Blocks IAM-protected AWS OpenSearch domains entirely.** |
| `--awsChain` | Standard AWS credential chain (env, profile, EC2/ECS role) | missing (error) | M | Recommended AWS path. Needed on EC2/EKS. |
| `--awsIniFileProfile`, `--awsIniFileName` | Credentials from an ini profile | missing (error) | S once SigV4 exists | |
| `--awsRegion`, `--awsService`, `--awsUrlRegex` | Signing scope and which URLs get signed | missing (error) | S once SigV4 exists | `awsService=aoss` is needed for Serverless. |
| `--openSearchServerless` (also auto-detected from `server: aoss-amazon`) | Adjusts requests for AOSS (no scroll/refresh etc.) | missing (error) | M | Only for Serverless users. |
| `--inputSocksProxy`/`--inputSocksPort`, `--outputSocksProxy`/`--outputSocksPort` | SOCKS5 proxy per direction | missing (error) | S–M | Used to reach clusters behind bastions. Workaround: `ssh -L` tunnel. |

## 5. Write side / data-movement modes
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `--delete` | Deletes each source doc after it is moved (a move, not a copy) | missing (error) | M | Destructive. Rarely wanted, but some people use it for "drain" jobs. |
| `--delete-with-routing` | Passes `routing` on those deletes | missing (error) | S once `--delete` exists | Only needed with routed indices. |
| `--noRefresh` | Node: skips the `_refresh` call it makes after writes | deviates (minor; verify) | S | Check that Rust's default refresh matches Node's per-batch `_refresh` behaviour. Affects speed, not correctness. |
| `--handleVersion` / `--versionType` | Puts `_version` / `version_type` into bulk actions; Node also documents `force` | implemented; `force` is passed through unvalidated, as in Node | – | No known gap. Listed for completeness. |
| Bulk HTTP method | Node uses PUT for `_bulk` | deviates (POST) | S | Harmless unless a proxy only allows specific verbs. |
| `/index/type` in `--input`/`--output` URL | Node parses the type from the URL (ES < 7) | deviates (not parsed; use `--output-index idx/type`) | S | Breaks old ES 5/6 scripts that put the type in the URL. |
| `--prefix` / `--suffix` | Rename the output index | implemented | – | No gap found. |

## 6. Retrieval / search
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `--scroll-with-post` | Node scrolls with GET unless this is set | accepted-but-ignored (always POST) | S | Only matters behind a proxy that blocks POST to `_search/scroll`. |
| `scrollRetryDelay` (undocumented, 15000 ms) | Retries a partial or failed scroll page after a delay | accepted-but-ignored (partial pages are a hard error) | S | Rust fails safe. A Node dump that "recovers" from flaky shards will abort in Rust. |
| `--offset` with scroll | Sends `from=` | deviates (skips client-side) | – | More correct in Rust. Large offsets still read the skipped docs. |
| `--support-big-int`, `--big-int-fields` | Opt-in lossless big integers | accepted-but-ignored (always lossless) | – | No gap in practice. |
| `--searchWithTemplate` | Sends search-template requests | deviates (`_render/template`, then a normal search) | S | Same result. The template `size` is overridden by `--size`, as in Node. |
| Default `searchBody` | `{"query":{"match_all":{}},"stored_fields":["*"],"_source":true}` | verify | S | Hits with stored fields might be written differently. |

## 7. HTTP client / transport options
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `--headers` default | `{"User-Agent":"elasticdump"}` | deviates (Rust sends its own UA) | S | Matters only where a WAF or proxy allow-lists the UA. |
| `--headers` / `--params` / `--input-*` / `--output-*` | Extra headers and query params | implemented | – | No gap found. |
| `--esCompress` | Accept-Encoding gzip on responses | implemented (also gzips bulk bodies) | – | Gzipped bulk bodies are a small deviation; some proxies reject them. |
| `--timeout` | Per-request timeout in ms | implemented | – | |
| `--maxSockets` | Node default is unlimited | deviates (also caps concurrent requests; default pool) | S | Throughput tuning only. |
| `--tlsAuth` | Node only applies `--ca`/`--cert`/`--key` when it is set | deviates (certificate flags always apply) | – | Safer. A script that passes certs without `--tlsAuth` changes behaviour (now verifies or uses them). |
| `--pass` | Passphrase for any key format | partial (encrypted PKCS#8 only) | S | Legacy encrypted PEM keys (`BEGIN RSA PRIVATE KEY` + DEK-Info) need converting with `openssl pkcs8`. |
| `--username` / `--password` | (Rust extension) | deviates (input only) | S | Output auth needs the URL, `--httpAuthFile` or the env vars. Surprising if someone assumes these cover both sides. |
| `--throttleInterval` | Default 1 ms sleep per batch | deviates (0 unless given) | – | Faster. Only "risky" if someone relied on the implicit pacing. |
| `--intervalCap` / `--concurrencyInterval` / `--carryoverConcurrencyCount` | Defaults 5 / 5000 / true; effectively no-ops in Node ≥ 6.119 | deviates (enforced when set) | – | Setting them in Rust now really rate-limits, which can slow old scripts that set them. |
| `--debug` | Prints every ES command | implemented | – | Log format differs, so log parsers may break. |
| `--quiet` | Errors only | implemented | – | |
| `--heapSnapshot` | Node heap dump | accepted-but-ignored | – | Not applicable to Rust. |
| `--force-os-version` | Forces the assumed OpenSearch version (default 7.10.2) | accepted-but-ignored (auto-detect) | S | Only matters if detection fails behind a proxy that hides `/`. |
| `--jsonLines` | No-op in Node | accepted-but-ignored | – | |

## 8. Files, stdin/stdout and splitting
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `--fileSize` | Split by size, counted in UTF-16 code units | deviates (UTF-8 bytes) | S | Same part names for ASCII. Part boundaries differ for non-ASCII data. |
| `--fileSize` / `--maxRows` commit model | Writes parts as it goes; a crash leaves the parts already written | deviates (staged, committed at the end; existing `*.split-*` files need `--overwrite`) | – | Safer, but needs disk space for the whole dump before commit, and a failed run leaves nothing to resume from. |
| `--format human` | `util.inspect` output | deviates (pretty JSON) | – | Cannot be combined with splitting. |
| Reading a directory or glob of split parts as one input | Not supported in Node either (one run per part; multielasticdump handles split files in `load` mode) | parity (one run per part) | M if wanted | Relevant only together with multielasticdump. |
| Compressed input detection | Needs `--fsCompress` to inflate | deviates (auto-detects gzip/zstd) | – | Superset. |
| `--overwrite` | Overwrite the output file | implemented (atomic; no-clobber hard link without it) | – | |
| stdin/stdout `$` | Supported; logs go to stderr when output is `$` | implemented | – | `--toLog` nuance only (see §1). |
| `--compressionLevel` | zlib level | implemented (+ zstd) | – | |

## 9. multielasticdump (whole binary missing)
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `multielasticdump` binary | Dumps or loads many indices in parallel to or from a directory | missing | L | **Main tool for whole-cluster migrations.** Shell loops over `_cat/indices` are the workaround. |
| `--direction dump\|load` | Cluster → dir, or dir → cluster | missing | (in L) | |
| `--match` (regex, default `^.*$`) | Selects indices | missing | (in L) | |
| `--matchType alias,datastream` (default `alias`) | Selects through `/_aliases` or `/_data_stream` | missing | (in L) | Data-stream users need this. |
| `--customMatcher` (JS or `@file`, e.g. `customMatcher/backup.js`) | Custom index filter function | missing | M | Needs JS, or a substitute such as a regex/list file. |
| `--parallel` (default number of CPUs) | Number of indices processed at once | missing | (in L) | |
| `--order asc\|desc` | Index processing order | missing | S | |
| `--interval` (default 1000 ms) | Polling interval between child launches | missing | S | |
| `--ignoreData`, `--ignoreMapping`, `--ignoreSettings`, `--ignoreTemplate`, `--ignoreAlias` (default true), `--ignoreAnalyzer` (default true), `--ignoreIndex` (default true), `--ignoreType`, `--includeType` | Chooses which per-index artefacts to dump or load | missing | M | Partly depends on the missing types. |
| `--ignoreChildError` | Keeps going when one index fails | accepted-but-ignored in single dump; missing in multi | S | Important for large clusters with a few broken indices. |
| Split-file awareness in `load` (`*.split-N.json(.gz)` regex) | Loads split parts per index | missing | M | |
| `--delete` in multi load/dump | Deletes the source index after processing | missing | S | Destructive. |
| Pass-through of options to child processes (auth, TLS, AWS, S3, prefix/suffix, fsCompress…) | Forwards options to each child | missing | (in L) | |

## 10. Type-adjacent flags (listed only for their CLI status)
| Feature / option | Node behaviour | Rust status | Effort | Value / risk note |
|---|---|---|---|---|
| `--templateRegex`, `--filterSystemTemplates` | Filter templates | accepted-but-ignored (warns) | – | Depends on the template types (covered in the other doc). |
| `--skip-existing` | Node: skips `resource_already_exists_exception` | implemented | – | |

## Counts by status (items in tables; "implemented"/parity rows excluded)
- missing: 24
- accepted-but-ignored: 20
- deviates: 16
- partial: 1

Rows counted as "implemented" or parity, or marked verify, are there only to show coverage. "Verify" rows (`--noRefresh`, default `searchBody`) were not traced through the Rust code paths and need a code-level check.

## Future options (deferred)

| Option | What it would do | Status | Effort | Note |
|---|---|---|---|---|
| Pure-Rust zstd backend (`ruzstd`) | Use `ruzstd` for all zstd decompression (restore), and offer a build feature to choose the zstd compression backend: C `zstd` (default; full levels 1–22, fastest) or pure-Rust `ruzstd` (no C code in the build) | Deferred until the `ruzstd` encoder is more stable | M | Removes C/FFI code from the zstd path. The decoder is mature; the encoder is newer, supports fewer levels and compresses less tightly and more slowly at high levels. Revisit when the encoder matures. |
