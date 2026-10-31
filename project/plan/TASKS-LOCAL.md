# elasticdump-rs: Local Task List (work that fits this environment)

> Written 2026-10-29. The task list was generated from `TEST-REQUIREMENTS.md` (204 requirements, MT-0…MT-8, findings INJ-F1…F4, F-01…F-14, H-01…H-10), `GAPS.md`, `PLAN.md` and `TYPES-PLAN.md`, plus the repo at `rewrite` HEAD `958e9d5`.
> Companion document: `HANDOFF-TASKPLAN.md` (B work for a separate, larger machine).
> This environment is called **A**; the outside agent is **B**.
> Every requirement is either **A** (fully here), **A+B** (A does the code and a reduced run; B does the full-scale run) or **B** (only there).

## 1. Measured environment (2026-10-29 12:30 EDT)

| Resource | Measured | Consequence |
|---|---|---|
| CPU | `nproc` = 8 (Intel Xeon, virtualised), **shared** with other agents and the process building in `/workspace/elasticdump-rs` | `CARGO_BUILD_JOBS=4`; fuzz with 1–2 workers; perf numbers are noisy (no authoritative scaling curves here) |
| RAM | 16,002 MB total, **no swap**; `MemAvailable` 12.9–13.2 GB during the measurement; other agents sometimes run a ~12.5 GB VM, which leaves **~3 GB** | Every task has a memory budget (§2). Heavy tasks start only after a `MemAvailable` check |
| Disk | `/` (= `/workspace`, `/tmp`, `/home/box`) 128 GB, **35 GB free**, shared; 56 images (~20 GB) already in podman storage | Budget **≤ 15 GB** for our datasets and indices; no 100 GB corpora |
| Containers | podman (rootless, cgroup v2, `cgroupfs` manager, netavark); the other process was running `it-os-plain-*` at measurement time | Use name prefix `edl-` and ports 19200–19299 to avoid clashes |
| Container limits | **`podman run --memory/--cpus` are NOT enforced** (inside the container `memory.max` = `max`, `cpu.max` = `max`; a 32 MB container allocated 200 MB) | Cap ES with `ES_JAVA_OPTS`/`OPENSEARCH_JAVA_OPTS` heaps. Cap the tool with a root-made cgroup (next row) |
| Root / cgroups | passwordless `sudo` works. A root-created cgroup `/sys/fs/cgroup/<x>` with `memory.max=64M` **OOM-killed** a 200 MB allocation (verified; probe removed) | BRK-RES-001 low-memory tests are possible here via `scripts/res/cg-run.sh` (L-003) |
| Network isolation | `--network none` and a `podman network create --internal` network both block egress (verified; test network removed) | SUP-001 egress-denied runs are possible here |
| Tracing | no `strace`/`perf`/`bpftool` on the host. `apt` over HTTP fails (proxy returns 500); HTTPS works. `strace` works **inside a container** with `--cap-add SYS_PTRACE` (`apk add strace`, verified). As root, tracefs has 712 syscall events, the bpf fs is mounted, LSMs = `capability,landlock,selinux,bpf`, `perf_event_paranoid` = 2 | SUP-002 (strace) and SUP-003 (seccomp/Landlock) are possible here. eBPF is untested, so it is optional in B |
| Outbound bandwidth | `rally-tracks.elastic.co` 49.5 MB in 11.4 s ≈ **4.3 MB/s**; CloudFront 117 MB in 15 s ≈ **7.8 MB/s** | 1–2 GB corpora are fine (minutes). big5-100 (7.3 GB compressed, ~100 GB raw) would take ~20 min to download but **cannot be unpacked or indexed here** |
| Toolchain | rustc/cargo 1.85.1 only (rustup has no nightly); Node at `/usr/bin/node` with elasticdump in `/tmp/ed`; local images for all 9 engine versions (OS 1.3.20/2/3, ES 7.10.2 OSS+default, 7.17.29, 8.6.1, 8.19.23, 9.1.2) | Install nightly (cargo-fuzz, udeps) and cargo tools in L-002 |
| Interruptions | the agent can be stopped at any time | One commit per task; long runs keep checkpoints and can be resumed (§2.3) |

## 2. Working rules for A

### 2.1 Memory and disk budgets (the "Mem" column)

| Class | Peak budget | Start only if | Typical tasks |
|---|---|---|---|
| **U** | ≤ 3 GB (cargo build/test, `CARGO_BUILD_JOBS=4`, no containers) | `MemAvailable` ≥ 4 GB | code fixes, unit, mock/contract, static supply-chain tasks |
| **U2** | ≤ 4 GB (two release builds in sequence) | ≥ 5 GB | reproducible-build check |
| **F** | ≤ 3 GB (`cargo fuzz run -j1 -- -rss_limit_mb=2048 -max_total_time=600`) | ≥ 4 GB | fuzz and proptest smoke runs |
| **N1** | ≤ 4 GB: one engine container (heap 1 GB; 512 MB for ES 7.10/OS 1.3) plus tool and Node | ≥ 6 GB | live per-version suites, run **one image at a time** |
| **N2** | ≤ 6 GB: two engines at 512 MB heaps | ≥ 9 GB (the 12.5 GB VM is not running) | cluster→cluster pairs |
| **L1L** | ≤ 6 GB RAM, ≤ 12 GB disk: one engine with a 2 GB heap, 1M docs ("L1-lite") | ≥ 9 GB RAM and ≥ 20 GB free disk | load tests at L0 (100k) and L1-lite (1M) |

`scripts/res/memguard.sh <GB>` waits up to 30 min for enough memory, otherwise exits 75 (the task is retried later).

Heavy jobs (N1/N2/L1L) take `flock /tmp/edrs-heavy.lock` so two agents never run engine containers at the same time. Do not run them while the other process's IT suite is running: check with `podman ps --filter name=it-`.

### 2.2 Where to work
- `/workspace/elasticdump-rs` is **in use by another process**.
- Until it is free, do A work in a **local clone**, which only reads from the source repo:
  - `git clone --branch rewrite /workspace/elasticdump-rs /workspace/edrs-work`
  - then `git -C /workspace/edrs-work remote set-url --push origin no-push-allowed`
  - work on a branch: `git switch -c local/tests`.
- Merge back with `git fetch /workspace/edrs-work local/tests` from the main repo once the owner agrees.
- Never push anywhere.

### 2.3 Resumability
- One commit per task: `L-<id>: <requirement IDs>: <summary>`.
- Each task writes `results/local/<L-id>.json` (same schema as `HANDOFF-TASKPLAN.md` §9) and adds a line to `results/local/PROGRESS.md` (task, status, commit, date).
- Long runs (fuzz, matrix, loads) are idempotent scripts that skip finished cells using per-cell result files. A rerun after an interruption continues where it stopped.
- Containers are named `edl-<task>-<cell>` and removed with `podman rm -f` at the start of each cell.

### 2.4 Effort scale
Sizes come from `TEST-REQUIREMENTS.md`:
- **S** ≈ 0.25 d, **M** ≈ 0.9 d, **L** ≈ 3 d (agent-days).
- This calibration makes all 204 requirements ≈ 23 engineer-weeks, matching §9 of the requirements.
- For **A+B** requirements, 60% of the effort is counted here.

## 3. Phase overview and order

| Phase | Contents | Tasks | Effort (agent-days) | Depends on | Hand-off gate after it |
|---|---|---|---|---|---|
| **A0 Setup** | work clone, toolchains, resource wrappers | 4 | 1.5 | — | — |
| **A1 Security fixes** (first) | INJ-F1 → F2 → F3 → F4, injection guards, supply-chain findings F-01…F-14 | 9 (15 reqs) | 6 | A0 | — |
| **A2 Static supply-chain gates** | cargo-deny/vet/geiger/machete, native-code allow-list, policies | 14 | 8 | A1 | — |
| **A3 Harness + B enablers** | nextest tiers, router mock, goldens, fault proxy, `edverify`, metrics, guards, `edgen`, dataset manifest/converters, load runner | 21 | 21 | A1 | **H1**: bundle #1 → B starts B1–B3, B6, B8 |
| **A4 Contracts + error paths + top-15 chaos** | API-002…072 per version (one image at a time), Node interop, export-once/import-everywhere, top-15 BRK | 56 | 36 | A3 | **H3**: bundle #3 → B4, B9 |
| **A5 Property and fuzz (short runs)** | proptests, 19 fuzz targets with 10-min smoke runs, sonic differential, hostile compression | 13 | 11 | A1, A3 (nightly toolchain from A0) | **H2**: bundle #2 → B7 (long campaigns) |
| **A6 Remaining chaos** | all remaining TST-BRK | 57 | 19 | A3 (fault proxy, router mock), A4 | — |
| **A7 Runtime supply-chain** | egress-denied runs, strace, seccomp/Landlock, same-machine repro build, RC diff | 5 | 3.5 | A2, A3 | **H4**: bundle #4 → B11, B12 |
| **A8 L0 and L1-lite loads** | 100k and 1M scenario runs, memory flatness, resume, faults, compression/splitting, Node comparison at 1M | 17 | 8 | A3 | **Final hand-off** |
| **A9 Hand-off packaging** | bundles, results merge-back | 2 | 0.5 + 0.5 per merge | each gate | — |
| **A10 Feature backlog (decision-gated)** | GAPS.md items and INJ-013; the owner decides what gets built | 11 groups | ~36 (not in the total) | owner decisions Q13 and GAPS | B13 (AWS) after G-04/G-05 |

