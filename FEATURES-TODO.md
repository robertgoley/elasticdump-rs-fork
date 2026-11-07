# Features to do (Node elasticdump parity)

What Node elasticdump does that this Rust version does not yet do, as of `main` at L-705 (2026-10-31). Data types and core dump, restore and copy are at parity, and per-direction credential flags (`--input-username`/`--output-username` and passwords) are done. None of the items below are in the current work queue; each needs an owner decision first (A10 backlog in [`project/handoff/HANDOFF-GROK-OPENCODE.md`](project/handoff/HANDOFF-GROK-OPENCODE.md)). Detail and effort estimates: [`project/plan/GAPS.md`](project/plan/GAPS.md). Effort: S under a day, M a few days, L a week or more.

## Big features

| Feature | Node behaviour | Rust status | Effort | Note |
|---|---|---|---|---|
| `multielasticdump` binary | Dumps or loads many indices in parallel to or from a directory: `--direction dump\|load`, `--match`, `--matchType alias,datastream`, `--parallel`, `--order`, `--interval`, `--ignoreData/Mapping/Settings/Template/Alias/Analyzer/Index/Type`, `--includeType`, `--ignoreChildError`, split-aware load, option pass-through to children | partly done (2026-11-07) | M | Built on `oslogs.py`: `--direction`, `--match`/`--exclude`, per-index or per-regex-group directories, `--parallel`, `--types`/`--ignoreType`, settings/mapping/alias/data, gzip/zstd, count checks, manifest + `--resume`, `--skipExisting`, prefix/suffix, pass-through after `--` (README "Bulk dump and load"). Still missing: `--matchType alias,datastream`, templates/analyzers/policies, `--interval`, split-part loads. Not yet run against a real cluster. |
| AWS SigV4 signing | `--awsAccessKeyId`, `--awsSecretAccessKey`, `--sessionToken`, `--awsChain`, `--awsIniFileProfile`, `--awsIniFileName`, `--awsRegion`, `--awsService`, `--awsUrlRegex` | missing (error) | M | Blocks IAM-protected AWS OpenSearch domains entirely. Credentials and signing are lead-only work; real AWS tests need owner approval (B13). |
| OpenSearch Serverless | `--openSearchServerless`, auto-detected from `server: aoss-amazon`; no scroll or refresh | missing (error) | M | Needs SigV4 with `awsService=aoss`. |
| S3 and MinIO | `s3://bucket/key` input and output with multipart upload; `--s3AccessKeyId`, `--s3SecretAccessKey`, `--s3SessionToken`, `--s3Region`, `--s3Endpoint`, `--s3ForcePathStyle`, `--s3SSLEnabled`, `--s3Compress`, `--s3ServerSideEncryption`, `--s3SSEKMSKeyId`, `--s3ACL`, `--s3StorageClass`, `--s3Options`, `--s3Configs`, `--retryDelayBase`, `--customBackoff` | missing (URL not recognised; flags warn and do nothing) | L | Common for cloud backups. Workaround: output `$` piped to `aws s3 cp -`. |
| OpenDAL additional write path (b2, s3, fs, sftp) (**owner request, 2026-11-07**) | Node writes S3 only (see the row above); B2, SFTP and SSE-C are not in Node | missing | L | A second, separate write path through [OpenDAL](https://crates.io/crates/opendal); the native `std::fs` file paths stay as they are and are not replaced. Selected by URL scheme: `s3://…`, `b2://…`, `sftp://…`, `file://…` / `fs://…`. A helper maps a URL to an OpenDAL `Operator` (enable only the `services-s3`, `services-b2`, `services-sftp`, `services-fs` features). Credentials and endpoints come from env, a config file or URL parameters (secrets redacted in logs and errors like the existing credential flags; never in the repo). **SSE-C** (`server_side_encryption_with_customer_key`) on backends that support it, S3 at minimum: accept the algorithm and customer key from config, env or a secret file, apply them with the official builder method, and document that the server does the encryption (the key is sent with each request and the object cannot be read without it). Async write API, with an optional blocking facade. Tests (MinIO for S3 and SSE-C, an SFTP container, `fs` in temp dirs; B2 and real AWS only with owner approval) and README docs. Overlaps the S3 and MinIO row; see the object_store vs OpenDAL comparison (2026-11-07 chat) when choosing the library. |
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
| Credential hygiene (Grok Bot, 2026-11-02) | Password in argv/URL is visible in `ps` | Same; env `ELASTICDUMP_*_PASSWORD` and `--httpAuthFile` are safe | S | Warn on stderr when a password comes from argv/URL; optional `--password-file`/stdin; `PR_SET_DUMPABLE=0` + `RLIMIT_CORE=0`; reject CR/LF in credentials. See security/findings/2026-11-02-secret-exposure.md |

## Deferred options

| Option | Note |
|---|---|
| Pure-Rust zstd (`ruzstd`) | Would remove C/FFI from the zstd path; deferred until the encoder matures. zstd stays on libzstd. |
| Reading a directory or glob of split parts as one input | Not in Node either (one run per part); relevant with multielasticdump. |
| `--fileSize` UTF-16 mode | Only if exact Node split points are wanted. |

## Build and tooling

| Item | Note |
|---|---|
| Simplify the image build to one script plus one Dockerfile per build type (**owner request, 2026-11-03**) | Today the image build is spread over the Makefile (`image`, `image-dhi-alpine`, `image-dhi-debian`, `image-public`, `image-alpine` plus pins and preflight), `scripts/build-image.sh`, `scripts/vendor-crates.sh` (crates fetched in a separate container, with network, cache-volume and userns workarounds) and one multi-stage `Dockerfile` switched by build args and `--target`. Replace it with a single bash `build-image.sh <type>` that runs `podman build -f Dockerfile.<type>` (for example `Dockerfile.dhi-static`, `Dockerfile.dhi-debian`, `Dockerfile.dhi-alpine`, `Dockerfile.public`, `Dockerfile.alpine`), each a self-contained, readable Dockerfile with its own pinned base images. Keep: a podman-only host (no host Rust), the dhi.io login check with a clear message, `--tag`/`--platform`/`--push`, image tags as today. Decide whether crates are fetched inside the build (a builder stage with network, cached with `RUN --mount=type=cache`) instead of the separate vendoring container, which would remove `vendor-crates.sh` and its DNS/userns workarounds; if offline builds stay a requirement, keep vendoring as an optional `--offline` mode. Drop the Makefile image targets (or make them one-line calls to the script), move pins out of the Makefile into the Dockerfiles, and update `scripts/lint-hygiene.sh` F-14, CI (`ci.yml`/`release.yml` image jobs), `scripts/test-image.sh`, `docker/image-binaries.*.txt`, the README build section and the handoff docs. Done when every current image builds from a fresh clone on Ubuntu with only podman and a dhi.io login, and the image tests pass. |
| multielasticdump in-process (threads instead of child processes) (**owner wishlist, 2026-11-07**) | Today `multielasticdump` starts one `elasticdump-rs` process per index and item type. Running them as tasks in one process would share one memory budget and HTTP connection pool, give one progress view and no binary lookup, and could make a busybox-style single binary possible. Startup cost is not the reason (runs are I/O-bound). Needs the main tool's process-wide state (parsed argv, secrets, memory plan, signal handling, global logging) moved into a per-run context first, and a panic/OOM in one index must not take down the rest (catch_unwind per task, per-task memory caps). Do it after real-cluster testing of the current version. |
