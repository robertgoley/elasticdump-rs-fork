# Handoff index

**Current handoff:** [HANDOFF-20261112.md](HANDOFF-20261112.md) — finish the beta1 aggregate, the ES 9 edge reruns, and the 1.1 point-in-time batch (2026-11-12).

The 1.1 batch details are on GitHub at
https://github.com/robertgoley/searchdump-rs/tree/release/1.1
(`origin/release/1.1`, HEAD `a93829a`, **UNCOMPILED / UNTESTED**). Fetch
with `git fetch origin release/1.1` and follow `project/HANDOFF-1.1.md` on
that branch. On this box the worktree is `/workspace/rel-1.1`. Push 1.1
work only to that branch; do not merge to `main` without Robert.
[HANDOFF-20261112.md](HANDOFF-20261112.md) is the resume point for another
Grok Bot.

## Historical handoffs

These describe earlier points in the work and are not the current resume point.

| Document | When | What it covered |
|---|---|---|
| [HANDOFF-perf-20261111.md](HANDOFF-perf-20261111.md) | 2026-11-11 | v1.0.0-beta1 draft release and the full L0 performance suite, including the usage-stop state after all 16 engines |
| [RESUME-FOR-GROK-BOT.md](RESUME-FOR-GROK-BOT.md) | 2026-11-01 (updated through L-808e) | Resume state on `main` after L-808d/e: queue, usage rule, open items |
| [handoff/README-HANDOFF.md](handoff/README-HANDOFF.md) | 2026-10-30 / 2026-11-01 | elasticdump-rs-fork bundle at `b4134af`; OpenCode agent setup |
| [handoff/HANDOFF-GROK-OPENCODE.md](handoff/HANDOFF-GROK-OPENCODE.md) | 2026-10-30 | OpenCode CLI agents (`impl`, `runner`, `review`, `lead`), cards and gates |
| [plan/HANDOFF-TASKPLAN.md](plan/HANDOFF-TASKPLAN.md) | 2026-10 | Task plan B0–B13 and gates H0–H4 for the higher-resource agent (a copy also lives at [handoff/plan/HANDOFF-TASKPLAN.md](handoff/plan/HANDOFF-TASKPLAN.md)) |