**Total for A (A0–A9):** ≈ **114 agent-days ≈ 23 weeks** in a single stream.
- A static/unit stream (A2, A5, much of A6) can run beside one live stream (A4/A8), because their memory classes fit together (U + N1 ≤ 7 GB). With two streams this is ≈ **12–13 weeks**.
- When the 12.5 GB VM is running (~3 GB free), only U/F tasks can proceed.

**Getting B started sooner:**
- Inside A3, do the B enablers first: INF-005 `edverify`, INF-006 metrics, INF-007 guards, INF-009/DATA-001 `edgen`, DATA-004/005/010 and LOAD-001…003. Leave the router mock and goldens until after them.
- Cut **H1** as soon as those and A1 are done. That is ≈ 15 agent-days (~3 weeks) after the start, rather than ~6 weeks for all of A3.
- B can do B0 and the B1 downloads on H0 straight away.

**A critical path:** A0 → L-101 (INJ-F1) → L-102 (INJ-F2) → rest of A1 → A3 (INF-002 router mock, INF-005 `edverify`, INF-009/DATA-001 `edgen`, LOAD-001 runner) → **H1** → A4 → A6 → A8.

## 4. Phase A0: Setup

| Task | What | Deps | Effort | Mem | Acceptance |
|---|---|---|---|---|---|
| L-000 | `scripts/env-check.sh`: prints nproc, MemAvailable, free disk, podman ps/images, and whether the other process's `it-*` containers are running; writes `results/local/env.json` | — | 0.1 d | U | Output matches §1 |
| L-001 | Local clone `/workspace/edrs-work` (§2.2), branch `local/tests`, push URL disabled, `CARGO_TARGET_DIR` inside the clone | — | 0.1 d | U | `git remote -v` shows the push URL `no-push-allowed`; `cargo test --lib` passes (first build ~2–3 GB RAM) |
| L-002 | Toolchains and tools: `rustup toolchain install nightly --profile minimal`; `cargo install --locked` cargo-nextest, cargo-deny, cargo-vet, cargo-fuzz, cargo-machete, cargo-geiger, cargo-cyclonedx, cargo-udeps (one at a time); actionlint and zizmor (via pip/https) for workflow linting; toxiproxy container image; `alpine` + `apk add strace` image `edl-strace`. Host `apt` over HTTP is broken: do not change apt sources without the owner's OK | L-001 | 1 d | U | All tools report versions in `results/local/tools.json` |
| L-003 | Resource wrappers: `scripts/res/memguard.sh`, `scripts/res/cg-run.sh <mem> <cmd>` (sudo cgroup v2 child with `memory.max`, `memory.swap.max=0` and `pids.max`; removed on exit), `scripts/res/heavy.sh` (flock), and a heap table per image | L-001 | 0.3 d | U | `cg-run.sh 64M python3 -c 'bytearray(200<<20)'` is OOM-killed and leaves no stray cgroup behind |

## 5. Phase A1: Security fixes (do first; INJ-F1 first)

| Task | Fix and tests | Requirement IDs | Deps | Effort | Mem | Acceptance |
|---|---|---|---|---|---|---|
| **L-101** | **INJ-F1 (High):** in `retrieval/extract.rs:231` and `pipeline/sink.rs::source_only_lines`, pass raw hits through `json_split::single_line` when `memchr2(b'\n', b'\r')` matches. `BulkSink::process_batch`/`Top::parse` reject any line that is not exactly one complete JSON object (no `filter_map(ok)` partial members). Count and report rejected lines; exit code is non-zero. Live PoC on ES 7.17.29 and OS 2.x, run one at a time | INJ-001, INJ-002, INJ-003, INJ-010 | L-001, L-003 | 1.5 d | N1 | The §8.1 PoC: `victim` unchanged; `x` keeps its source byte for byte; dump lines == hits; `--fileSize` parts never split a record; `--sourceOnly` is single-line |
| **L-102** | **INJ-F2 (Medium):** `validate_index_name` on every name from a file or server (data `_index`, meta keys for index/settings/mapping/analyzer/alias), renamed or not; reject `* ? , _all . ..`, empty names and a leading `-_+`. `EsClient::url` returns an error on `.`, `..` and empty segments. `close_put_open` only takes a concrete single index | INJ-006, INJ-007, INJ-016 | L-101 (shared test files) | 1 d | N1 | MockEs records **no** request for the PoC files; `POST /*/_close` is never sent (guard tested live on ES 7.17 and OS 2.x); URL segment proptest passes |
| L-103 | **INJ-F3 (Low):** type-check spliced `_id`/`_routing`/`_parent` (string; numbers converted) and `version` (u64) in `encode_doc`; reject bad values per document | INJ-005 | L-101 | 0.5 d | U | One bad doc never causes a 400 for the whole bulk request (MockEs) |
| L-104 | **INJ-F4 (Low):** `--parseExtraFields` names must match `^[a-z][a-z0-9_]*$`, are de-duplicated, and are written with `push_json_str` | (INJ-004 later) | L-101 | 0.25 d | U | Usage error for `x":1,"_index":"o`; unit test |
| L-105 | Injection guards: duplicate meta keys (default **reject**, pending Q13), raw `_source` must be exactly one object, header CR/LF/NUL usage error, query-encoding goldens | INJ-008, INJ-009, INJ-011, INJ-012 | L-101 | 1 d | U | Per-requirement acceptance in TEST-REQUIREMENTS §8.3 |
| L-106 | Secret handling: F-05 (`--debug` URL incl. `--params` → redact), F-06 (`redacted_endpoint` never returns raw on parse failure), F-08 (manual `Debug` for structs holding passwords), F-07 (warn when env proxies are used) | SUP-018 (part) | L-001 | 0.5 d | U | Canary password never appears in stdout/stderr/`--debug` output (the full CLI-007 canary test comes in A4) |
| L-107 | CI hardening: F-01 (remove `curl \| sh`; download pinned release assets and check SHA-256), F-02 (pin actions to commit SHAs), F-03 (`permissions: contents: read` in every workflow), F-09 (dispatch input via `env:`), F-10 (`cargo install --locked --version`) | SUP-013, SUP-018 (part) | L-002 | 0.5 d | U | `actionlint` and `zizmor` are clean. Workflows **cannot be run here** (local fork, no push), so B or the owner verifies them on a CI runner |
| L-108 | Repo hygiene: F-04 track `tests/fixtures/node/stored_scripts.jsonl` (`.gitignore` exception); F-11 tests write only to tempdirs and the stray `test_query_*.json` and `oom` files are removed; F-12 compose binds ports to `127.0.0.1` and turns security on; F-13 IT passwords are random and stored 0600 outside `/tmp`; F-14 images pinned by digest | INF-014, SUP-018 (part) | L-001 | 0.75 d | U | `git status --ignored` is clean after `cargo test`; fixtures are tracked |
| L-109 | SUP-018 close-out: one regression test or lint per F-01…F-14; update `SECURITY-CVE.md` | SUP-018 | L-106…L-108 | 0.4 d | U | Table F-01…F-14 → test/lint names |

## 6. Phase A2: Static supply-chain gates

Phase dependency: A1.

