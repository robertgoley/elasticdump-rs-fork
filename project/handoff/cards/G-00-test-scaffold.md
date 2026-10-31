---
id: G-00
plan_task: none
lane: code
agent: lead
requirements: [TST-INF-001]
goal: Per-card chaos test files (tests/chaos_local_<id>.rs) run in the default and chaos tiers, with shared CLI helpers, so parallel card branches never edit the same file
depends_on: []
timeout_min: 30
branch: card/G-00
summary: results/cards/G-00.md
files_allowed:
  - .config/nextest.toml
  - tests/support/mod.rs
  - tests/support/cli.rs
  - tests/chaos_local_g00.rs
files_readonly:
  - tests/chaos_brk.rs:1-68
forbidden:
  - done by the lead session by hand (touches .config/); not an impl card
acceptance:
  - run: cargo nextest list --profile chaos 2>/dev/null | grep -q chaos_brk
    exit: 0
    expect: chaos tier still lists chaos_brk
  - run: cargo nextest run --profile contract
    exit: 0
    expect: contract tier unchanged and green
---
## Why
Every A6 chaos test lives in `tests/chaos_brk.rs` (2,321 lines), which agent A on the owner's box is still appending
to. Parallel card branches appending to the same file conflict on every merge. One test binary per card avoids that.

## Spec (the lead session does this by hand, one commit `G-00: TST-INF-001: per-card chaos test binaries`)
1. `tests/support/cli.rs`: move-free copy of the helpers at `tests/chaos_brk.rs:1-68` (`bin()`, `run_async`, `s`,
   `stderr`, `hit`, `page`, `count`, `dir_names`, consts `INFO`, `CLEARED`) as `pub` items. Do not edit chaos_brk.rs.
2. `tests/support/mod.rs`: add `pub mod cli;` (module already has `#![allow(dead_code)]`).
3. `.config/nextest.toml`: add `| binary(/^chaos_local_/)` to `[profile.default].default-filter` and
   `[profile.chaos].default-filter`. No other change.
4. Add an empty smoke file `tests/chaos_local_g00.rs` with one test that runs `bin().arg("--help")` → exit 0,  Check `cargo nextest list --profile chaos` lists it.
5. Record the commit in the progress log (§9 of HANDOFF-GROK-OPENCODE.md). Cards depending on G-00 can then run.
