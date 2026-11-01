# Testing in progress

The task plan delegated to the other agent (Grok 4.7 through OpenCode, using [`project/handoff/HANDOFF-GROK-OPENCODE.md`](project/handoff/HANDOFF-GROK-OPENCODE.md)). Everything below is assigned to it; it commits each task to `main` as `L-NNN: ...` and adds a row to [`project/plan/PROGRESS.md`](project/plan/PROGRESS.md). Finished tasks move to [TESTING-COMPLETED.md](TESTING-COMPLETED.md). Tests outside this plan are in [TESTING-TODO.md](TESTING-TODO.md).

Status as of `main` at L-808a (2026-11-01): the agent finished A6 (L-643 to L-657), L-701, L-702, L-704, L-705, L-801, the L-455c fix-up (empty `_source` refused unless `--allowMissingSource`), L-802 (memory checks; README memory formula changed, pending lead review), L-803 (kill/resume runs), L-802a (allocator study; default allocator unchanged, owner decision pending), L-804 (fault injection at 1M; non-zero exits await lead classification) L-805 (compression levels; README numbers updated), L-806 (splitting at scale), L-807 (concurrency curves; box baseline committed) L-808 (Node comparison; README Performance table re-measured) and L-808a (TESTING-TODO row for networked compression performance). **Next: L-809.** L-703 was skipped and is still open.

Who: impl = bounded code card, runner = scripted campaign, lead = supervised session for security-sensitive work, review = independent read-only review of every change.

## 1. Owner decision items

| Item | State | Who | Note |
|---|---|---|---|
| D-1 red-index partial results | not started | impl | Opt-in flag (suggested `--allowPartialResults`); refuse by default, warn with failed-shard count when allowed. Tests for both paths. |
| D-2 gzip default | to verify | impl | Confirm gzip stays the default and zstd is opt-in in code, help and docs; add a test if missing. |
| D-3 Alpine Dockerfile default | to verify | impl, runner | Dockerfiles already build on dhi.io Alpine `-dev` images; confirm the default, keep a Debian alternate, re-run the CVE scan. |

D-4 (per-direction credential flags, L-645) and D-5 (leftover `.part` warning, L-648) are done.

## 2. Runtime supply chain (remaining, A7)

| Item | Req | Who | Note |
|---|---|---|---|
| L-703 | SUP-003 | lead | seccomp/Landlock profile (security boundary) |
| H4 | gate | lead | cut bundle #4 (§9) once L-701..704 merge → unblocks B11, B12 |

## 3. Load and data generation (A8, L-801 to L-817)

Scale: L0 100k docs and L1-lite 1M docs, disk at most 12 GB.

| Item | Req | Who | Note |
|---|---|---|---|
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

## 4. Big-host campaigns (B0 to B13)

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
