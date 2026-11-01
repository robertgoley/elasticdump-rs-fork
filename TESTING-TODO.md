# Testing to do

Planned work not started yet, plus further tests that were suggested but are not in the plan. Status as of `main` at L-705 (2026-10-31). Completed work is in [TESTING-COMPLETED.md](TESTING-COMPLETED.md); the next items in line are in [TESTING-IN-PROGRESS.md](TESTING-IN-PROGRESS.md). Requirement IDs refer to [`project/plan/TEST-REQUIREMENTS.md`](project/plan/TEST-REQUIREMENTS.md); the agent roles (impl, runner, review, lead) are in [`project/handoff/HANDOFF-GROK-OPENCODE.md`](project/handoff/HANDOFF-GROK-OPENCODE.md).

## 1. Runtime supply chain (remaining)

| Item | Req | Who | Note |
|---|---|---|---|
| L-703 | SUP-003 | lead | seccomp/Landlock profile (security boundary) |
| H4 | gate | lead | cut bundle #4 (§9) once L-701..704 merge → unblocks B11, B12 |

## 2. Load and data generation (A8, L-801 to L-817)

Scale: L0 100k docs and L1-lite 1M docs, disk at most 12 GB.

| Item | Req | Who | Note |
|---|---|---|---|
| L-801 | LOAD-004 | runner | scenario matrix cells via `scripts/load/run.sh L0/L1-lite`; new scenario TOMLs are mechanical |
| L-802 | LOAD-005 | impl/runner → lead | flat-memory runs; lead updates the README memory formula if wrong |
| L-803 | LOAD-006 | impl/runner → lead | kill/resume runs; the documented recipe is owner Q4-dependent |
| L-804 | LOAD-007 | impl/runner → lead | fault injection under load; failures classified by lead |
| L-805 | LOAD-008 | runner | compression levels table |
| L-806 | LOAD-009 | runner | splitting at scale |
| L-807 | LOAD-010 | runner | concurrency curves (noisy here; B10 is authoritative) |
| L-808 | LOAD-011 | runner | Node comparison at 100k/1M (Node in /tmp/ed) |
| L-809 | LOAD-014 | runner | 300 indices campaign |
| L-810 | LOAD-015 | impl/runner → lead | large docs; 413/limit wording is lead's |
| L-811 | LOAD-017 | impl | leak check in teardown of each run (script change in scripts/load) |
| L-812 | LOAD-018 | runner | data streams 1M |
| L-813 | LOAD-020 | lead | trend store / regression gate design (CI) |
| L-814 | DATA-002 | impl | edgen mapping/settings per engine (single crate tools/edgen) |
| L-815 | DATA-003 | impl/runner → lead | edge catalogue `--profile edge` (tools/edgen/src/edge.rs exists; check first); expectations reviewed by lead |
| L-816 | DATA-007 | impl | OSB generate-data cross-check |
| L-817 | DATA-008 | impl | corpus generator with `--seed` |
| Final | gate | lead | bundle after A8 → B2 (full), B10 |

## 3. Big-host campaigns (B0 to B13)

| Item | Req | Who | Depends | Note |
|---|---|---|---|---|
| B0 | — | runner | H0 ✓ | `cards/T-B0` |
| B1 | DATA-004/005/010 | runner → lead | H0/H1 ✓ | `cards/T-B1`; lead pins datasets.toml |
| B2 | LOAD-004…012, 014 | runner (cells) / lead (matrix design) | H1 ✓, A8 preferred | start with `cards/T-B2a`, then one card per scenario × engine |
| B3 | LOAD-005/008/009 at L2 | runner | B2 | big-host campaign; big5-100 / full http_logs need lead to add manifest entries |
| B4 | LOAD-013 | runner | B2, H3 ✓ | two clusters at once: only if memory allows |
| B5 | LOAD-016, DATA-009 | runner | B3 | 6–12 h soak; `SOAK_RUNNER` var (owner item) |
| B6 | LOAD-019, DATA-006 | impl/runner → lead | B1 | Rally/OSB baselines need owner Q8 (Rally/OSB) answered |
| B7 | API-084, INJ-015, SUP-020/022 | runner → lead | H2 ✓ | `cards/T-B7` |
| B8 | BRK-NET-009, BRK-SRV-012 | lead (design) → impl | H1 ✓ | multi-node compose and scenarios are new harness code |
| B9 | API-070, API-072 | runner | H3 ✓ | `scripts/it/xmatrix.sh all`, `xpairs.sh`, serialised |
| B10 | LOAD-010/011 | runner | B2 | dedicated cores, 3 runs each |
| B11 | SUP-010 | impl/runner → lead | H4 | second machine/VM needed |
| B12 | SUP-002/003 | lead | H4, B2 | root eBPF: optional, ask the owner |
| B13 | GAPS §3/§4 | O | L-A04/A05 + owner approval | **blocked**: no AWS without owner approval and a named sandbox account |

## 4. Further tests not yet in the plan

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
| Release integrity | cosign signing and provenance attestations for binaries and images; review open Dependabot branches before any merge (dependencies are pinned on purpose). |

## 5. Tests that come with unbuilt features

Each feature in [FEATURES-TODO.md](FEATURES-TODO.md) needs its own contract, interop and break tests when it is built (multielasticdump, AWS SigV4 and Serverless, S3/MinIO, CSV, SOCKS5, `--delete`, transforms). AWS tests (B13) stay blocked until the owner approves a named sandbox account.
