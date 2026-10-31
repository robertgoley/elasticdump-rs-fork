# export-elasticdump-rs-b4134af
- repo/: elasticdump-rs-full.bundle (git bundle --all, verified, HEAD b4134af on rewrite) and REPO-STATE.txt. The tree was clean, so there is no uncommitted.patch.
- plan/: copy of /workspace/elasticdump-rs-plan. Left out: the elasticdump-rs-h1/h2.bundle files, elasticdump-rs-plan-h1/h2.tar.gz, and the junk file `oom`. The full bundle supersedes them.
- handoff/: copy of /workspace/handoff-b4134af-r3 without elasticdump-rs-b4134af.bundle, which the full bundle supersedes. Its own SHA256SUMS still lists that bundle.
- RESUME-FOR-GROK-BOT.md: how to restore and resume.
- SHA256SUMS: checksums for every file except itself.
- Contains no credentials.
- 2026-10-31: project/plan/PROGRESS.md and project/handoff/plan/PROGRESS.md updated through L-646 + L-644a (handoff/SHA256SUMS line for plan/PROGRESS.md updated; its bundle line is still stale, as noted above). RESUME-FOR-GROK-BOT.md now says to work on `main` and push to this fork.
