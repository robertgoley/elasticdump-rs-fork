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
