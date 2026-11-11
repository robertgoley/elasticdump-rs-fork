# Workflow hardening review (TESTING-TODO §2) — 2026-11-02, Grok Bot

Scope: `.github/workflows/{ci,nightly,release,security,soak}.yml` and `.github/dependabot.yml` at 37fa3ac.
Method: manual review with rg (zizmor not installed, skipped to save time/usage budget).

## Commands
```
rg -n 'uses:' .github/workflows | rg -v '@[0-9a-f]{40}'            # unpinned actions
rg -n -A3 '^permissions:|^\s+permissions:' .github/workflows
rg -n 'pull_request_target|workflow_run|issue_comment' .github/workflows
rg -n '\$\{\{\s*github\.(event|head_ref)' .github/workflows      # injection candidates
rg -n -o 'secrets\.[A-Z_]+' .github/workflows ; rg -c persist-credentials .github/workflows
gh api repos/robertgoley/searchdump-rs/branches/main/protection
```

## Results
| Check | Result | Verdict |
|---|---|---|
| Action pinning | 72 `uses:` across 6 actions (checkout v4.3.1, upload-artifact v4.6.2, Swatinem/rust-cache v2.9.3, dtolnay/rust-toolchain, taiki-e/install-action v2.88.0, mlugg/setup-zig v1.2.2) — **all pinned to 40-char SHAs** with version comments; 0 unpinned | good |
| `permissions:` | top-level `contents: read` in all 5 workflows; no job-level `write` grants | good (least privilege) |
| Dangerous triggers | no `pull_request_target`, `workflow_run`, `issue_comment` | good |
| Injection | 3 `${{ github.event... }}` uses (ci.yml:99, security.yml:202, :232) are SHAs (`base.sha`, `before`) passed via `env:` — not attacker-controlled strings, not interpolated into `run:` | benign |
| `inputs.*` | `workflow_dispatch` inputs (soak `tier`, security `base`) only via `env:` / `if:`; dispatch requires write access | benign |
| Secrets | only `DHI_USERNAME`/`DHI_TOKEN`, passed via `env:` into pull-only `login` steps in nightly/release/security; not available to fork PRs; image job skipped on Dependabot PRs (L-808h) | good |
| Checkout credentials | `persist-credentials: false` on all 22 checkouts | good |
| Dependabot | github-actions, cargo, docker ecosystems; weekly; 7-day cooldown (30 for semver-major); grouped | good |
| Branch protection | none on `main` | gap (see provenance finding) |

**Suspicious findings: 0.** No workflow pins changed.

## Follow-ups
1. Run `zizmor .github/workflows` (`pipx install zizmor` or `cargo binstall zizmor`) in security.yml for template-injection / cache-poisoning checks that manual review can miss (e.g. rust-cache on release jobs).
2. Consider disabling `Swatinem/rust-cache` in release.yml (cache poisoning of release artifacts).
3. Prefer a dhi.io token scoped to pull-only and store it as an environment secret with required reviewers for release.
4. Add a branch ruleset on `main` (required status checks, no force push, signed commits).
