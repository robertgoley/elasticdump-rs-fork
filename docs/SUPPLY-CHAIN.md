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

## Unused dependencies and enabled features (SUP-007, L-204)

- `cargo machete` runs in the `supply-chain` job on every PR and push. `rust-ini` is in
  `[package.metadata.cargo-machete] ignored`: it is imported as `ini`, which machete does not match to the package
  name. `cargo +nightly udeps --workspace --all-targets --features elasticdump-rs/load` runs weekly in
  `security.yml`; it must compile the `it`/`load` tests, otherwise `lipsum` (used only by
  `tests/integration_test.rs`) is reported as unused. L-204 removed `thiserror`, which was declared but never used.
- No `cargo hack` on the box. The crate's own features (`it`, `load`, `chaos`, `fuzz`, and the deprecated aliases
  `large_scale_test` and `manual_test`) gate tests only; none of them changes the shipped binary.

Every non-default feature enabled in `Cargo.toml` is listed below with its reason.

| Crate | Feature | Why |
|---|---|---|
| tokio | `rt-multi-thread`, `macros` | `#[tokio::main]` multi-thread runtime; `tokio::select!` in the pipeline and signal handling |
| tokio | `fs`, `io-util`, `io-std` | async file input/output; stdin/stdout as `$` endpoints |
| tokio | `sync`, `time` | channels and semaphores between reader and writers; retry backoff and throttling |
| tokio | `signal` | SIGINT/SIGTERM handling (staging-file cleanup) |
| reqwest | `gzip` | transparent decompression of gzip-encoded responses |
| reqwest | `http2` | servers that negotiate h2 over ALPN |
| reqwest | `rustls-tls-manual-roots` | rustls only, and we choose the trust roots (`--ca`, native, webpki); no OpenSSL (deny.toml bans it) |
| rustls | `ring`, `std`, `tls12` | crypto provider without aws-lc/cmake; TLS 1.2 for older clusters |
| rustls | `logging` | rustls handshake diagnostics under `RUST_LOG` (header values are marked sensitive; see H-10) |
| pkcs8 | `encryption`, `pem`, `std` | encrypted PKCS#8 client keys (`--key` with a passphrase) |
| clap | `derive`, `wrap_help` | CLI definition; readable `--help` |
| serde | `derive` | config and wire types |
| serde_json | `preserve_order`, `arbitrary_precision` | byte-faithful `_source` (key order, big integers and decimals unchanged) |
| flate2 | `zlib-rs` (no default) | pure-Rust deflate; no `libz-sys` (banned) |
| zstd | no default features | no legacy/zdict builder; the `zstd-sys` C code is on the native allow-list |
| regex | `std`, `unicode-perl`, `unicode-case` (no default) | template/rename patterns with `\w`/`\d` and `(?i)`; the rest of the Unicode tables and the perf features are off |

## Build-time code (SUP-009, L-206)

### Inventory

`supply-chain/build-inventory.json` lists every package that runs code at build time: build scripts and proc macros.

- **All of Cargo.lock:** 45 build scripts and 18 proc macros.
- **Shipped graph:** normal and build dependencies of `elasticdump-rs` on x86_64/aarch64 Linux, 32 build scripts and 14 proc macros. This is an over-approximation: `cargo metadata` unifies features across the workspace.

Each entry is pinned as `name@version` and carries a review note.

### Gate

`scripts/sup/build-inventory.py` runs on every PR. It fails when a build script or proc macro is new to the shipped graph, or when such a package changes version. After review, refresh the baseline with `--update`.

### Network-less build

`scripts/sup/offline-build.sh` builds without network access:

1. It vendors the dependencies on the host. This is the only step that downloads anything.
2. It runs `cargo build --offline --locked --release` in `podman run --network none`, with the sources and the vendor directory mounted read-only.
3. Before building, a probe inside the sandbox checks that DNS and TCP really fail.

Verified locally on 2026-10-29 with `rust:1.99-trixie`: the probe showed the network unreachable, and the build succeeded in 2m01s. No build script needs the network.
