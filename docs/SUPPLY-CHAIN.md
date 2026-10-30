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
  `large_scale_test` and `manual_test`) gate tests only; none of them changes the shipped binary. The one
  shipped feature is `encrypted-keys` (default; SUP-023).

Every non-default feature enabled in `Cargo.toml` is listed below with its reason.

| Crate | Feature | Why |
|---|---|---|
| tokio | `rt-multi-thread`, `macros` | `#[tokio::main]` multi-thread runtime; `tokio::select!` in the pipeline and signal handling |
| tokio | `fs`, `io-util`, `io-std` | async file input/output; stdin/stdout as `$` endpoints |
| tokio | `sync`, `time` | channels and semaphores between reader and writers; retry backoff and throttling |
| tokio | `signal` | SIGINT/SIGTERM handling (staging-file cleanup) |
| reqwest | `gzip` | transparent decompression of gzip-encoded responses |
| reqwest | `rustls-tls-manual-roots` | rustls only, and we choose the trust roots (`--ca`, native, webpki); no OpenSSL (deny.toml bans it) |
| rustls | `ring`, `std`, `tls12` | crypto provider without aws-lc/cmake; TLS 1.2 for older clusters |
| rustls | `logging` | rustls handshake diagnostics under `RUST_LOG` (header values are marked sensitive; see H-10) |
| pkcs8 (optional) | `encryption`, `pem`, `std` | encrypted PKCS#8 client keys (`--key` with a passphrase); only with the crate feature `encrypted-keys` (default on) |
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
- **Shipped graph:** normal and build dependencies of `elasticdump-rs` on x86_64/aarch64 Linux, 26 build scripts and 10 proc macros. This is the feature-resolved graph from `cargo tree`; L-213 refined the over-approximated metadata closure, which had given 32/14.

Each entry is pinned as `name@version` and carries a review note.

### Gate

`scripts/sup/build-inventory.py` runs on every PR. It fails when a build script or proc macro is new to the shipped graph, or when such a package changes version. After review, refresh the baseline with `--update`.

### Network-less build

`scripts/sup/offline-build.sh` builds without network access:

1. It vendors the dependencies on the host. This is the only step that downloads anything.
2. It runs `cargo build --offline --locked --release` in `podman run --network none`, with the sources and the vendor directory mounted read-only.
3. Before building, a probe inside the sandbox checks that DNS and TCP really fail.

Verified locally on 2026-10-29 with `rust:1.99-trixie`: the probe showed the network unreachable, and the build succeeded in 2m01s. No build script needs the network.

## SBOM diff per PR (SUP-011, L-207)

`scripts/sup/sbom-pr.sh <base-ref>` checks the base out into a temporary worktree and compares it with HEAD. It builds the following for both sides:

- a CycloneDX SBOM of the Linux graph, with `cargo cyclonedx --target x86_64-unknown-linux-gnu`;
- the build-time inventory.

`scripts/sup/sbom-diff.py` then reports:

- crates added, removed, upgraded or downgraded;
- licence changes;
- new build scripts and proc macros;
- image package changes, when `IMAGE_SBOM_BASE`/`IMAGE_SBOM_HEAD` point to syft SBOMs.

The `sbom-diff` job in `security.yml` runs on every PR. It attaches the diff as an artefact and adds it to the job summary. A non-empty diff fails the job until the PR carries the `sbom-reviewed` label.

Not covered: unsafe deltas. `scripts/sup/geiger-gate.sh` in the `supply-chain` job catches those.

## Native-code allow-list (SUP-021, L-213)

`scripts/sup/native-code-check.py` runs in the `supply-chain` job. It inspects the feature-resolved Linux graph of the shipped binary and flags any crate that has:

- a `links =` key;
- a `cc`, `cmake`, `bindgen` or `pkg-config` build dependency;
- C, C++ or assembly sources.

Any flagged crate that is not on the allow-list fails the job. The allow-list is `ring` (BoringSSL-derived C/asm) and `zstd-sys` (vendored libzstd).

