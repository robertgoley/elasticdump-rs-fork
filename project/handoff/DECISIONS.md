# Decisions confirmed by Robert (2026-10-30)

These are final. Do not reopen them; implement or keep as stated.

## Keep as implemented (no code change)
- `--offset` + `--slices` is a usage error (sliced results have no stable order).
- `--compressionType` overrides `--fsCompress`.
- Same source/destination check does not detect aliases or alternate hostnames (accepted limitation).
- Deleted-index-mid-restore check runs only on restores longer than 5 s (L-619).
- Restore rejects invalid UTF-8, nesting deeper than 1000 levels, and duplicate metadata keys (default kept; not re-asked).
- Junk files `oom` and `test_query_elasticdump_rs_test_857109_16.json` stay in the tree (do not delete).
- zstd decoding stays on libzstd; sonic-rs stays pinned =0.5.10 with sanitize.

## New work items (not yet implemented in b4134af)
| ID | Owner | Change |
|----|-------|--------|
| D-1 | impl (review agent reviews) | Red index: keep refusing partial results by default; add an opt-in flag (suggested `--allowPartialResults`) that accepts them and logs a warning with the failed-shard count. Tests for both paths. |
| D-2 | impl (review agent reviews) | Compression default stays gzip for Node parity; zstd only when asked via `--compressionType zstd`. Verify docs/help and defaults say so; add a test if missing. |
| D-3 | impl/runner → lead | Dockerfiles default to the dhi.io Alpine-based `-dev` images (registry variables unchanged). Keep a Debian build arg/target as the alternate. Rebuild and re-run the CVE scan in SECURITY-CVE.md. |
| D-4 | lead only (credential handling) | Add `--output-username` / `--output-password` (and confirm `--input-username` / `--input-password` parity) matching Node elasticdump. Passwords must never appear in logs, errors, `--debug`, or panics; reuse the existing redaction and add fuzz/redaction tests. Flags override URL-embedded credentials. |
| D-5 | impl (review agent reviews) | After an OOM kill, leave the hidden `.part` file in place; on the next run targeting the same output, print a warning naming the leftover `.part` file. Do not delete or overwrite it automatically. |

_Owner column re-labelled 2026-10-31 for the Grok 4.7 / OpenCode agent split (impl, runner, review, lead); decisions unchanged._

## Decisions 2026-11-01 (L-808b to L-808d)
- Default build and image: glibc + mimalloc, image on `dhi.io/static:20250419-glibc-debian13` (supersedes D-3's Alpine default). Alpine keeps musl's allocator as the non-default `-alpine` variant.
- The default image's 5 no-fix High CVEs (libc6 CVE-2026-19499, CVE-2026-5435, CVE-2026-84243; libgcc-s1 CVE-2026-102010, CVE-2026-95619) are accepted until upstream fixes them; `make scan` ignores only those, only while not-fixed/wont-fix.
- License: the project stays MIT; Apache-2.0 only for the 12 fixtures copied from elasticsearch-dump.
- TESTING-TODO, not now: l638 `--intervalCap 1` timing on a stable machine (do not widen the limit); full arm64 image build (low priority).
- Usage: past 90% weekly usage is OK if every task completes before 100% (start a task only if usage + 1.5 × estimate < 98%).
