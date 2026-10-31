---
id: T-L652
plan_task: L-652
lane: code
agent: impl
requirements: [TST-BRK-ILM-002]
goal: Odd but valid ILM policies convert to ISM with the right state order and transitions (unit tests); unit-conversion bugs are fixed
depends_on: [T-L651]
timeout_min: 60
branch: card/T-L652
summary: results/cards/T-L652.md
files_allowed:
  - src/meta/policy/ilm_to_ism.rs
files_readonly:
  - src/meta/policy/ilm_to_ism.rs
forbidden:
  - change behaviour for inputs the existing tests cover
  - new dependency, unsafe, #[allow], #[ignore], weakened or deleted test
acceptance:
  - run: cargo test -q --lib meta::policy::ilm_to_ism
    exit: 0
    expect: old and new unit tests pass
  - run: cargo nextest run --profile unit
    exit: 0
    expect: unit tier green (includes the L-507 proptests)
---
## Context
TST-BRK-ILM-002 (must): "Odd but valid ILM: phases out of JSON order; `min_age` in every unit (`0ms`, `30s`, `1d`,
`1000d`); missing `min_age`; empty `phases`; only a `delete` phase; rollover-only hot phase; `_meta` with nested
objects; unicode policy names | `convert` | Correct state order and transitions; ISM accepts them | bug hunted:
transition units mis-converted (e.g. `ms`, `micros`, `nanos`) | live PUT on OS 1.3/2.19/3.x". This card is the unit half;
the live PUT is a follow-up campaign. Empty `phases` is already refused ("no convertible phases") — keep that.

## Spec
1. Add tests inside the existing `#[cfg(test)] mod tests` of ilm_to_ism.rs (append at the end of the module):
   - phases given as delete, warm, hot in JSON order → states hot, warm, delete; transitions point forward only;
   - `min_age` each of `0ms 30s 1d 1000d 90m 12h 500micros 7nanos` → ISM `min_index_age` keeps value and unit
     (ISM accepts d h m s ms micros nanos); missing `min_age` → treated as 0 (no transition delay);
   - only a delete phase; rollover-only hot phase; `_meta` with nested objects copied as-is; policy name `politique-é-日本`.
2. A failing case is a converter bug: fix it in the conversion code with the smallest change and list it under Findings.

## Output contract
One commit `T-L652: TST-BRK-ILM-002: odd but valid ILM converts correctly` + results/cards/T-L652.md.
