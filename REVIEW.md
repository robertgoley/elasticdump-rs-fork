# Hostile-code review of upstream-origin code (TST-SUP-012)

**Scope.** Every line in `src/` that `git blame -w` attributes to a commit reachable from `main` (the upstream mirror, `b32b6e9`). `scripts/sup/upstream-lines.py` lists them: 4,263 lines in 16 files, mostly `src/retrieval/*` (scroll/PIT/search_after retrieval, workers, extraction) and `src/output.rs` (staged output). The upstream tests (`tests/integration_test.rs`, `tests/support/mock_es.rs`) and `scripts/benchmark/compare-with-elasticdump.sh` were reviewed against the same checklist.

**Method.** I ran `scripts/sup/upstream-lines.py --checklist src`. It flags upstream lines in six categories: I/O targets, spawned work, swallowed errors, exit/panic paths, cancellation and logging. I then read each flagged site and the code around it. This was a targeted review driven by the checklist, not a separate line-by-line read of all 4,263 lines.

**Reviewed:** 2026-10-29 by the local agent, at rewrite `9f9a8d7` plus this commit (L-208). Any later change to these lines re-opens the review: CODEOWNERS covers `src/` and `tests/`.

## Checklist results

| Item | Hits (upstream lines) | Result |
|---|---|---|
| I/O targets | 12 (6 in tests) | All in `output.rs`. Staging file `.<name>.part-<pid>-<n>` sits in the output's directory; `create_new`; mode copied from the overwritten file; rename-commit plus directory fsync. Removal happens only on our own staging path. Nothing writes outside the user-chosen output path. OK |
| Spawned work | 7 | The signal task, output task and PIT waiters are each wrapped in `AbortOnDrop`/`PitAbortGuard`. Worker OS threads join through completion channels, and a panic becomes an error via `catch_unwind` (`worker_thread_panic`). Nothing outlives `dump_data`. OK |
| Swallowed errors | 21 | `let _ = …set/send` covers once-cells and channels whose receiver may be gone; that is intended. `unwrap_or(0)` is for progress lengths only. Fallback metadata uses `.ok()`, with the first error still returned. Further pipeline errors are logged as "Suppressed …" at error level and the first error is returned. OK |
| Exit/panic paths | 17 outside tests | `process::exit(130)` fires only on a second Ctrl+C, after the first one cancelled. It leaves the hidden staging file, as SIGKILL does (see the L-003 finding). The `unwrap`/`expect` calls on `as_object_mut` rely on the invariant that `finalize_search_body` always yields an object. `expect` on PIT generation and coordinator is a structural invariant of PIT mode. `progress.rs` unwraps static indicatif templates. OK |
| Data loss on cancellation | 34 | Cancel or output-closed calls `output_target.abort()`, which removes the staging file and never touches the final path. An existing destination survives a failed or cancelled run; the `test_failed_dump_preserves_existing_output` and `test_sigint_cleans_up_staging` tests confirm this. OK |
| Logging of secrets | 47 | No credentials or headers are logged in upstream code; the client side was rewritten and covered in A1. **U-1 fixed:** see below |

## Findings

**U-1 (low): scroll and PIT ids logged in full at debug level.**

- Where: `scroll.rs` ("Got scroll_id"), plus the full PIT search body in `pit_search.rs`.
- Risk: a context id lets anyone with cluster access read the open search context until keep-alive expires.
- Fixed: `redact_context_id` logs the first 8 characters plus the length.
- Test: `retrieval::pit_search::redact_tests`.

**U-2 (medium, tooling only): the benchmark script could delete existing data.**

- `scripts/benchmark/compare-with-elasticdump.sh` deleted `--index-name <x>` at start and in cleanup, even when that index already existed on `ES_URL` (default `localhost:9200`).
- Fixed: the script now refuses an existing caller-named index unless `BENCH_ALLOW_DELETE=1`, and cleanup no longer deletes it.
- Generated index names (`<prefix>_<timestamp>_<pid>`) are unchanged.

**U-3 (info): `process::exit(130)` on a second Ctrl+C leaves the staging file.**

- This is the same as the SIGKILL case.
- Decision for the owner, shared with L-003: sweep stale `.<name>.part-<deadpid>-*` files on start.

**Upstream tests:** `tests/integration_test.rs` writes only to tempdirs (`scratch()`, F-04), and only to indices named `elasticdump_rs_test_*` on `IT_ES_URL`. Live tests in the `load` tier need `--features load`. No network access beyond the configured cluster.

No hostile behaviour found: no hidden I/O targets, no unexpected network endpoints, no telemetry, no process spawning, no unsafe code (`forbid(unsafe_code)`).
