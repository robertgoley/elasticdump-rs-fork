# elasticdump-rs handoff (b4134af)

> **Read `DECISIONS.md` first:** Robert confirmed the open design decisions on 2026-10-30 and added work items D-1 to D-5.

Local fork only. Do not push or open PRs.

**Then read HANDOFF-GROK-OPENCODE.md.** The local setup is **only Grok 4.7 driven through the OpenCode CLI** (xAI
provider, model `xai/grok-4.7`; confirm the id with `opencode models xai`). There is no local model or server. The guide
covers bootstrap, the four agents (`impl`, `runner`, `review`, `lead`), the work queue, the review gate and the constraints.
Then:
- opencode/: opencode.json (provider xai, no API key in the file; auth via `opencode auth login` or XAI_API_KEY), AGENTS.md,
  prompts/ (impl, runner, review, lead), and scripts/ (run-card.sh, gate.sh, card.py, smoke-grok.sh).
- cards/: CARD-TEMPLATE.md, G-00 (lead, first), and ready cards (T-B0, T-B1, T-B2a, T-B7, T-L643, T-L651, T-L652,
  T-L657). Branches are `card/<id>`, summaries `results/cards/<id>.md`, artefacts `target/cards/<id>/`.

## Restore the repo
    git clone elasticdump-rs-b4134af.bundle elasticdump-rs
    cd elasticdump-rs && git checkout rewrite
    git log --oneline -1   # expect b4134af

## What to read
- plan/HANDOFF-TASKPLAN.md: tasks B0-B13 for the higher-resource agent, gates H0-H4.
- plan/PROGRESS.md: local tasks done, commits, results, deferrals (items marked B / owner).
- plan/TASKS-LOCAL.md: the full local list (L-xxx); anything not in PROGRESS.md is not done yet.
- plan/TEST-REQUIREMENTS.md, PLAN.md, GAPS.md, TYPES-PLAN.md: requirements and background.
- In the repo: TESTING-MATRIX.md, SECURITY-CVE.md, NODE-DIFFERENCES.md, docs/SUPPLY-CHAIN.md, results/local/.

## Tooling in the repo
- scripts/load/run.sh <tier> <scenario>: load runner (L0 to L2 tiers; L2 needs more resources than the box had).
- edgen (seeded data generator), edverify (document-set verifier), scripts/data/datasets.toml + fetch.sh.
- Unpinned datasets (pmc, nyc_taxis, big5, eventdata, OSB 181998, loghub) are for task B1.
- cargo nextest tiers: unit, contract, it, load, chaos.

## Needs on the new machine
- podman with cgroup v2 delegation (memory limits enforced), Rust 1.99, nightly for geiger and udeps.
- dhi.io login (podman login dhi.io) for the default images; credentials not included.
- Engine images listed in images.lock.


## Status
A0-A5 done; A6 done through L-642 (HEAD b4134af). The original agent has stopped. The local Grok/OpenCode setup owns
everything from **L-643** on, plus A7, A8, the B tasks and D-1..D-5 (DECISIONS.md). Start with G-00 (lead), then T-L643.
H4 gate (after A7) not reached. Cards for tasks already done were removed; run-card.sh refuses any task that already
has a commit (exit 3).
