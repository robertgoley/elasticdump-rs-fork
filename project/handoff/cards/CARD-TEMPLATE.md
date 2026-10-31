---
# Front matter is machine-read by opencode/scripts/card.py (restricted YAML: scalars, [a, b], "- item").
id: T-XXXX                      # T-<plan id>; G-.. for lead tasks
plan_task: L-XXX                # L-/B- id from the plan (run-card.sh skips L-tasks already in PROGRESS.md)
lane: code                      # code | campaign (campaign: run-card.sh runs `campaign` first, runner summarises)
agent: impl                # impl | runner | lead (lead cards are not run by run-card.sh)
requirements: [TST-XXX-000]
goal: one sentence, observable outcome
depends_on: [G-00]              # IDs whose "<ID>: ..." commit must be on rewrite
timeout_min: 45                 # wall clock for the opencode run
campaign_timeout_min: 0         # per campaign command (campaign lane only)
heavy_gb: 0                     # >0: campaign runs under scripts/res/heavy.sh <GB> (engine lock + memguard)
branch: card/T-XXXX
summary: results/cards/T-XXXX.md
files_allowed:
  - tests/chaos_local_lXXX.rs
files_readonly:
  - tests/support/cli.rs
  - src/cli.rs:1-140             # path:line-range = read only these lines
forbidden:
  - edit any file not in files_allowed
  - new dependency, unsafe, #[allow], #[ignore], weakened or deleted test
  - network access, containers (code lane), git push/branch/rebase/amend
campaign:
  - echo "campaign lane only: exact shell commands, run in the worktree with $ART = artefact dir"
acceptance:
  - run: cargo test -q --test chaos_local_lXXX
    exit: 0
    expect: every test passes; no "panicked" in output
  - run: test -s results/cards/T-XXXX.md
    exit: 0
    expect: summary present
---
## Context (≤ 15 lines; quote the requirement row, do not paste plan files)

## Spec (numbered, testable)
1.

## Stop conditions
- If the fix needs a file outside files_allowed: commit the test with a `// FINDING:` comment, report, stop.

## Output contract
One commit `T-XXXX: TST-XXX-000: <summary>` containing the changes and results/cards/T-XXXX.md
(sections: Result, Changes, Commands with exit codes and PASS/FAIL, Findings).