| Task | Req | Pri | Class | Title | Deps | Effort | Mem | Acceptance (short; full text in TEST-REQUIREMENTS) |
|---|---|---|---|---|---|---|---|---|
| L-201 | SUP-004 | must | A | cargo-vet | L-002 | L | U | `cargo vet` green in PR CI |
| L-202 | SUP-005 | must | A | cargo-deny tightening | — | S | U | `cargo deny check` green; adding a build-script crate fails until allow-listed |
| L-203 | SUP-006 | must | A | Unsafe inventory | — | S | U | Baseline committed; diff gate works |
| L-204 | SUP-007 | should | A | Unused dependencies and features | — | S | U | 0 unused deps; feature list documented |
| L-205 | SUP-008 | should | A | Dead and unreachable code | — | S | U | Uncovered-function report reviewed per release |
| L-206 | SUP-009 | must | A | Build-time behaviour review | — | M | U | CI release build runs under `--network none`; the inventory diff gate fires on new build scripts or proc macros |
| L-207 | SUP-011 | must | A | SBOM diff per PR | L-002 | M | U | Diff artefact on every PR; label gate |
| L-208 | SUP-012 | must | A | Hostile-code review of upstream-origin code | — | M | U | Review record exists; CODEOWNERS rule active |
| L-209 | SUP-014 | must | A | New-dependency policy | — | S | U | Policy doc; bot config; CODEOWNERS enforced |
| L-210 | SUP-015 | should | A | Secret scanning and canaries | — | S | U | 0 findings; canary job green |
| L-211 | SUP-017 | should | A | Image hardening tests | — | S | N1 | `make test-image` extended |
| L-212 | SUP-019 | could | A | Publisher/ownership monitoring: `cargo supply-chain publishers` snapshot diffed weekly; al | — | S | U | Weekly report |
| L-213 | SUP-021 | should | A | Native-code allow-list | — | S | U | Adding e.g. `libz-sys` fails CI |
| L-214 | SUP-023 | could | A | Trim non-hot unsafe | — | S | U | geiger total drops; all tests pass |

## 7. Phase A3: Harness and B enablers (gate H1)

These tasks also unblock B. After this phase, create bundle #1 (A9) so B can start the 10M/100 GB runs.

Phase dependency: A1.

