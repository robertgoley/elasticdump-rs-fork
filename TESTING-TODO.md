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
| Test | What it checks |
|---|---|
| Networked backup and restore with compression at scale | Tool and cluster on **separate machines** over a real network, with bandwidth and latency measured first (for example 1 Gbit and 10 Gbit links, iperf3 and ping); a multi-node cluster; 10M+ docs and multi-GB files. Dump (scroll unsliced, scroll with slices = shards, PIT with 4/8/16 slices) and restore (`--concurrency` 1/4/8/16) with `--compressionType` none, gzip levels 1 and 6, and zstd levels 1, 3 and 9. Record throughput in MB/s and docs/s, client CPU-s and server CPU per node, and network utilisation, and find out whether compression pays off once the network, not the CPU or the cluster, is the bottleneck. Baseline: the box results in [`results/local/L-805.md`](results/local/L-805.md) (compression levels) and [`results/local/L-807.md`](results/local/L-807.md) (concurrency curves). Those ran tool and engine on one 8-vCPU machine over loopback, with no real network, so they measure codec CPU and engine limits, not transfer cost. |

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

## 2. Foreign or malicious code detection (project and dependencies)

Builds on what is already done: cargo vet and deny, the `unsafe` inventory, SBOM diffs, secret scans, the hostile-code audit of upstream code, egress-denied runs (L-701), syscall tracing (L-702) and the reproducible-build check (L-704).

### Own code and history
| Test | What it checks |
|---|---|
| Commit provenance audit | Author, committer and signature of every commit; flag any not made by the owner or the owner's agents. Enable signed commits and branch protection on `main` going forward. |
| Upstream line-origin review | `git blame` surviving upstream lines back to their original authors; focused manual review of any that touch networking, file paths, credentials, process spawning or `build.rs`. |
| Hidden-code scans | Bidirectional Unicode and invisible characters (Trojan Source), look-alike identifiers, long base64 or hex blobs, and tests or goldens that quietly assert insecure behaviour; a Semgrep ruleset for Rust covers most of this. |
| Mutation testing of security guards | `cargo-mutants` on credential redaction, the injection guards and TLS, proving the tests catch any weakening. |

### Dependencies
| Test | What it checks |
|---|---|
| Published crate vs tagged source | Diff each vendored `.crate` against its tagged repository; malicious code often exists only in the published package (as in the xz backdoor). |
| Build scripts and proc macros | Inventory every crate with a `build.rs` or proc macro (code that runs at build time), review each, and build offline in a sandbox that only allows writes under `target/`. |
| Maintainer and typosquat review | `cargo-supply-chain` owner lists: recent owner changes, new maintainers, odd release timing, and names one letter off a popular crate. |
| Full cargo vet coverage | Replace exemptions with real audits or imported audits (Mozilla, Google, Bytecode Alliance); diff-audit every version bump, including the open Dependabot branches. |
| Vendored, pinned builds | Build only from vendored sources with checksums, never fresh registry fetches; fail the build on an unexpected `Cargo.lock` change. |
| C code verification | Check bundled libzstd and any other C sources byte for byte against signed upstream release tarballs. |

### Binary behaviour
| Test | What it checks |
|---|---|
| Binary inspection | Strings, symbols and linked libraries of the release binary for unexpected URLs, IPs, domains, shell commands or crypto. |
| Wider runtime monitoring | Egress and syscall tests across every CLI option; eBPF tracing of all network, file and process activity (extends B12); runs with the clock moved months ahead and unusual environment variables to catch time- or environment-triggered behaviour. |
| Container image checks | Layer-by-layer scan of built images for unexpected files (Syft, Grype, Trivy) and comparison against the dhi.io base image digests. |
| Independent rebuild and signing | Rebuild on a second machine (B11), match hashes, then sign releases with cosign and provenance attestations. |

### Tooling and CI
| Test | What it checks |
|---|---|
| Workflow hardening review | Every GitHub Actions step pinned to a full commit SHA and checked with zizmor; least-privilege tokens; no `pull_request_target`. |
| Toolchain integrity | Checksums of the Rust toolchain, cargo plugins, gitleaks and the scanners themselves, since a compromised tool can hide anything. |

## 3. Tests that come with unbuilt features

Each feature in [FEATURES-TODO.md](FEATURES-TODO.md) needs its own contract, interop and break tests when it is built (multielasticdump, AWS SigV4 and Serverless, S3/MinIO, CSV, SOCKS5, `--delete`, transforms). AWS tests (B13) stay blocked until the owner approves a named sandbox account.
