# Testing to do

Every test idea that is **not** part of the other agent's task plan. The delegated plan is in [TESTING-IN-PROGRESS.md](TESTING-IN-PROGRESS.md), finished work in [TESTING-COMPLETED.md](TESTING-COMPLETED.md). Requirement IDs refer to [`project/plan/TEST-REQUIREMENTS.md`](project/plan/TEST-REQUIREMENTS.md).

## 1. Further tests

### Stability
| Test | What it checks |
|---|---|
| Cluster faults mid-dump on a 3-node cluster | Node restart, shard relocation, master election and a rolling upgrade while dumping and restoring; extends B8 with a full scenario list. |
| Source changing during a dump | Documents written or deleted while dumping, data-stream rollover mid-dump, scroll context expiry under load; record exactly what was and was not copied. |
| Soak with resource tracking | 12 to 24 h runs logging RSS, open file descriptors and sockets over time, plus heaptrack or valgrind massif profiles; B5 only checks pass or fail. |
| Squeezed resources | cgroup CPU throttling (`cpu.max`), a very low open-file limit during a large parallel copy, IPv6-only clusters, DNS failure or change mid-run, TLS certificate rotation mid-run. |
| Slow servers | Responses trickled one byte at a time and stalls mid-headers, checking `--timeout` and `--idleTimeout` under high concurrency. |
| l638 `--intervalCap` timing on a stable machine | Re-run `chaos_brk::l638_extreme_numeric_flags_never_panic` (the `--intervalCap 1` case, 40 s limit) many times on a more stable or dedicated machine, alone and in the full chaos profile, to settle the timing flake seen once under load in L-808b (the loaded egress run of `rc-diff.sh`; it passes alone in 22.9 s and passed in every gate run). Do not widen the limit before that (user decision 2026-11-01, L-808d). |
| Full arm64 image build (**low priority**) | Build and run both images for `linux/arm64` (`scripts/build-image.sh --platform linux/arm64`, then `make test-image`) on an arm64 host or with qemu/binfmt; so far only the arm64 base images were resolved (L-808b). Deprioritized by the user on 2026-11-01 (L-808d). |

### Performance

- **Sequential speed suite (t89u): pending real-engine validation.** Defined/scripted in
  [scripts/perf/README.md](scripts/perf/README.md), entrypoint `scripts/perf/run-suite.sh`.
  Run all 37 cases across the 16 plain/secure profiles (all nine database images from
  `images.lock`) with Node elasticdump 6.125.1, including `searchdump-multi` against Node's `multielasticdump`. Validate
  the Podman-only controller build/remote socket, named-volume cleanup, auth, every
  codec/split/metadata path and failure/interruption teardown. Then run L0 and dedicated-host
  L1, record reviewed per-engine baselines, rerun with the 10% regression gate, and retain
  `combined.json`/`summary.md`. Offline plan/summary/comparator tests do **not** establish
  correctness or speed against real engines. No real suite baseline exists yet.

| Test | What it checks |
|---|---|
| Networked backup and restore with compression at scale | Tool and cluster on **separate machines** over a real network, with bandwidth and latency measured first (for example 1 Gbit and 10 Gbit links, iperf3 and ping); a multi-node cluster; 10M+ docs and multi-GB files. Dump (scroll unsliced, scroll with slices = shards, PIT with 4/8/16 slices) and restore (`--concurrency` 1/4/8/16) with `--compressionType` none, gzip levels 1 and 6, and zstd levels 1, 3 and 9. Record throughput in MB/s and docs/s, client CPU-s and server CPU per node, and network utilisation, and find out whether compression pays off once the network, not the CPU or the cluster, is the bottleneck. Baseline: the box results in [`results/local/L-805.md`](results/local/L-805.md) (compression levels) and [`results/local/L-807.md`](results/local/L-807.md) (concurrency curves). Those ran tool and engine on one 8-vCPU machine over loopback, with no real network, so they measure codec CPU and engine limits, not transfer cost. |

### Malicious or broken input
| Test | What it checks |
|---|---|
| Hostile cluster proxy | A proxy that mutates every response: gzip and zstd decompression bombs in `Content-Encoding`, endless chunked bodies, huge headers, redirects to other hosts or `file://`; credentials are never sent across a redirect. |
| Header and request injection | CR/LF in `--headers` and `--params`, and request smuggling attempts through a reverse proxy. |
| Sanitizer and Miri runs | Miri on code next to `unsafe`, ThreadSanitizer and UndefinedBehaviorSanitizer runs of the test suite (only AddressSanitizer via fuzzing so far). |
| Mutation testing (`cargo-mutants`) | Deliberately breaks the code to measure whether the tests notice; the main check on AI-written test strength. |
| Differential fuzzing against Node | Same generated clusters and files through both tools, outputs diffed automatically, beyond the fixed interop corpus. |
| Release integrity | cosign signing and provenance attestations for binaries and images. |
| Dependency update review | Six open Dependabot branches (base64, pkcs8, rand, rand_chacha, sha2, GitHub Actions): run the full gate, cargo vet and deny on each before any merge; dependencies are pinned on purpose. |

## 2. Foreign or malicious code detection (project and dependencies)