| Task | Req | Pri | Class | Title | Deps | Effort | Mem | Acceptance (short; full text in TEST-REQUIREMENTS) |
|---|---|---|---|---|---|---|---|---|
| L-301 | INF-001 | must | A | Introduce tier selection with `cargo-nextest` profiles (`unit`, `contract`, `it`, `load`, | — | M | U | `cargo nextest run --profile <tier>` runs only that tier. JUnit XML is produced. Per-test slow-timeout is set (T0 30 s, T2 5 min, load per scenario) |
| L-302 | INF-002 | must | A | Replace the sequential `MockEs` with a router mock: match on method + path + query + b | — | L | U | Existing 22 contract tests pass unchanged on top of it. Each fault mode has a self-test |
| L-303 | INF-003 | must | A | Record/replay golden infrastructure | INF-002 | L | U | Re-recording an unchanged version gives a zero diff after normalisation. Replay runs offline in T0/T1 |
| L-304 | INF-004 | must | A | Promote the in-process fault proxy (`tests/it/concurrency.rs::fault_proxy`) to `tests/supp | — | M | U | Both proxy kinds pass the same fault conformance tests |
| L-305 | INF-005 | must | A | Document-set verifier | — | M | U | Detects 1 changed byte, 1 missing doc and 1 duplicate doc in 100M docs. Runs at ≥ 200k docs/s on the box. Reports the first 10 differing `_id`s by re-scanning the mismatched shard |
| L-306 | INF-006 | must | A | Metrics wrapper | — | M | U | Emits `results/<run-id>.json` against a JSON schema. A comparison script flags regressions above a threshold |
| L-307 | INF-007 | must | A | Resource guards | — | S | U | A run on a box with too little memory SKIPs with a reason and exits 0 (or 1 when `*_REQUIRED=1`). Two parallel `run.sh` invocations serialise |
| L-308 | INF-008 | should | A | Dataset cache manager | DATA-004 | M | U | A second fetch is a no-op. A checksum mismatch fails. `--max-docs N` takes a prefix without decompressing the whole file |
| L-309 | INF-009 | must | A | Seeded generator `edgen` | — | L | U | see TST-DATA-001..009 |
| L-310 | INF-010 | should | A | CI layout: PR = T0, T1, T4 plus a T2 subset (OS 2.19 plain+secure, ES 9.1 plain, ES 7.10 O | INF-001 | M | U | All jobs green on `rewrite`. A nightly failure opens no ticket automatically (Robert's call) but leaves artefacts |
| L-311 | INF-011 | must | A | Create the missing `TESTING-MATRIX.md`, generated from `IT-SUMMARY` lines, the golden fixt | INF-001 | S | U | The README link resolves. Every image is listed with its digest. The cross-version expectation table (TST-API-071) is embedded |
| L-312 | INF-012 | should | A | Pin container images by digest in `scripts/it/up.sh` / `run.sh` (tags today) and record th | — | S | U | A re-pushed tag cannot change what CI runs without a diff |
| L-313 | INF-013 | should | A | Flake policy: correctness tests never auto-retry. A quarantine list in `nextest.toml` need | — | S | U | A quarantined test older than 14 days fails CI |
| L-314 | LOAD-001 | must | A | Load runner `scripts/load/run.sh <tier> <scenario>`: provisions the node with caps, seeds | INF-005, INF-006, INF-007, INF-009 | M | L1L | One command per tier; results JSON per scenario; seeds are recorded |
| L-315 | LOAD-002 | must | A | Correctness on every load run | INF-005 | S | L1L | Any mismatch fails and names the first 10 differing ids |
| L-316 | LOAD-003 | must | A | Metrics per run | INF-006 | S | L1L | All fields present in results JSON; schema-validated |
| L-317 | LOAD-012 | must | A | Resource limits by place | — | S | L1L | The runner refuses tiers that do not fit (INF-007) |
| L-318 | DATA-001 | must | A | `edgen` with counter-based seeding, profiles `logs`, `nested`, `geo`, `wide`, `huge`, `edg | INF-009 | L | U | Same seed gives the same bytes across runs and thread counts (CI hash check); ≥ 300k docs/s per core (`logs`) |
| L-319 | DATA-004 | must | A | Dataset manifest `datasets.toml` with URL, size, SHA-256, licence and tier for every §5.1 | — | S | U | `fetch.sh --verify` passes; the licence gate fails on a disallowed licence |
| L-320 | DATA-005 | must | A | Converters: Rally/OSB NDJSON, CLF/combined, Loghub raw → `elasticdump` format with determi | DATA-004 | M | L1L | Same input gives the same output hash |
| L-321 | DATA-010 | must | A | Deterministic `_id` policy for every external dataset (`hash(dataset, file, line)`) so dum | — | S | U | Same id on every conversion |

## 8. Phase A4: Contracts, error paths and top-15 chaos (gate H3)

Live suites run **one image at a time** (N1). Order: OS 2.19 → ES 8.19 → ES 7.17 → OS 3 → ES 9.1 → ES 7.10 OSS → ES 7.10 default → OS 1.3 → ES 8.6. Each image is one resumable cell. Top-15 BRK items are listed here because MT-2 runs them first.

Phase dependency: A3 (INF-002/003/004).

| Task | Req | Pri | Class | Title | Deps | Effort | Mem | Acceptance (short; full text in TEST-REQUIREMENTS) |
|---|---|---|---|---|---|---|---|---|
| L-401 | API-001 | must | A | Machine-readable endpoint inventory (`tests/golden/endpoints.toml`, rows E1–E20 plus the r | — | S | U | Any new endpoint in a PR fails the guard until the inventory and goldens are updated |
| L-402 | API-002 | must | A | Live per-version contract suite | — | L | N1 | One green row per (version, endpoint) in TESTING-MATRIX.md. "Not applicable" (e.g. PIT on OS 1.3) is asserted as the specific expected failure, not skipped |
| L-403 | API-003 | must | A | Golden replay | API-002, INF-003 | L | N1 | Runs offline in < 2 min. A change in request shape or output bytes shows a reviewable diff |
| L-404 | API-004 | must | A | Golden refresh and drift job: nightly `IT_RECORD=1` against all images, then a normalised | API-003 | S | N1 | Unchanged versions produce a zero diff; a drift report is an artefact |
| L-405 | API-010 | must | A | Scroll: initial/continuation/clear on every version; sliced scroll (`slices` = shards, > s | — | M | N1 | Digest equal to source; no open contexts left (`_nodes/stats/indices/search.scroll_current == 0`) |
| L-406 | API-011 | must | A | PIT: open/close request shape per engine; `pit_id` rotation each page; tiebreaker per vers | — | M | N1 | Digest equal for unsliced and slices ∈ {2, 4, 16}; no open PITs afterwards (`point_in_time_current == 0` where the stat exists) |
| L-407 | API-012 | must | A | `search_after` without PIT: sort required (usage error otherwise); ties on sort values wit | — | S | N1 | Documents a known limitation with an asserted count, or digest equal if the tool adds an `_id` tiebreaker |
| L-408 | API-013 | must | A | `hits.total` variants: int (`rest_total_hits_as_int=true`, ES 6 goldens), `{value, relatio | — | S | N1 | Progress estimate and final reconciliation (BRK-SRV-009) behave correctly in each case |
| L-409 | API-014 | should | A | `_render/template` with inline and stored (`id`) templates on every version that supports | — | S | N1 | Same documents as the equivalent plain query |
| L-410 | API-015 | must | A | `searchBody` variants: `_source` includes/excludes, `stored_fields`, `docvalue_fields`, `f | — | M | N1 | Output matches the golden for each variant; meta extraction order matches Node (`X`, `_X`, `fields.X`, `fields._X`) |
| L-411 | API-020 | must | A | `_bulk` request shape per version and action (`index`, `create`, `update`, `delete`) with | — | M | N1 | Byte-exact NDJSON goldens; every version accepts the gzip body (or the failure is documented per version) |
| L-412 | API-021 | must | A | `_bulk` response parsing per version: `errors:false` fast path; mixed items; `error.type` | — | M | N1 | `Total Writes`/`Failed Writes` equal the server's truth (cross-checked by `_count`) |
| L-413 | API-022 | should | A | Refresh behaviour: per-batch `_refresh` unless `--noRefresh`; refresh failure is only a wa | — | S | N1 | Asserted request counts; visible-doc timing documented |
| L-414 | API-030 | must | A | Mappings: typeless on 7+/OpenSearch; typed 6.x file unwrap; field literally named `propert | — | M | N1 | Same-engine round trips are lossless; cross-engine either converts with a named warning or fails with a clear error (expectation table, API-071) |
| L-415 | API-031 | must | A | Settings: generated keys stripped (`uuid`, `creation_date`, `version.*`, `provided_name`, | — | M | N1 | The restored `_settings` equal the source modulo an allow-list asserted per version pair |
| L-416 | API-032 | must | A | `--type index` with hidden and system indices (`expand_wildcards`), closed indices, data-s | — | M | N1 | Expected inclusion list per version |
| L-417 | API-033 | must | A | Analyzer type: new index; existing index with error, `--skip-existing`, or `--closeIndex` | — | S | N1 | The index is never left closed (asserted after an injected failure) |
| L-418 | API-034 | must | A | Aliases: `is_write_index`, `is_hidden`, `filter`, `index_routing`/`search_routing`; atomic | — | S | N1 | Round trip equal per version |
| L-419 | API-035 | must | A | Legacy `_template` on all versions incl. Warning headers on ES ≥ 8 (logged once, not fatal | — | S | N1 | Golden + live |
| L-420 | API-036 | must | A | Component and index templates: `composed_of` ordering; `priority`; `data_stream`; per-vers | — | M | N1 | A new built-in in a future version is caught by the drift job |
| L-421 | API-037 | must | A | ILM (ES 7.10 default+, missing on OSS → clear error) and ISM (OS 1.3/2.19/3.x): paging pas | — | M | N1 | Round trip equal; 1,050-policy paging test |
| L-422 | API-038 | should | A | `_opendistro/_ism` vs `_plugins/_ism`: on OS 1.3 verify that policies created via the lega | — | S | N1 | Asserted on OS 1.3 |
| L-423 | API-039 | must | A | Scripts: `_cluster/state/metadata` with and without the `monitor` privilege (secure profil | — | S | N1 | A 403 gives a message naming the `monitor` privilege |
| L-424 | API-040 | must | A | Cluster info variants (golden): OpenSearch with `compatibility.override_main_response_vers | — | S | N1 | Engine detection is correct, or the run fails with a clear message. Silently falling back to "Elastic 5.0.0" is a failure |
| L-425 | API-041 | should | A | Deprecation `Warning` headers and `X-Elastic-Product` are tolerated and logged at most onc | — | S | N1 | Golden |
| L-426 | API-050 | must | A | 429 handling per request kind: initial search, scroll continuation (retry allowed only on | — | M | U | Exact attempt counts and delays under `tokio::time::pause` (mock) |
| L-427 | API-051 | must | A | 5xx (500/502/503/504) per request kind; scroll continuations are not retried on 5xx (b | — | S | U | Exit 1, staging removed, clear message |
| L-428 | API-052 | must | A | Partial results: `_shards.failed > 0`, `timed_out: true`, `_shards.skipped`, failures on o | — | S | U | Fails; nothing committed; the message names the shard reason |
| L-429 | API-053 | must | A | Version conflicts: `create` on existing ids; `--handleVersion --versionType external` with | — | S | U | Counts per item type are correct; exit code follows the ignore flags (Q3) |
| L-430 | API-054 | must | A | Auth failures: no credentials, wrong credentials, expired/locked user, missing index or cl | — | M | N1 | One clear message per case naming the endpoint and privilege; no secret in output (STD-ACC 5) |
| L-431 | API-055 | must | A | TLS failures: unknown CA, expired cert, not-yet-valid cert, hostname mismatch, IP SAN, cli | — | M | N1 | One clear message per case; exit 1; `--insecure` and `NODE_TLS_REJECT_UNAUTHORIZED=0` print a warning |
| L-432 | API-056 | should | A | Server error passthrough: `index_not_found`, `resource_already_exists`, `illegal_argument` | — | S | U | The message includes the server's `type` and `reason`, truncated to ≤ 2 KB |
| L-433 | API-060 | must | A | Node interop for every type | INF-014 | M | N1 | 22 directions × 2 engines green; fixtures tracked (INF-014) |
| L-434 | API-061 | must | A | Compression interop: Node gzip parts → Rust; Rust gzip parts → Node; Rust zstd → Node is r | — | S | N1 | Asserted per case |
| L-435 | API-062 | must | A | Split naming interop including the non-ASCII boundary difference (UTF-8 bytes vs UTF-16 un | — | S | N1 | Part names equal for ASCII; documented difference asserted for non-ASCII |
| L-436 | API-063 | must | A | Data file variants accepted: `--sourceOnly` files; `_type`; `fields`; `_routing`/`routing` | — | S | N1 | Each restores to the same digest |
| L-437 | API-064 | should | A | Node's `bigint*.json` fixtures and `--support-big-int` outputs round-trip byte for byte | — | S | N1 | Byte equal |
| L-438 | API-070 | must | A | Cross-version matrix, "export once, import everywhere" | — | L | N1 | Results match the expectation table (API-071) exactly |
| L-439 | API-071 | must | A | Committed expectation table | API-070 | M | N1 | A diff between actual and expected fails CI; changes need a review |
| L-440 | API-072 | should | A+B: two-node pairs at 256-512 MB heaps fit only when the 12.5 GB VM is not running; full set of 9 pairs in parallel is B | Live cluster-to-cluster pairs | API-070 | M | N2 | Digest equal for data; metadata per the expectation table |
| L-441 | BRK-NET-003 | must | A | Reset or timeout after the server applied a `_bulk` (cut the response, not the request | — | M | U | STD-ACC; Explicit ids: harmless. **Auto-id docs** (`--sourceOnly` dumps, files without `_id`): duplicates must not happen silently. `create`: 409s on retry must not count as failures. `update` with scripts: non-idempotent Extra: … |
| L-442 | BRK-NET-004 | must | A | Server stalls: accepts and never responds; or sends headers then stops; no `--timeout` | — | S | U | STD-ACC; A default idle/read timeout (e.g. 5 min, configurable), then a retry or a clear failure Extra: finishes or fails within the configured bound |
| L-443 | BRK-NET-006 | must | A | Stale keep-alive connection | — | M | U | STD-ACC; Requests that provably never reached the server (connection closed before any byte was written) are retried even for continuations Extra: dump through a proxy with a 5 s idle close + `--throttleInterval 6000` succeeds |
| L-444 | BRK-SRV-001 | must | A | 429 storm on bulk items | — | S | U | STD-ACC; Non-zero exit (or a distinct exit code) whenever documents were not written, unless the user opted in explicitly (Q3) Extra: exit code policy enforced by test |
| L-445 | BRK-SRV-006 | must | A | Huge responses | — | M | U | STD-ACC; Response size cap (configurable), clear error suggesting a lower `--limit` Extra: under a 512 MB cgroup: clear error, not OOM |
| L-446 | BRK-SRV-009 | must | A | Server reports `hits.total.value = N` (`relation: eq`) for a scroll or PIT but returns few | — | S | U | STD-ACC; For snapshot reads (scroll/PIT, no `--size`/`--offset`/`--searchBody` size), retrieved ≠ exact total is an **error** (or at least a loud warning) Extra: asserted with mock early-EOF |
| L-447 | BRK-IN-002 | must | A | Truncated zstd | — | S | U | STD-ACC; Error as above Extra: exit 1 |
| L-448 | BRK-IN-004 | must | A | Mixed or wrong types | — | S | U | STD-ACC; "this input looks like a --type mapping dump" for data input too Extra: exit 2 or 1, nothing written |
| L-449 | BRK-IN-005 | must | A | Malformed values that the splitter accepts: unbalanced `{"a":1]`, stray `}` at top level, | — | M | U | STD-ACC; Per-document validation with line number; skip or fail per flags, without poisoning the whole batch Extra: only the bad line is reported/skipped |
| L-450 | BRK-IN-007 | must | A | Giant single value | — | S | U | STD-ACC; Max-document-size guard (default ≤ `http.max_content_length`) with a clear error Extra: under a 512 MB cgroup: clear error |
| L-451 | BRK-RES-002 | must | A | Disk full | L-003 | S | U | STD-ACC; All-or-nothing split commit as documented ("staged and committed together"); existing destination untouched; staging removed Extra: after failure: either all parts or none |
| L-452 | BRK-CLI-003 | must | A | Rename collisions | — | S | U | STD-ACC; Detect many-to-one mappings before writing and fail (or require `--allowMerge`) Extra: refused, nothing written |
| L-453 | BRK-CLI-007 | must | A | Secret leakage canary | L-106 | M | U | STD-ACC; No canary in stdout, stderr, log files, output files or staging names Extra: 0 canary hits across the matrix |
| L-454 | BRK-SIG-001 | must | A | SIGTERM | — | S | U | STD-ACC; Same as SIGINT: cancel, clear scroll, close PIT, remove staging, exit 143 (or 130) Extra: no `.part-` leftovers; contexts closed |
| L-455 | BRK-XV-002 | must | A | `_source` not stored | — | S | N1 | STD-ACC; Detect from the mapping and refuse or warn loudly ("documents cannot be fully dumped") Extra: warning/refusal asserted per version |
| L-456 | BRK-XV-003 | must | A | Data streams | — | S | N1 | STD-ACC; Map backing indices back to the data stream name and require `--bulkAction create`, or warn and stop Extra: asserted |

## 9. Phase A5: Property and fuzz, short runs (gate H2)

Short runs only: 10 min per fuzz target per session (`-max_total_time=600`), with proptests at default cases in T0 and 100k cases once. B runs the long campaigns.

Phase dependency: A1, L-002.

| Task | Req | Pri | Class | Title | Deps | Effort | Mem | Acceptance (short; full text in TEST-REQUIREMENTS) |
|---|---|---|---|---|---|---|---|---|
| L-501 | API-080 | must | A | proptest: NDJSON splitter | — | M | F | 10k cases in T0; 1M nightly |
| L-502 | API-081 | must | A | proptest: double-encoded meta codec | — | M | F | Round trip equal; byte equal with Node on the corpus (or a documented difference) |
| L-503 | API-082 | must | A | proptest/fuzz: URL parsing | — | M | F | Property: no redacted string ever contains the password; parse never panics |
| L-504 | API-083 | must | A | proptest: rename rules | — | M | F | 0 disagreements with the oracle |
| L-505 | API-084 | must | A+B: targets + 60 s smoke runs here; multi-hour campaigns need dedicated cores | cargo-fuzz targets | L-002 (nightly) | L | F | 60 s per target on PR, 1 h nightly; crashes are minimised and committed as regression unit tests |
| L-506 | API-085 | should | A | Differential bulk encoding against Node: for 10k random docs, Rust `encode_doc` output equ | API-060 | M | F | Byte equal, or a documented difference |
| L-507 | API-086 | should | A | ILM→ISM property oracle | — | M | F | 0 accepted-by-converter-but-rejected-by-OpenSearch cases |
| L-508 | SUP-020 | must | A+B: targets + short runs here; long hostile-zstd/gzip campaigns and bomb tests at GB scale are B | Hostile compressed input | L-002 (nightly) | M | F | No crash, no out-of-memory beyond the cap, clear error and non-zero exit; 1 h nightly fuzz with no findings; ruzstd decision recorded |
| L-509 | SUP-022 | must | A+B: harness + short ASan runs here; 4 h ASan campaigns are B | sonic-rs differential fuzz | L-002 (nightly) | M | F | 4 h nightly with no divergence or sanitizer report; any divergence is filed upstream and pinned |
| L-510 | INJ-004 | must | A | Encoder differential proptest | — | M | F | 100k cases per CI run with no mismatch |
| L-511 | INJ-014 | must | A | Escaping corpus round-trip | DATA-003 | M | N1 | Multiset digest equal; differences from Node are documented |
| L-512 | INJ-015 | must | A+B: targets + short runs here; 1 h/target nightly campaigns are B | Fuzz targets | L-101…L-105 | M | F | No crash and no oracle violation; findings become unit tests |
| L-513 | INJ-017 | could | A | Node oracle on the injection corpus | — | S | N1 | Comparison table in TESTING-MATRIX.md |

## 10. Phase A6: Remaining chaos (TST-BRK)

Phase dependency: A3, A4.

| Task | Req | Pri | Class | Title | Deps | Effort | Mem | Acceptance (short; full text in TEST-REQUIREMENTS) |
|---|---|---|---|---|---|---|---|---|
| L-601 | BRK-NET-001 | must | A | Connection reset mid-response on a scroll continuation (after N bytes of the body) | — | S | U | STD-ACC; Not retried (the cursor may have advanced). Exit 1. Staged output discarded. Scroll cleared. Message suggests a larger `--retryAttempts` cannot help and that a rerun or `--scrollId` resume can Extra: digest of a rerun = … |
| L-602 | BRK-NET-002 | must | A | Reset mid-response on a PIT search page, unsliced and sliced (16), with the PIT id rot | — | M | U | STD-ACC; Retry the same `search_after` with the latest PIT id → no duplicates and no gaps Extra: digest equal over 50 seeded runs |
| L-603 | BRK-NET-005 | must | A | Slow drip (1 KB/s) on a large legitimate response with `--timeout 30000` | — | S | U | STD-ACC; Distinguish idle from total: a slow but progressing response succeeds Extra: 200 MB response at 2 MB/s with `--timeout 30000` succeeds once idle-timeout semantics exist |
| L-604 | BRK-NET-007 | should | A | Half-closed socket: the server sends FIN after the headers, or the client write side is cl | — | S | U | STD-ACC; Clear error, retry per mode Extra: completes or fails within the timeout |
| L-605 | BRK-NET-008 | must | A | DNS failure: NXDOMAIN at start; resolver black-holed (no answer); DNS failing mid-run | — | S | U | STD-ACC; NXDOMAIN fails fast with the host name. Black-holed DNS is bounded by the connect timeout. Mid-run failure is retried Extra: bounded time; message names the host (password-free) |
| L-606 | BRK-NET-009 | must | A+B: single-node restart here; multi-node rolling restart and replica failover are B | Node restart mid-dump and mid-restore (`podman restart`, ~30–60 s down) | — | S | N1 | STD-ACC; Restore: survives if `--retryAttempts` covers the downtime, otherwise fails clearly. Dump: scroll/PIT contexts are lost → clear "context lost, rerun" error Extra: resume recipe works (LOAD-006) |
| L-607 | BRK-NET-010 | must | A | Proxy returns HTML: 200 `text/html` login page; 502/503 HTML error pages; WAF 403 HTML | — | S | U | STD-ACC; Content-type check: "expected JSON from <host>, got text/html (first 200 bytes …)". Error bodies truncated to ≤ 2 KB Extra: message ≤ 4 KB; no engine fallback without a warning |
| L-608 | BRK-NET-011 | should | A | `Content-Encoding: gzip` with a plain body; truncated gzip response; `Content-Length` shor | — | S | U | STD-ACC; Clear decode error; treated like a transport error Extra: bounded, clear |
| L-609 | BRK-NET-012 | must | A | Redirects | — | S | U | STD-ACC; Do not follow redirects for API calls (or only same-origin 307/308), and say so Extra: no request reaches the redirect target with an `Authorization` header; no silent success |
| L-610 | BRK-TLS-001 | must | A | Server cert issues: expired, not yet valid, hostname mismatch, IP-only SAN accessed by nam | — | S | N1 | STD-ACC; Fails with the rustls reason, the host, and a hint (`--ca`, `--insecure`) Extra: message names the reason |
| L-611 | BRK-TLS-002 | must | A | Client cert faults: required but missing; wrong CA; encrypted key with a wrong `--pass`; l | — | S | N1 | STD-ACC; Clear per-case error; never echo the passphrase Extra: canary not found |
| L-612 | BRK-TLS-003 | should | A | Protocol mismatch: `https://` to a plain port; `http://` to a TLS port; TLS 1.0/1.1-only s | — | S | N1 | STD-ACC; Clear message ("server spoke plain HTTP", "no common protocol") Extra: bounded |
| L-613 | BRK-TLS-004 | should | A | `SSL_CERT_FILE`/`SSL_CERT_DIR` pointing at garbage; an empty system store | — | S | U | STD-ACC; Debug log, then falls back to bundled roots (documented) Extra: documented and tested |
| L-614 | BRK-SRV-002 | must | A | 5xx bursts on `_bulk` beyond the retry budget, with `--ignore-errors` | — | S | U | STD-ACC; The loss is counted and reported; exit code per Q3 Extra: asserted counts |
| L-615 | BRK-SRV-003 | must | A | Bulk response with mixed items: 201, 200, 409, 400 mapper_parsing, 429, 503, 500, and an i | — | S | U | STD-ACC; Each item classified correctly; only 429/503 retried; ids of failed docs listed Extra: counts equal ground truth |
| L-616 | BRK-SRV-004 | must | A | Bulk response item count ≠ documents sent; `errors:false` but items missing; response not | — | S | U | STD-ACC; Fail clearly, never count as written Extra: / router mock |
| L-617 | BRK-SRV-005 | must | A | Truncated or malformed JSON in a 200 search response (cut inside `hits.hits[k]`); missing | — | S | U | STD-ACC; The batch is rejected before any of its docs reach the sink Extra: stdout consumer sees no partial page + exit 1 |
| L-618 | BRK-SRV-007 | must | A | Scroll expiry mid-dump (`--scrollTime 1s` + slow sink / `--throttleInterval`) and PIT expi | — | S | N1 | STD-ACC; Clear "search context expired; raise --scrollTime/--pitKeepAlive" Extra: message asserted |
| L-619 | BRK-SRV-008 | must | A | Index deleted mid-dump; index deleted mid-restore (auto-create recreates it with dynamic m | — | M | N1 | STD-ACC; Dump: clear error. Restore: detect that the target index was recreated (e.g. check the index uuid at start and end) or at least report mapper errors with a non-zero exit Extra: asserted |
| L-620 | BRK-SRV-010 | must | A | Duplicate or overlapping docs across PIT pages or slices (mock returns the same hit on two | — | S | U | STD-ACC; Detected by the digest in tests; the tool does not dedupe, but tests catch regressions Extra: digest equal on tie-heavy data |
| L-621 | BRK-SRV-011 | should | A | Version conflicts under concurrency: two restores of the same file in parallel; `--bulkAct | — | S | N1 | STD-ACC; Counts correct; ISM second 409 reported as a concurrent change Extra: / live |
| L-622 | BRK-SRV-012 | should | A+B: red via allocation filtering on one node here; a real lost-node red cluster needs multi-node (B) | Cluster `red` / some shards unassigned during a dump | — | S | N1 | STD-ACC; Fails clearly naming the shards Extra: / node.sh (`index.routing.allocation.require` to a missing node) |
| L-623 | BRK-IN-001 | must | A | Truncated gzip (cut at random offsets, incl. inside the trailer) and trailing garbage afte | — | S | U | STD-ACC; Error naming the file, byte offset and docs read so far; non-zero exit Extra: exit 1; the message has an offset |
| L-624 | BRK-IN-003 | should | A | Wrong compression: bzip2 (the Rally corpora!), xz, lz4, zip, tar.gz | — | S | U | STD-ACC; "bzip2 input is not supported; decompress first" Extra: message asserted |
| L-625 | BRK-IN-006 | must | A | Invalid UTF-8 inside strings; UTF-16 LE/BE files with BOM; a UTF-8 BOM in the middle of th | — | S | U | STD-ACC; "line N is not valid UTF-8" Extra: clear messages |
| L-626 | BRK-IN-008 | must | A | Deep nesting: 1k, 10k and 100k levels of `[`/`{` in a data file and in a meta file | — | S | U | STD-ACC; Clear error, no crash Extra: no SIGSEGV/SIGABRT |
| L-627 | BRK-IN-009 | must | A | Duplicate keys in a document line: `{"_id":"a","_id":"b",…}`, duplicate `_source`/`_index` | — | S | U | STD-ACC; Same semantics as Node (`JSON.parse` keeps the **last**) or reject Extra: Node vs Rust digest equal, or a documented rejection |
| L-628 | BRK-IN-010 | must | A | Duplicate `_id`s across lines (last write wins) | — | S | U | STD-ACC; Report created vs updated counts (from item `result`) so users can see overwrites Extra: summary includes `updated` |
| L-629 | BRK-IN-011 | must | A | Big numbers and odd numerics: > 2^64, `-0`, `1e400`, long mantissas, numbers in `_id`/`_ro | — | S | U | STD-ACC; Numeric `_id` handled as Node does (stringified) without precision loss Extra: byte-exact `_source`; `_id` equals Node's |
| L-630 | BRK-IN-012 | must | A | Special characters in `_id` (`/ ? # % + " \` unicode, 512/513 bytes); in index names (`#`, | — | S | U | STD-ACC; Encoded correctly or rejected up front with the ES rule Extra: per-case expectation file |
| L-631 | BRK-IN-013 | must | A | Node double-encoding edge cases for meta files: triple-encoded strings; a string literal h | — | S | U | STD-ACC; Accept everything Node writes; reject the rest clearly Extra: Node-generated corpus all restores |
| L-632 | BRK-IN-014 | must | A | Empty inputs: 0-byte file, whitespace only, BOM only, an empty `.gz`, an empty `.zst`, `/d | — | S | U | STD-ACC; Data: `Total Writes: 0` plus a warning, exit 0 (or Q3). Meta: clear error Extra: asserted |
| L-633 | BRK-IN-015 | should | A | Split-part problems: a missing part (`split-1`, `split-3`); parts restored out of order; a | — | M | U | STD-ACC; Each part restores independently; truncated part per IN-001; a helper (`--input` glob or a manifest) warns about gaps Extra: gap warning if a manifest/glob mode is added |
| L-634 | BRK-IN-016 | could | A | Input file modified while reading (appended, truncated, replaced) | — | S | U | STD-ACC; Defined behaviour (read to the EOF seen at open, or fail) Extra: documented |
| L-635 | BRK-RES-001 | must | A | Low memory: cgroup 64/128/256 MB with defaults; with `--limit 10000` × 40 KB docs; with `- | L-003 (cg-run) | M | U | STD-ACC; Stays within the README memory formula; warns at start when the estimate exceeds the available memory Extra: no OOM inside the formula's bound; leftovers handled (SIG-003) |
| L-636 | BRK-RES-003 | should | A | stdout to `/dev/full`; stdout to a closed pipe (exit 0 tested already); stderr closed | — | S | U | STD-ACC; ENOSPC on stdout → exit 1 Extra: asserted |
| L-637 | BRK-RES-004 | must | A | FD limit `ulimit -n 32` with `--concurrency 64`, `--slices 16`, many split parts | — | S | U | STD-ACC; "too many open files" with a hint Extra: message asserted |
| L-638 | BRK-RES-005 | must | A | Extreme numeric flags: `--limit 1000000`, `--limit 18446744073709551615`, `--concurrency 1 | — | S | U | STD-ACC; Validated at parse time (exit 2) or server errors explained Extra: no panic for any value in the u64 range (proptest over the CLI) |
| L-639 | BRK-RES-006 | must | A | Tiny `--fileSize` (`1b`) and `--maxRows 1` on 1M docs | — | S | N1 | STD-ACC; Refuse below a sane minimum, or stream-commit with a warning Extra: refused (exit 2) or completes within budget |
| L-640 | BRK-RES-007 | should | A | Very high `--concurrency` with `--maxSockets 1` | — | S | U | STD-ACC; Semantics documented and enforced Extra: asserted connection count |
| L-641 | BRK-CLI-001 | must | A | Conflicting flags, table-driven (≥ 40 combos): `--fsCompress` + `--compressionType zstd`; | — | M | U | STD-ACC; Each is a usage error (exit 2) or has documented precedence Extra: table asserted |
| L-642 | BRK-CLI-002 | must | A | Same source and destination | — | S | U | STD-ACC; Refuse unless `--force` Extra: refused |
| L-643 | BRK-CLI-004 | should | A | Regex abuse in `--renameIndex`, `--templateRegex`: huge patterns (`(a{1000}){1000}`), 1 MB | — | S | U | STD-ACC; Clear compile error with a size-limit hint; missing captures are an error Extra: asserted |
| L-644 | BRK-CLI-005 | must | A | Output path abuse: a directory; `/`; a symlink to elsewhere (hard-link commit semantics); | — | S | U | STD-ACC; Clear errors; the symlink target is never silently replaced; the mode is preserved Extra: asserted |
| L-645 | BRK-CLI-006 | must | A | Credential sources and precedence: URL vs `--httpAuthFile` vs env vs `--username/--passwor | — | S | U | STD-ACC; Documented precedence; CRLF trimmed; warning for world-readable auth files Extra: asserted |
| L-646 | BRK-CLI-008 | should | A | Scheme-less URLs (`user:pw@host:9200/idx`), `HTTP://` uppercase, `http:/host` typos | — | S | U | STD-ACC; Warn when a "file" output looks like a URL (`host:port`, `@`) Extra: warning asserted |
| L-647 | BRK-SIG-002 | must | A | SIGINT during a file→cluster restore and cluster→cluster | — | S | U | STD-ACC; Stop after in-flight bulks, print `Total Writes` and the `--offset` to resume, exit 130 Extra: message includes the resume offset |
| L-648 | BRK-SIG-003 | must | A | `kill -9` at random points (dump, split, restore) ×50 runs: crash safety of output and spl | — | M | N1 | STD-ACC; The final path is either absent or complete (digest equal); staging leftovers are detectable and a later run cleans them up or warns Extra: 50/50 runs crash-consistent |
| L-649 | BRK-SIG-004 | should | A | Second SIGINT during cleanup (forced exit path `std::process::exit(130)`); SIGINT while th | — | S | U | STD-ACC; The forced exit still leaves no committed partial output; the analyzer index is reopened, or the message says it is closed Extra: asserted |
| L-650 | BRK-SIG-005 | must | A | Resume semantics end to end: `--scrollId` after a kill (context alive), `--offset` after a | — | S | N1 | STD-ACC; Documented recipes produce the full digest Extra: digest equal |
| L-651 | BRK-ILM-001 | must | A | Unsupported phases and actions (`frozen`, `searchable_snapshot`, `downsample`, `wait_for_s | — | S | U | STD-ACC; Refused naming phase and action; lenient mode drops with `DROPPED …`; no partially written policy set Extra: OpenSearch accepts every converted policy |
| L-652 | BRK-ILM-002 | must | A | Odd but valid ILM: phases out of JSON order; `min_age` in every unit (`0ms`, `30s`, `1d`, | — | S | U | STD-ACC; Correct state order and transitions; ISM accepts them Extra: live PUT on OS 1.3/2.19/3.x |
| L-653 | BRK-ILM-003 | should | A | Converting ES built-in managed policies (when `--filterManaged=false`) and ISM → ILM attem | — | S | N1 | STD-ACC; Clear refusal for ISM → ILM; managed policies convert or refuse clearly Extra: / live |
| L-654 | BRK-XV-001 | must | A | OpenSearch with `compatibility.override_main_response_version: true` (reports `7.10.2`; ch | — | S | N1 | STD-ACC; Detect OpenSearch by other signals (`tagline`, `_plugins` probe) or fail clearly Extra: correct engine on the live OS setting |
| L-655 | BRK-XV-004 | should | A | Field types without an equivalent across engines: `flattened` → OS, `flat_object` → ES, `s | — | M | N1 | STD-ACC; Clear refusal naming the field and type; data restore fails clearly when the mapping is missing Extra: expectation table (API-071) |
| L-656 | BRK-XV-005 | should | A | ES 7.10 OSS (no X-Pack) as a target of ILM, security and PIT features; OS 1.3 as a target | — | S | N1 | STD-ACC; Clear capability errors Extra: asserted |
| L-657 | BRK-XV-006 | should | A | `hits.total` as an int (ES 6.x-shaped goldens, `rest_total_hits_as_int`) and `_type` prese | — | S | U | STD-ACC; Handled Extra: golden |

## 11. Phase A7: Runtime supply-chain (gate H4)

Phase dependency: A2, A3.

| Task | Req | Pri | Class | Title | Deps | Effort | Mem | Acceptance (short; full text in TEST-REQUIREMENTS) |
|---|---|---|---|---|---|---|---|---|
| L-701 | SUP-001 | must | A | Egress-denied test runs | image build, L-003 | M | N1 | A planted test that resolves `example.com` makes the job fail. The normal suite passes with 0 unexpected connections |
| L-702 | SUP-002 | must | A | Syscall and file-access tracing | L-002 (edl-strace) | M | N1 | The allow-list diff is empty; a new path requires a reviewed allow-list change |
| L-703 | SUP-003 | should | A | Enforcement profile | SUP-002 | M | N1 | Suite passes under the profile; a test binary that tries `execve` or writes outside is blocked |
| L-704 | SUP-010 | must | A+B: two builds in different dirs/users here; cross-machine bit-for-bit verification is B | Reproducible builds | L-002 | M | U2 | Bit-for-bit equal in CI |
| L-705 | SUP-016 | should | A | Release-candidate behaviour diff | SUP-011 | S | N1 | Profile diff artefact |

## 12. Phase A8: L0 and L1-lite loads (final hand-off)

L0 = 100k docs (edgen `logs`); L1-lite = 1M docs (edgen) plus the 1.3 GB `http_logs` documents-191998 subset capped at 1M lines. Disk ≤ 12 GB; delete indices and dumps after each cell (keep digests and metrics).

Phase dependency: A3.

| Task | Req | Pri | Class | Title | Deps | Effort | Mem | Acceptance (short; full text in TEST-REQUIREMENTS) |
|---|---|---|---|---|---|---|---|---|
| L-801 | LOAD-004 | must | A+B: scenario matrix at L0 (100k) and L1-lite (1M) here; 10M is B | Scenario matrix | — | M | L1L | Matrix defined in `scenarios/*.toml`; each cell has a correctness gate and a perf baseline |
| L-802 | LOAD-005 | must | A+B: flat-memory check 100k vs 1M here; 10M and 1 MB-doc tiers are B | Memory is bounded and flat | — | M | L1L | Asserted per run; the formula is updated if it is wrong |
| L-803 | LOAD-006 | must | A+B: kill/resume at L0/1M here; 10M is B | Resumability under load | — | M | L1L | Digest equal in (a)–(d); the documented resume recipe is tested verbatim |
| L-804 | LOAD-007 | must | A+B: fault injection at 1M here; 10M with flood-stage disk watermarks is B | Failure injection under load | — | M | L1L | Run completes with digest equal, **or** exits non-zero with a clear message; never exit 0 with loss (see BRK-SRV-001) |
| L-805 | LOAD-008 | must | A+B: compression levels at 1M here; multi-GB single files at 10M are B | Compression at scale | — | S | L1L | Results recorded; README numbers within ±20% or updated |
| L-806 | LOAD-009 | must | A+B: splitting at 1M here; 10M with 256 MB parts is B | Splitting at scale | — | S | L1L | Asserted |
| L-807 | LOAD-010 | must | A+B: curves can be measured here but the shared CPU makes them noisy; authoritative curves are B | Concurrency scaling curves | — | M | L1L | Regression > 10% on any point fails nightly |
| L-808 | LOAD-011 | must | A+B: Node comparison at 100k/1M here; Node at 10M takes hours and needs a quiet machine (B) | Node comparison | LOAD-001, /tmp/ed Node | M | L1L | Ratios recorded; digest equal |
| L-809 | LOAD-014 | should | A+B: 300 indices here (single-node shard limit and heap); 1,000 indices × many fields is B | Many indices and many fields: 1,000 small indices (wildcard dump of `data`, `mapping`, `se | — | M | L1L | Completes; meta types finish in under N minutes (baseline) |
| L-810 | LOAD-015 | should | A | Large documents: pmc corpus (~40 KB avg); edgen `huge` (1–10 MB docs); one 90 MB doc near | — | S | L1L | Digest equal; 413 handled with a clear error when exceeded |
| L-811 | LOAD-017 | must | A | Server-side leak check after every load and IT run | — | S | N1 | Asserted in the teardown of every load/IT run |
| L-812 | LOAD-018 | should | A | Data streams at scale: restore 1M docs into a data stream (requires `--bulkAction create`) | — | S | N1 | Digest equal; behaviour documented (BRK-XV-003) |
| L-813 | LOAD-020 | should | A | Trend store and regression gates: results JSON appended to a `bench-results` branch or an | LOAD-003 | M | L1L | Nightly comment-free report artefact; > 10% regression fails |
| L-814 | DATA-002 | must | A | Profiles emit their mapping and settings, valid on every target version (with per-engine v | DATA-001 | M | N1 | Index creation succeeds on all 9 images |
| L-815 | DATA-003 | must | A | Edge catalogue §5.4 implemented as `--profile edge`, with each case tagged (`_source.case` | DATA-001 | M | U | Every case has an expected outcome per version (store / reject with type X) |
| L-816 | DATA-007 | should | A | OSB `generate-data` driven by our fixture mappings for a "foreign generator" cross-check o | — | S | N1 | Generated docs load on every version |
| L-817 | DATA-008 | could | A | elastic-integration-corpus-generator-tool with `--seed` for ECS data-stream corpora (nginx | — | S | N1 | Deterministic output hash |

## 13. Phase A9: Hand-off packaging (at each gate)

| Task | What | Deps | Effort | Mem | Acceptance |
|---|---|---|---|---|---|
| L-901 | **Create bundle #N at a gate (H1, H2, H3, H4, final).** Run the `git bundle` commands in `HANDOFF-TASKPLAN.md` §3 from the repo that holds the work (`/workspace/edrs-work`, or the main repo when it is free), together with a tarball of `/workspace/elasticdump-rs-plan/*.md`. Record the base SHA in `handoff/BASE-SHA` | the gate's phase | 0.25 d | U | `git bundle verify` OK; `sha256sum` file written next to the bundle |
| L-902 | **Merge back B results:** `git fetch <b-results.bundle> b/results:b-results`; review the commits (B is untrusted input as well: no new deps, no CI changes, no build.rs); copy `results/B-*/result.json` and `results/SUMMARY.md` into `results/remote/`; update TESTING-MATRIX.md | B results arrive | 0.5 d each | U | Results validated against the schema in HANDOFF §9; findings filed as tasks |

## 14. Phase A10: Feature backlog (decision-gated; GAPS.md plus INJ-013)

None of these may start until the owner decides to build them (GAPS.md "does not recommend what to build"). They are classified now so the split is known. Effort is outside the 23-week test total.

| Task | GAPS rows | Class | Why | Effort | Mem |
|---|---|---|---|---|---|
| L-A01 | INJ-013 `--allow-index` and `--dry-run` for untrusted restores (Q13) | A | Pure code plus MockEs | M | U |
| L-A02 | §1 `--transform` (JS) and module transforms: Rust/WASM substitute, `--searchBodyTemplate`, custom transports, `--toLog` | A | Code plus unit/IT tests; no scale needed | L | U |
| L-A03 | §2 CSV transport and all `--csv*` flags | A | Code plus file tests; Node interop with `/tmp/ed` | L | U |
| L-A04 | §3 S3: `s3://` input/output plus `--s3*` flags, tested against a **MinIO container** | A, then **B13** for real AWS S3 | MinIO fits here (~300 MB). Real S3 (SSE-KMS, storage classes, IAM) needs cloud credentials | L | N1 |
| L-A05 | §4 AWS SigV4, `--awsChain`, ini profiles, `--awsService aoss`, OpenSearch Serverless | A for code (signing tested with the AWS SigV4 test vectors and a signature-checking mock), then **B13** for live Amazon OpenSearch Service and AOSS | No AWS account here | M+M | U |
| L-A06 | §4 SOCKS5 proxies per direction | A | `microsocks`/dante container on an internal network | S–M | N1 |
| L-A07 | §5 `--delete`, `--delete-with-routing` | A | Destructive; test only on throwaway containers | M | N1 |
| L-A08 | §5–§8 small deviations: PUT `_bulk`, `/index/type` URLs, `--scroll-with-post`, `scrollRetryDelay`, default `User-Agent`, `--force-os-version`, legacy encrypted PEM `--pass`, `--username` for output, `--maxSockets`, and the "verify" rows (`--noRefresh`, default `searchBody`) | A | Small code changes plus MockEs/live single node | 10 × S | U/N1 |
| L-A09 | §9 `multielasticdump` (dump/load, `--match`, `--matchType alias,datastream`, `--parallel`, `--ignore*`, split-file aware load, pass-through) | A for code plus ≤ 50 small indices; **B** for 500+ indices in parallel with whole-cluster migrations at L1 | Parallel many-index migrations need RAM and disk | L | N1 |
| L-A10 | GAPS "Future options": **ruzstd** decode backend (feature flag), compared with C zstd on SUP-020 corpora | A | Small corpora; also feeds Q14 | M | F |
| L-A11 | `--fileSize` UTF-16 code-unit compatibility mode (§8 deviation), only if wanted | A | Unit tests | S | U |

## 15. Requirements that are B only (not done here)

| Req | Why B | B task |
|---|---|---|
| LOAD-013 (should) | 10M docs on two clusters at once (2×4-6 GB heaps) plus 2×~15 GB index storage; exceeds RAM/disk here | B4 |
| LOAD-016 (should) | 6-12 h soak on L2 (100 GB+) data: disk (35 GB free) and wall-clock; would also starve the shared box | B5 |
| LOAD-019 (could) | Rally/OSB baseline needs dedicated, quiet CPUs and L1 corpora; numbers are meaningless on a shared 8-vCPU box | B6 |
| DATA-006 (should) | Persistent cache of all L1 corpora (~40 GB parsed incl. eventdata 16.4 GB, pmc 23 GB) does not fit the 35 GB shared disk | B6 |
| DATA-009 (could) | flog unbounded streaming input exists only for soak runs (LOAD-016) | B5 |

## 16. Classifications I was unsure about

| Req | Chosen | Doubt | What would change it |
|---|---|---|---|
| API-072 (live cluster→cluster pairs) | A+B | Two 512 MB-heap nodes fit only when the 12.5 GB VM is off | If `MemAvailable` stays < 9 GB for long, give all of it to B9 |
| LOAD-010 (concurrency scaling curves) | A+B | Curves can be measured here, but 8 shared vCPUs make them noisy | If the curves are only needed as trends, A is enough |
| LOAD-011 (Node comparison) | A+B | Node at 1M takes ~10–20 min here; at 10M it takes hours | — |
| LOAD-014 (1,000 indices) | A+B | The single-node shard limit (1,000 by default) and heap; 300 indices fit | `cluster.max_shards_per_node` plus a 2 GB heap may allow 1,000 here |
| LOAD-015 (90 MB single doc) | A | Needs `http.max_content_length` ≥ 100 MB and a 2 GB heap; at the edge of L1L | Move it to B if it OOMs |
| BRK-RES-001 (low memory) | A | Only works through `sudo` cgroups (podman `--memory` is not enforced) | If using root on the shared box is not acceptable → B |
| SUP-002 / SUP-003 (strace, seccomp/Landlock) | A | strace only inside a container (`SYS_PTRACE`); eBPF untested | B12 adds host-level eBPF tracing at scale |
| SUP-010 (reproducible builds) | A+B | Same-machine check here; cross-machine is B11 | — |
| DATA-007 (OSB `generate-data`) | A | Needs OSB from pip (HTTPS works) and Python; the output is small | Move to B6 if the OSB install is too heavy |
| BRK-SRV-012 / BRK-NET-009 | A+B | A single node can only show yellow/red by allocation filtering, or restart itself | Real red, failover and rolling restarts → B8 |
| TST-INF-010 / L-107 (CI layout, workflow fixes) | A | Can be written and linted here but **not executed** (local fork, no push, no runner) | The owner or B runs them on a CI runner |
