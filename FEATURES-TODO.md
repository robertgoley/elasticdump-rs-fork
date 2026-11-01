# Features to do (Node elasticdump parity)

What Node elasticdump does that this Rust version does not yet do, as of `main` at L-705 (2026-10-31). Data types and core dump, restore and copy are at parity, and per-direction credential flags (`--input-username`/`--output-username` and passwords) are done. None of the items below are in the current work queue; each needs an owner decision first (A10 backlog in [`project/handoff/HANDOFF-GROK-OPENCODE.md`](project/handoff/HANDOFF-GROK-OPENCODE.md)). Detail and effort estimates: [`project/plan/GAPS.md`](project/plan/GAPS.md). Effort: S under a day, M a few days, L a week or more.

## Big features

| Feature | Node behaviour | Rust status | Effort | Note |
|---|---|---|---|---|
| `multielasticdump` binary | Dumps or loads many indices in parallel to or from a directory: `--direction dump\|load`, `--match`, `--matchType alias,datastream`, `--parallel`, `--order`, `--interval`, `--ignoreData/Mapping/Settings/Template/Alias/Analyzer/Index/Type`, `--includeType`, `--ignoreChildError`, split-aware load, option pass-through to children | missing | L | Main tool for whole-cluster migrations. Workaround: shell loop over `_cat/indices`. |
| AWS SigV4 signing | `--awsAccessKeyId`, `--awsSecretAccessKey`, `--sessionToken`, `--awsChain`, `--awsIniFileProfile`, `--awsIniFileName`, `--awsRegion`, `--awsService`, `--awsUrlRegex` | missing (error) | M | Blocks IAM-protected AWS OpenSearch domains entirely. Credentials and signing are lead-only work; real AWS tests need owner approval (B13). |
| OpenSearch Serverless | `--openSearchServerless`, auto-detected from `server: aoss-amazon`; no scroll or refresh | missing (error) | M | Needs SigV4 with `awsService=aoss`. |
| S3 and MinIO | `s3://bucket/key` input and output with multipart upload; `--s3AccessKeyId`, `--s3SecretAccessKey`, `--s3SessionToken`, `--s3Region`, `--s3Endpoint`, `--s3ForcePathStyle`, `--s3SSLEnabled`, `--s3Compress`, `--s3ServerSideEncryption`, `--s3SSEKMSKeyId`, `--s3ACL`, `--s3StorageClass`, `--s3Options`, `--s3Configs`, `--retryDelayBase`, `--customBackoff` | missing (URL not recognised; flags warn and do nothing) | L | Common for cloud backups. Workaround: output `$` piped to `aws s3 cp -`. |
| JavaScript hooks | `--transform` (inline or `@module.js`), `--searchBodyTemplate`, `--inputTransport`/`--outputTransport`, multielasticdump `--customMatcher` | missing (error) | L | Needs a JS engine or a substitute such as WASM or a small DSL; adds security surface. |
| CSV | `csv://path` input and output via fast-csv; `--csvConfigs`, delimiter, quote, escape and row options, header options, `--csvIdColumn`/`--csvIndexColumn`/`--csvTypeColumn`, `--csvHandleNestedData`, trim and skip options | missing (flags warn and do nothing) | L | Spreadsheet export and import rather than migration. |
| SOCKS5 proxies | `--inputSocksProxy`/`--inputSocksPort`, `--outputSocksProxy`/`--outputSocksPort` | missing (error) | S–M | Workaround: `ssh -L` tunnel. |
| Move instead of copy | `--delete`, `--delete-with-routing` (and `--delete` in multielasticdump) | missing (error) | M | Destructive; deletes source docs after moving them. |

## Small differences

| Area | Node | Rust | Effort | Note |
|---|---|---|---|---|
| `--pass` | Any encrypted key format | Encrypted PKCS#8 only | S | Legacy encrypted PEM (`Proc-Type: 4,ENCRYPTED`) needs `openssl pkcs8 -topk8` first; clear error with that hint. |
| `--fileSize` | Counts UTF-16 code units | Counts UTF-8 bytes | S | Same split points for ASCII; differ for non-ASCII data. |
| `--scroll-with-post` | GET unless set | Ignored, always POST | S | Matters only behind a proxy that blocks POST to `_search/scroll`. |
| `scrollRetryDelay` | Retries a partial page after a delay | Ignored; a partial page is a hard error | S | Rust fails safe. D-1 adds an opt-in flag for red indices. |
| `--force-os-version` | Forces the assumed OpenSearch version | Ignored; auto-detects | S | Only matters if a proxy hides `GET /`. |
| Type in URL path | Parses `/index/type` (ES 5 and 6) | Not parsed | S | Use `--output-index idx/type`. |
| Bulk HTTP method | PUT | POST | S | Only matters if a proxy allow-lists verbs. |
| Default User-Agent | `elasticdump` | Rust's own | S | Only matters where a WAF allow-lists the UA. |
| `--toLog` | Logs into a custom transport stream | Always stderr | S | Only relevant with custom transports. |
| `--maxSockets` | Unlimited by default | Also caps concurrent requests | – | Throughput tuning only. |
| `--throttleInterval` | 1 ms sleep per batch by default | 0 unless given | – | Faster. |
| `--intervalCap` and related | Effectively no-ops in Node 6.119+ | Enforced when set | – | Old scripts that set them now really rate-limit. |
| `--tlsAuth` | Cert flags apply only when set | Cert flags always apply | – | Safer by design. |
| `--offset` with scroll | Sends `from=` | Skips client-side | – | More correct. |
| `--fileSize`/`--maxRows` commit | Parts written as it goes | Staged, committed at the end | – | Safer; needs disk for the whole dump; leftovers are warned about. |
| `--format human` | `util.inspect` output | Pretty JSON | – | Cannot be combined with splitting. |
| `--esCompress` | Gzip responses only | Also gzips bulk bodies | – | Some proxies reject gzipped requests. |
| `--debug` log format | Node format | Different format | – | Log parsers may need changes. |
| `--heapSnapshot`, `--jsonLines`, `--support-big-int` | Node options | Accepted and ignored | – | Not applicable; big ints are always lossless. |

## Deferred options

| Option | Note |
|---|---|
| Pure-Rust zstd (`ruzstd`) | Would remove C/FFI from the zstd path; deferred until the encoder matures. zstd stays on libzstd. |
| Reading a directory or glob of split parts as one input | Not in Node either (one run per part); relevant with multielasticdump. |
| `--fileSize` UTF-16 mode | Only if exact Node split points are wanted. |
