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

### Malicious or broken input
| Test | What it checks |
|---|---|
| Hostile cluster proxy | A proxy that mutates every response: gzip and zstd decompression bombs in `Content-Encoding`, endless chunked bodies, huge headers, redirects to other hosts or `file://`; credentials are never sent across a redirect. |
| Header and request injection | CR/LF in `--headers` and `--params`, and request smuggling attempts through a reverse proxy. |
| Secret exposure on the host | Whether passwords passed as flags show in `/proc/<pid>/cmdline` or `ps`, core dumps or temp files; decide whether to warn or offer an env-only mode. |
| Sanitizer and Miri runs | Miri on code next to `unsafe`, ThreadSanitizer and UndefinedBehaviorSanitizer runs of the test suite (only AddressSanitizer via fuzzing so far). |
| Mutation testing (`cargo-mutants`) | Deliberately breaks the code to measure whether the tests notice; the main check on AI-written test strength. |
| Differential fuzzing against Node | Same generated clusters and files through both tools, outputs diffed automatically, beyond the fixed interop corpus. |
| Release integrity | cosign signing and provenance attestations for binaries and images. |
| Dependency update review | Six open Dependabot branches (base64, pkcs8, rand, rand_chacha, sha2, GitHub Actions): run the full gate, cargo vet and deny on each before any merge; dependencies are pinned on purpose. |

## 2. Tests that come with unbuilt features

Each feature in [FEATURES-TODO.md](FEATURES-TODO.md) needs its own contract, interop and break tests when it is built (multielasticdump, AWS SigV4 and Serverless, S3/MinIO, CSV, SOCKS5, `--delete`, transforms). AWS tests (B13) stay blocked until the owner approves a named sandbox account.
