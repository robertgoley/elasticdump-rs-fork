# Testing in progress

Nothing is running right now. The last task committed was L-705 on 2026-10-31. These are the items next in line, or started but unfinished, as of `main` at that commit. Full detail: [`project/plan/PROGRESS.md`](project/plan/PROGRESS.md) (resume note at the end) and [TESTING-TODO.md](TESTING-TODO.md).

| Item | State | Note |
|---|---|---|
| L-801 LOAD-004 scenario matrix | next | Named as the next task in the PROGRESS resume note. Runs scenario matrix cells with `scripts/load/run.sh` at L0 (100k) and L1-lite (1M). |
| L-703 SUP-003 seccomp/Landlock profile | skipped, open | Passed over while L-701, L-702, L-704 and L-705 were done. Security boundary work for the lead role. Blocks the H4 bundle. |
| H4 checkpoint bundle | waiting | Cut once L-703 is merged; unblocks B11 and B12. |
| D-1 red-index partial results | not started | Opt-in flag (suggested `--allowPartialResults`); refuse by default, warn with failed-shard count when allowed. |
| D-2 gzip default | to verify | Confirm gzip stays the default and zstd is opt-in in code, help and docs; add a test if missing. |
| D-3 Alpine Dockerfile default | to verify | The Dockerfiles already build on dhi.io Alpine `-dev` images; confirm the default, keep a Debian alternate, re-run the CVE scan. |
| Open Dependabot branches | needs review | Six update branches (base64, pkcs8, rand, rand_chacha, sha2, GitHub Actions). Review against the pinning policy in `docs/DEPENDENCY-POLICY.md` before merging. |
| L-505 fuzz harness rows | superseded | Two `wip` rows in PROGRESS.md; the later rows and the 15-target fuzz work replaced them. |

Done from the decision list: D-4 (per-direction credential flags, L-645) and D-5 (leftover `.part` warning, L-648).