Verified: adding `libz-sys` fails the check. `deny.toml` also bans it, so `cargo deny` fails as well.

## Secret scanning and canaries (SUP-015, L-210)

`scripts/sup/secret-scan.sh` runs three checks:

1. gitleaks 8.28.0 over the whole git history, or only the PR's commits;
2. gitleaks over the tracked files;
3. the `tests/secrets.rs` canary suite.

The `secret-scan` job in `security.yml` runs it on every push and PR.

Results on 2026-10-29:

- 137 commits scanned, no findings.
- Tracked tree clean.
- Canary suite: 3/3 tests pass.
- Negative check: a planted `ghp_…` token is caught, exit 1.

Untracked `target/` holds 44 "private-key" hits. These are test PEMs that dependencies embed in build output. They are not scanned, because `target/` is ignored and never committed.

## Image hardening tests (SUP-017, L-211)

`make test-image` runs `scripts/test-image.sh <image> <variant>`. It checks:

- `--version` and `--help` work;
- bash, the `elasticdump` symlink and the CA bundle are present;
- the image runs as non-root (uid 65532), with working directory `/data`;
- a file-to-file dump into a writable `/data` volume works under `--read-only --cap-drop ALL --security-opt no-new-privileges --network none`;
- the root filesystem cannot be written under `--read-only`;
- the image contains no executable outside the documented set in `docker/image-binaries.<variant>.txt`;
- the base image label is pinned by digest.

The executable baselines were reviewed on 2026-10-29 from images rebuilt at L-211:

| Variant | Executables |
|---|---|
| `dhi-alpine` | 62 |
| `alpine` | 68 |
| `dhi-debian` | 300 |
| `public` | 409 |

All four variants pass. Negative check: testing the old dhi-debian image against the dhi-alpine baseline fails, both on undocumented executables and on a base that is not pinned by digest.

After a reviewed base change, refresh a baseline with `scripts/test-image.sh <image> <variant> --update`.

## Publisher monitoring (SUP-019)

`supply-chain/publishers.json` records the crates.io owners of every dependency (220 crates, 142 distinct
publishers at the 2026-10-30 snapshot, `cargo supply-chain` 0.3.8). The weekly `publishers` job in
`security.yml` re-queries crates.io and runs `scripts/sup/publishers.py diff`; it fails (exit 10) when an
existing crate gains or loses an owner, so a takeover or hand-over gets a human look before the next release.
New or removed crates are listed but do not fail the job; the SBOM diff (SUP-020) covers them. After review,
refresh the snapshot with `cargo supply-chain json > sc.json && scripts/sup/publishers.py snapshot sc.json`.

## Non-hot unsafe trimmed (SUP-023, L-214)

| Item | Decision | Effect |
|---|---|---|
| reqwest `http2` | **Removed.** Elasticsearch and OpenSearch serve HTTP/1.1 only, and Node elasticdump is HTTP/1.1 only, so h2 was only reachable through an h2-capable proxy, where HTTP/1.1 also works. | `h2` and its deps leave the graph (Cargo.lock −29 lines); geiger used-unsafe 27,289 → 27,245, crates with unsafe 111 → 110 |
| pkcs8 `encryption` (aes, scrypt, pbkdf2, sha2, salsa20, cbc) | Behind the crate feature `encrypted-keys`, **on by default** so `--key` + `--pass` keeps working; `--no-default-features` builds leave the stack out and an encrypted key then fails with a message naming the feature and the `openssl pkcs8` conversion. | minimal build: 25,774 used-unsafe across 162 packages (default 27,245 / 186) |
| env_logger `humantime` (jiff) | **Kept.** Dropping it removes the timestamp from every log line, a visible output change. | — |
| indicatif → portable-atomic, console, terminal_size | **Kept.** portable-atomic is a non-optional indicatif dependency; the progress bar is a user-facing feature. | — |

The geiger baseline (`supply-chain/geiger-baseline.json`) was lowered to the new total, so the gate now holds the
reduction.
