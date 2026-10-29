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
