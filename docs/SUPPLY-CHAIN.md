# Supply-chain controls

How elasticdump-rs implements the SUP-* requirements (TEST-REQUIREMENTS.md §7.3).
Each section names the gate that enforces it and how to run it locally.

## cargo-vet (SUP-004, L-201)

- Config: `supply-chain/config.toml`, `audits.toml`, `imports.lock`.
- Imported audit sets: Mozilla, Google, Bytecode Alliance, Embark, ISRG, Zcash.
- Baseline (2026-10-29): 61 crates fully audited by imports, 2 partially, 207 exempted.
  Every exemption carries a note (`L-201 baseline …`). A new crate or a new version
  of an exempted crate fails `cargo vet` until it is audited
  (`cargo vet certify`) or an imported audit covers it.
- Gate: CI job `supply-chain`, step `cargo vet --locked`.
- Local: `cargo vet --locked`; refresh imports with `cargo vet regenerate imports`.
- Shrinking the exemption list is ongoing work: `cargo vet suggest` lists the
  cheapest audits.

## cargo-deny (SUP-005, L-202)

`deny.toml`, checked by `cargo deny check advisories bans licenses sources`
(workflow `security`). The graph is limited to the shipped Linux targets.

- **Sources:** crates.io only; unknown registries and git sources are denied.
- **Duplicates:** `multiple-versions = "deny"`. The skip list has three
  entries, each with the crate that pulls in the old version: base64 0.22,
  hashbrown 0.14 and getrandom 0.2.
- **Banned crates:** OpenSSL and native-tls, libgit2, curl, other HTTP clients,
  and `libz-sys`.
- **Build scripts** (`[bans.build]`): only the 27 crates listed under
  `allow-build-scripts` may have a `build.rs`. Each has a one-line reason;
  `ring` and `zstd-sys` are the only ones that compile native code. Any other
  crate with a build script fails the check until it is reviewed and added.
- **Executables and scripts:** `executables = "deny"` and `interpreted = "deny"`,
  checked in the allowed crates and their dependencies. Maintainer scripts that
  the build never runs are bypassed by exact version: `untrusted` `mk/*`, the
  `ring` perlasm generators, and the `zerocopy` CI scripts. The `ring` build
  only runs perl when built from a git checkout; the crates.io package ships
  pregenerated assembly.

Negative check (2026-10-29): adding `libz-sys` and `num-traits` reports
`banned`, two `build-script-not-allowed` errors and `detected-executable-script`.

## Unsafe inventory (SUP-006, L-203)

- `#![forbid(unsafe_code)]` is set in `src/lib.rs` and `src/main.rs`; the crate has no unsafe code.
- `scripts/sup/geiger-gate.sh` runs `cargo geiger` (Linux graph) and compares the result with
  `supply-chain/geiger-baseline.json`. It fails if:
  - our crate stops forbidding unsafe;
  - any crate's used-unsafe count rises;
  - a new crate brings unsafe code;
  - the total rises.

  After a reviewed dependency change, refresh the baseline with `--update`.
- Baseline (2026-10-29): 27,289 used-unsafe items in 111 of 191 crates. The largest counts are
  hashbrown, zlib-rs, tokio, memchr and sonic-rs.
- Known gap: cargo-geiger 0.14's parser cannot read `signal-hook-registry` 1.4.8 (Rust syntax it
  does not support), so that crate is reported as 0.
- CI: the `supply-chain` job, which needs nightly to build cargo-geiger.

## Test-only tools

`tools/edverify` and `tools/edgen` are workspace members, but they are never shipped: `default-members` is the root crate, and the image builds only `elasticdump-rs`. In cargo-vet they have `criteria = "safe-to-run"` (`[policy.edgen]`, `[policy.edverify]`), so `rand`, `rand_chacha` and `ppv-lite86` need only the safe-to-run level they already had as dev-dependencies.
