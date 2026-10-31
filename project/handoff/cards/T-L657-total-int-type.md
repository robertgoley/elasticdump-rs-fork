---
id: T-L657
plan_task: L-657
lane: code
agent: impl
requirements: [TST-BRK-XV-006]
goal: A dump from ES 6.x-shaped responses (hits.total as an integer, _type in hits) completes with every document; behaviour of _type is pinned or reported
depends_on: [G-00]
timeout_min: 45
branch: card/T-L657
summary: results/cards/T-L657.md
files_allowed:
  - tests/chaos_local_l657.rs
files_readonly:
  - tests/support/cli.rs
  - tests/support/router_mock.rs:200-440
  - tests/chaos_brk.rs:1764-1800
  - NODE-DIFFERENCES.md
forbidden:
  - edit src/ (extract.rs and the bulk encoder are on the lead-only security list)
  - new dependency, unsafe, #[allow], #[ignore], weakened or deleted test
acceptance:
  - run: cargo test -q --test chaos_local_l657
    exit: 0
    expect: dump cases pass
  - run: git diff --quiet rewrite...HEAD -- src
    exit: 0
    expect: no product change
---
## Context
TST-BRK-XV-006 (should): "`hits.total` as an int (ES 6.x-shaped goldens, `rest_total_hits_as_int`) and `_type`
present in hits on 7.x for old indices | `extract.rs`, `retrieval_task.rs::extract_total_hits_estimate` | Handled | bug
hunted: progress/total confusion | golden". F-408 (L-408) already fixed integer totals for scroll pages; this pins it
end to end and covers `_type`.

## Spec
1. Router mock: INFO for ES 7.17 (`{"version":{"number":"7.17.29"}}`); first `POST /idx/_search` page with
   `"hits":{"total":5,"hits":[...]}` (integer) holding 3 hits that each carry `"_type":"_doc"`; scroll page with 2 more
   hits (`"_type":"doc"`), then an empty page; fallback `CLEARED`.
2. Dump to a tempfile: assert exit 0, 5 lines, ids 1..5, no "retrieved N of M" error, no "invalid type".
3. Record whether `_type` is kept in the output lines. Compare with NODE-DIFFERENCES.md (Node writes hits as served).
   Assert the observed behaviour only if it matches Node or a documented difference; otherwise do not assert it and
   report it under Findings.
4. Repeat 2 with `--searchAfter` if the mock can serve it with the same pages; skip with a note if not.

## Output contract
One commit `T-L657: TST-BRK-XV-006: integer totals and _type handled` + results/cards/T-L657.md.