Builds on what is already done: cargo vet and deny, the `unsafe` inventory, SBOM diffs, secret scans, the hostile-code audit of upstream code, egress-denied runs (L-701), syscall tracing (L-702) and the reproducible-build check (L-704).

### Own code and history
| Test | What it checks |
|---|---|
| Upstream line-origin review | `git blame` surviving upstream lines back to their original authors; focused manual review of any that touch networking, file paths, credentials, process spawning or `build.rs`. |
| Mutation testing of security guards | `cargo-mutants` on credential redaction, the injection guards and TLS, proving the tests catch any weakening. |

### Dependencies
| Test | What it checks |
|---|---|
| Published crate vs tagged source | Diff each vendored `.crate` against its tagged repository; malicious code often exists only in the published package (as in the xz backdoor). |
| Full cargo vet coverage | Replace exemptions with real audits or imported audits (Mozilla, Google, Bytecode Alliance); diff-audit every version bump, including the open Dependabot branches. |
| Vendored, pinned builds | Build only from vendored sources with checksums, never fresh registry fetches; fail the build on an unexpected `Cargo.lock` change. |

### Binary behaviour
| Test | What it checks |
|---|---|
| Wider runtime monitoring | Egress and syscall tests across every CLI option; eBPF tracing of all network, file and process activity (extends B12); runs with the clock moved months ahead and unusual environment variables to catch time- or environment-triggered behaviour. |
| Container image checks | Layer-by-layer scan of built images for unexpected files (Syft, Grype, Trivy) and comparison against the dhi.io base image digests. |
| Independent rebuild and signing | Rebuild on a second machine (B11), match hashes, then sign releases with cosign and provenance attestations. |

### Tooling and CI
| Test | What it checks |
|---|---|

## 3. Tests that come with unbuilt features

Each feature in [FEATURES-TODO.md](FEATURES-TODO.md) needs its own contract, interop and break tests when it is built (searchdump-multi, AWS SigV4 and Serverless, S3/MinIO, CSV, SOCKS5, `--delete`, transforms). AWS tests (B13) stay blocked until the owner approves a named sandbox account.

## searchdump-multi (added 2026-11-07)

- Real-cluster round trip with `scripts/it` containers: ES 7/8 -> OpenSearch 2/3 with `--lifecycle drop`, a few hundred indices at `--parallel` 4-8, zstd and gzip, and a mid-run Ctrl-C then `--resume`.
- Load an `oslogs.py backup` directory and dump with searchdump-multi, then restore with `oslogs.py` (cross-tool compatibility).
- Image check: `searchdump-multi` runs inside each image and finds `/usr/local/bin/searchdump`.
- Real-cluster run (not possible on Grok Bot's computer: no podman/docker and about 3 GB of free RAM). Needs a single-node Elasticsearch 8.x and OpenSearch 2.x (1 GB heap each, security on, self-signed TLS) on the build machine, for example `podman run --network=host` with `discovery.type=single-node`. Then:
  - Dump about 20 indices (some with aliases, one with custom analysis, one empty, one hidden) plus a data stream with its component and index templates, an ILM policy, a legacy template and a stored script, with `--matchType index,alias,datastream --types=all --fileSize=50mb --parallel=4 --interval=200`.
  - Load into a fresh Elasticsearch: counts match, templates and policy present, `GET _data_stream/NAME` shows the stream with its documents, analyzer works (`_analyze`).
  - Load into OpenSearch with `--lifecycle drop` and with `--types=policy --lifecycle=convert`: the ISM policy is created and index templates load without `index.lifecycle`.
  - Interrupt a split dump with Ctrl-C and rerun with `--resume`; kill the target mid-load and confirm the failed index is reported and the others finish.
  - Large index (several GB) with `--fileSize`: part sizes near the limit and memory flat.

## OpenDAL storage path (added 2026-11-11, L-809a)

- **Real Backblaze B2** (needs the owner's account; B2 is not allowed without approval): `B2_APPLICATION_KEY_ID= B2_APPLICATION_KEY= B2_BUCKET= B2_BUCKET_ID= scripts/opendal-tests/b2-real.sh` (values from the environment only, never in the repo). It covers a gzip round trip, split parts, refusal of an existing object, a wrong key failing without printing it, and searchdump-multi copy to and from B2. Only parsing and settings are unit tested today; no B2 mock exists that OpenDAL can use.
- **Real AWS S3** (needs owner approval and a named sandbox account, as B13): SigV4 against AWS itself, virtual-host style, `AWS_SESSION_TOKEN`, a bucket in a non-default region, SSE-C, and a large (multi-GB) multipart upload with `--fileSize`.
- Break tests: kill the S3/SFTP server mid-upload (retries, abort of the multipart upload, partial split sets), slow links with `tc`, and a full disk on the SFTP server.
- searchdump-multi reads and writes storage URLs through a local staging directory; staging space equals the dump size. Direct streaming without staging is not built.
- Note: the shared rootless podman store on the Grok Bot box has damaged layers for the OpenSearch and Elasticsearch images (not repaired, since other agents use it); `cross.sh` used an isolated store via `ODT_OS_PODMAN_ARGS="--root ~/.local/share/edrs-odt-podman --runroot /tmp/edrs-odt-podman/run"`.
