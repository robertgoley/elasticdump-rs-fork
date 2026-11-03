# Toolchain integrity — 2026-11-02, Grok Bot

Box: `~/.cargo/bin`, `~/.local/bin`. Method: download the official release asset, verify its published checksum, then
`cmp` the extracted binary with the installed one.

| Tool | Installed | Check | Result |
|---|---|---|---|
| zizmor | 1.31.1 (`~/.cargo/bin`) | GitHub release asset digest `sha256:d185f726…8c99` (API) → `sha256sum -c` OK | **byte-identical** to release binary |
| gitleaks | 8.28.0 (`~/.local/bin`) | `gitleaks_8.28.0_checksums.txt` → OK | **byte-identical** |
| actionlint | 1.7.12 (`~/.cargo/bin`) | `actionlint_1.7.12_checksums.txt` → OK | **byte-identical** |
| rustc/cargo | 1.85.1 + nightly via rustup | rustup verifies component tarball SHA-256 from the signed channel manifest at install; installed files carry no hashes | **not verifiable after install** without re-downloading (~300 MB) |
| cargo-deny 0.20.2, cargo-audit 0.22.2 | `~/.local/bin` (prebuilt) | not compared (time/usage budget) | not verified |
| cargo-vet 0.10.3, cargo-nextest 0.9.150, cargo-geiger, cargo-llvm-cov, cargo-supply-chain, cargo-cyclonedx, cargo-machete, cargo-udeps, cargo-fuzz | `~/.cargo/bin` | cargo-install builds verify crate checksums at build time (`.crates2.json` lists geiger, llvm-cov, supply-chain, rustfilt); binstall'd ones would need release-asset compares | not verified |

**Suspicious findings: 0** (3/3 checked tools identical to upstream).

## Follow-ups
- Toolchain: `rustup toolchain install 1.85.1 --profile minimal` into a temp `RUSTUP_HOME` and `diff -r` against the installed
  one, or rely on CI's fresh install (dtolnay/rust-toolchain) and compare build outputs (reproducibility, B11).
- Compare cargo-deny / cargo-audit / cargo-nextest / cargo-vet against their GitHub release checksums with the same loop.
- CI already checksum-pins grype/syft/trivy/gitleaks/zizmor; taiki-e/install-action verifies its own manifest checksums.
