# AGENTS.md — elasticdump-rs (for the OpenCode agents impl, runner, review — Grok 4.7)

You work on ONE task card inside ONE git worktree. The card is the spec. Do only what it says.

## Repo map (Rust 2024, workspace)
- `src/` library + binary `elasticdump-rs` (Rust rewrite of Node elasticdump). Our crate forbids `unsafe`.
  - `cli.rs` flags (clap) · `lib.rs` validate/dispatch · `output.rs` stdout/file sink
  - `pipeline/` bulk encoder, file source/sink, NDJSON split · `retrieval/` scroll/PIT/search_after
  - `http/` client, retry, TLS · `meta/` mapping/settings/alias/template/policy (`policy/ilm_to_ism.rs`)
  - `rename.rs` index renaming · `secrets.rs` redaction · `endpoint.rs` URL parsing
- `tests/` integration-style tests that run the binary. Helpers in `tests/support/` (`router_mock.rs`, `mock_es.rs`, and `cli.rs` once G-00 is merged).
  - `chaos_brk.rs` (A6 chaos, mocks only) · `cli_contract.rs` · `injection.rs` · `secrets.rs`
  - `tests/it/` live-engine suites (need containers; do not run unless the card says so)
- `tools/edgen` (data generator), `tools/edverify` (count + multiset digest).
- `scripts/` harness: `it/run.sh` (engines), `load/run.sh` (loads), `fuzz/smoke.sh`, `res/*` (guards).
- Requirement IDs (`TST-BRK-...`, `TST-LOAD-...`) come from TEST-REQUIREMENTS.md. The card quotes what you need.

## Commands (use these exact forms)
- Build: `cargo build -q`
- One test file: `cargo test -q --test <name>` · one unit module: `cargo test -q --lib <path::to::mod>`
- Tiers: `cargo nextest run --profile unit` · `--profile contract` · `--profile chaos`
- Format: `cargo fmt --all`
- Lint: `cargo clippy -q --workspace --lib --bins --tests --features elasticdump-rs/load,elasticdump-rs/it -- -D warnings`
- Keep `CARGO_BUILD_JOBS` as set in the environment. Never start containers yourself.

## Rules
1. Edit only the files listed under `files_allowed`. Read-only files: read, never write.
2. Never touch: `Cargo.toml`, `Cargo.lock`, `deny.toml`, `supply-chain/`, `.github/`, `.config/`, `build.rs`, `tests/golden/`, `oom`, `test_query_elasticdump_rs_test_857109_16.json`.
3. No new dependencies, no `unsafe`, no `#[allow(...)]`, no `#[ignore]` on existing tests, no weakened assertions, no deleted tests.
4. No network code: no new URLs, sockets, telemetry or HTTP clients in `src/`. Tests use the in-process mocks only.
5. Never print or log secrets (passwords, tokens, `user:pw@` URLs). Tests use canary values and assert they are absent.
6. Tests write only to temp dirs (`tempfile::tempdir()`), never into the repo.
2a. Security-sensitive paths are lead-only even if a card lists them by mistake: the bulk/NDJSON encoder (`src/bulk.rs`, `src/pipeline/*` encode path, `extract.rs`), injection guards (index/id/URL segment validation, `url_segments`, rename output validation), credential redaction / `secrets.rs` / `--httpAuthFile`, TLS and proxies, `src/http/retry.rs` and timeouts, `unsafe`/FFI. You may write tests against them; mark the finding with `// FINDING:`.
7. If the card's behaviour needs a change in a file you may not edit, STOP: write the finding in the summary and commit only the test (mark it with a `// FINDING:` comment, do not `#[ignore]` it unless the card says so).
8. If you are stuck after 3 failed attempts at the same step, stop and report. Do not loop.

## Output contract (every card)
1. Exactly ONE commit on the current branch: `<CARD-ID>: <requirement IDs>: <summary>`.
2. Commit includes `results/cards/<CARD-ID>.md` with these sections:
   - `## Result` one of `pass | fail | partial | blocked`
   - `## Changes` file list, one line each
   - `## Commands` each acceptance command, its exit code, and PASS/FAIL against the expected result
   - `## Findings` bugs found, behaviour you could not fix, questions for the reviewer
3. Do not create branches, push, amend, rebase or switch branches.
