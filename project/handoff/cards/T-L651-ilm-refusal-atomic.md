---
id: T-L651
plan_task: L-651
lane: code
agent: impl
requirements: [TST-BRK-ILM-001]
goal: A policy restore with --convertIlmToIsm writes nothing when any policy is refused (strict), and writes all with DROPPED notes when lenient
depends_on: [G-00]
timeout_min: 60
branch: card/T-L651
summary: results/cards/T-L651.md
files_allowed:
  - tests/chaos_local_l651.rs
  - src/meta/policy/mod.rs
files_readonly:
  - tests/support/cli.rs
  - tests/support/router_mock.rs:200-440
  - src/meta/policy/mod.rs
  - src/meta/policy/ilm_to_ism.rs:100-130
  - src/meta/policy/ilm_to_ism.rs:474-540
forbidden:
  - change conversion rules in ilm_to_ism.rs (already unit- and proptested: L-507)
  - new dependency, unsafe, #[allow], #[ignore], weakened or deleted test
acceptance:
  - run: cargo test -q --test chaos_local_l651
    exit: 0
    expect: strict and lenient cases pass
  - run: cargo test -q --lib meta::policy
    exit: 0
    expect: existing policy unit tests green
---
## Context
TST-BRK-ILM-001 (must): "Unsupported phases and actions (...) with and without `--lenientPolicyConversion` |
`ilm_to_ism.rs::convert` | Refused naming phase and action; lenient mode drops with `DROPPED …`; **no partially written
policy set** | OpenSearch accepts every converted policy | proptest + live". The refusal messages and DROPPED notes are
already unit-tested (ilm_to_ism.rs tests). Missing: the end-to-end "no partial set" check. Live OpenSearch acceptance is
a separate campaign card (the lead queues it after this one).

## Spec
1. Find the policy file format and the ISM write requests in src/meta/policy/mod.rs (Node-compatible dump of ILM
   policies; ISM endpoint `_plugins/_ism/policies/<name>`).
2. Test: file with 3 ILM policies `ok1`, `bad` (cold `searchable_snapshot`), `ok2`; router mock = OpenSearch 2.19 INFO,
   GET of each policy → 404, PUT → 201. Run `--type policy --convertIlmToIsm --input <file> --output <mock>`.
   Strict: exit ≠ 0, stderr names `'bad'` and `searchable_snapshot`, and the mock recorded ZERO PUT requests.
   Lenient (`--lenientPolicyConversion`): exit 0, 3 PUTs, stderr has a `DROPPED` line for `bad`.
3. If strict mode writes `ok1` before refusing `bad`, fix in policy/mod.rs: convert every policy first, write only when
   all converted.

## Output contract
One commit `T-L651: TST-BRK-ILM-001: refused policy sets are never written partially` + results/cards/T-L651.md.
