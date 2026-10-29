## Summary

## New or changed dependencies (SUP-014). Delete this section if none.

For each crate added or upgraded across a major version, answer every item:

- [ ] **Purpose.** Why we need it, and why std or an existing dependency is not enough.
- [ ] **Alternatives** considered.
- [ ] **Maintainers and owners.** `cargo vet` / `cargo supply-chain publishers`. Note any new publisher.
- [ ] **Age, downloads, release cadence.** At least 7 days since the release (cooldown).
- [ ] **Unsafe.** `scripts/sup/geiger-gate.sh` delta.
- [ ] **Build-time code.** Any `build.rs` or proc macro? `scripts/sup/build-inventory.py`.
- [ ] **Native code.** Any `links =`, `cc`/`cmake`/`bindgen`, or C/asm sources? `scripts/sup/native-code-check.py`.
- [ ] **Network or file access** at build time or runtime.
- [ ] **Licence** allowed by `deny.toml`.
- [ ] `cargo deny check` and `cargo vet` pass. The SBOM diff has been reviewed, and the `sbom-reviewed` label added.
