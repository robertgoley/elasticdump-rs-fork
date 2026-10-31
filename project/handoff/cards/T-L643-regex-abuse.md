---
id: T-L643
plan_task: L-643
lane: code
agent: impl
requirements: [TST-BRK-CLI-004]
goal: Huge or broken regexes in --renameIndex/--templateRegex fail with a clear compile error and size hint; replacement references to missing groups are an error; the policy name filter never silently treats a build failure as no-match
depends_on: [G-00]
timeout_min: 60
branch: card/T-L643
summary: results/cards/T-L643.md
files_allowed:
  - tests/chaos_local_l643.rs
  - src/rename.rs
  - src/lib.rs
  - src/meta/policy/mod.rs
files_readonly:
  - tests/support/cli.rs
  - src/rename.rs:1-140
  - src/lib.rs:140-170
  - src/meta/policy/mod.rs:20-100
  - src/meta/policy/mod.rs:415-426
forbidden:
  - change how a valid rename maps names (index names feed URLs; INJ-F2 validation must still run on the result)
  - new dependency (regex is already one), unsafe, #[allow], #[ignore], weakened or deleted test
acceptance:
  - run: cargo test -q --test chaos_local_l643
    exit: 0
    expect: all abuse cases pass
  - run: cargo test -q --lib rename
    exit: 0
    expect: existing rename unit and proptests green
  - run: cargo test -q --test injection
    exit: 0
    expect: INJ-F2 guards unchanged
---
## Context
TST-BRK-CLI-004 (should): "Regex abuse in `--renameIndex`, `--templateRegex`: huge patterns (`(a{1000}){1000}`), 1 MB
pattern, invalid UTF-8 escapes, capture references to missing groups (`$9`), `${name}` typos | `rename.rs`
(`Regex::new`), `lib.rs:457`; `meta/policy/mod.rs:35` (`is_ok_and` silently treats a build failure as no match) | Clear
compile error with a size-limit hint; missing captures are an error | bug hunted: silent empty replacements; silent
non-match in the policy pattern filter".

## Spec
1. Test cases via the binary (usage error → exit 2, before any request). Rename rules only apply file → cluster
   (lib.rs:145-161), so use `--input <tmpfile> --output http://127.0.0.1:9` (dead port, host only, no index):
   `(a{1000}){1000}`, a 1 MB pattern, `\xff` escape, `$9` with one group, `${nmae}` with group `name`.
   Each: exit 2, stderr names the flag and (for size) contains "size limit". For `--templateRegex` use
   `--type template --input http://127.0.0.1:9 --output <tmpfile>` (dump side).
2. rename.rs: when building, check every `$N`/`${name}` in the replacement against `captures_len()` / `capture_names()`;
   unknown → usage error naming the reference. Keep regex's default size limit; map its error to a message with the hint
   "pattern too large (regex size limit); simplify it".
3. policy/mod.rs:35 (`glob`): replace `is_ok_and` with an explicit match; a build failure (e.g. size limit on a huge
   comma list) must surface as an error to the caller (change the return type to `Result<bool>` and update callers in
   the same file). Log-and-skip is not ok. Update the assertion at policy/mod.rs:424 only by adding `.unwrap()`, and add a
   unit test in that tests module: a 1 MB comma list pattern returns `Err`.
4. Valid patterns behave exactly as before (existing tests prove it).

## Output contract
One commit `T-L643: TST-BRK-CLI-004: regex abuse is a clear usage error` + results/cards/T-L643.md.
Review focus for the review agent: rename output still passes `validate_index_name`.
