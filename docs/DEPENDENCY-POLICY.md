# New-dependency policy (TST-SUP-014)

This policy covers adding, upgrading and replacing dependencies.

1. **Prefer none.** Use std or an existing dependency first. A new crate needs a reason in the PR template (`.github/pull_request_template.md`); every checklist item must be answered.
2. **Ownership.** `.github/CODEOWNERS` requires an owner review for changes to:
   - `Cargo.toml`, `Cargo.lock`, `deny.toml` and `supply-chain/`;
   - `.github/`, `.config/` and `scripts/`;
   - `Dockerfile`, `Makefile` and `images.lock`;
   - `tools/`, plus the upstream-origin `src/` and `tests/`.
3. **Automated gates.** Every PR runs these:

   | Gate | Job | What it checks |
   |---|---|---|
   | `cargo deny` | security | advisories, licences, bans, sources, `[bans.build]` |
   | `cargo vet --locked` | ci/supply-chain | every crate audited or exempted |
   | geiger gate | ci/supply-chain | unsafe count must not rise |
   | `cargo machete` | ci/supply-chain | no unused dependencies |
   | build-time inventory | security/offline-build | new `build.rs` or proc macro |
   | network-less build | security/offline-build | the build needs no network |
   | SBOM diff | security/sbom-diff | non-empty diff needs the `sbom-reviewed` label |
   | native-code check | ci/supply-chain | new C/asm code |

4. **Update bot.** Dependabot (`.github/dependabot.yml`) opens grouped weekly PRs for cargo patch and minor updates, GitHub Actions and the Docker bases. Its cooldowns:
   - 7 days before proposing a new release;
   - 30 days for a semver-major cargo release.

   Bot PRs go through the same gates and reviews.
5. **Pinning.** `Cargo.lock` is committed and every build uses `--locked`. Actions are pinned by SHA and images by digest (`images.lock`). Tools are installed with `cargo install --locked --version`.
6. **Removal.** A dependency that becomes unused is removed in the same PR; machete and udeps enforce this.

**Owner action needed:**

- Replace `@OWNER` in CODEOWNERS.
- Enable "Require review from Code Owners" in branch protection, and create the `sbom-reviewed` label.

Neither can be done from this local fork, because nothing is pushed.
