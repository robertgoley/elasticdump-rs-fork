# Enable SSH commit signing and a `main` ruleset — steps for Robert (2026-11-02, Grok Bot)

Nothing here was changed by Grok Bot; these are owner actions. Context: 349/351 commits on `main` are unsigned and
`main` has no protection (see 2026-11-02-commit-provenance.md).

## 1. SSH commit signing (each machine/agent that commits)
1. Create a dedicated signing key (no need to reuse the auth key):
   `ssh-keygen -t ed25519 -C "robertgoley commit signing" -f ~/.ssh/id_ed25519_signing`
2. Configure git (global, or per repo for an agent):
   ```
   git config --global gpg.format ssh
   git config --global user.signingkey ~/.ssh/id_ed25519_signing.pub
   git config --global commit.gpgsign true
   git config --global tag.gpgsign true
   git config --global user.email robertgoley@users.noreply.github.com   # replaces robert@local
   ```
3. Register the **public** key on GitHub as a *Signing key*: Settings → SSH and GPG keys → New SSH key → Key type
   "Signing Key" (or `gh ssh-key add ~/.ssh/id_ed25519_signing.pub --type signing --title "signing"`; needs the
   `admin:ssh_signing_key` scope: `gh auth refresh -s admin:ssh_signing_key`).
4. Local verification: `echo "robertgoley@users.noreply.github.com $(cat ~/.ssh/id_ed25519_signing.pub)" >> ~/.ssh/allowed_signers`,
   `git config --global gpg.ssh.allowedSignersFile ~/.ssh/allowed_signers`, then `git commit --allow-empty -m test-sign && git log --show-signature -1` → "Good signature"; on GitHub the commit shows **Verified**.
5. Repeat for each agent box (OpenCode agent, Grok Bot box) with its own key, all registered on your account, or give
   bots a separate GitHub account. Keep private keys out of the repo.

## 2. Ruleset on `main` (GitHub web UI)
Repo `robertgoley/searchdump-rs` → Settings → Rules → Rulesets → New branch ruleset:
- Name `main-protection`, Enforcement **Active**, Target branches: *Include default branch*.
- Bypass list: empty (or only yourself, "for pull requests only" is not needed since agents push directly).
- Enable: **Restrict deletions**, **Block force pushes**, **Require signed commits**.
- Optional once signing works everywhere: **Require status checks to pass** (CI `unit`, security `zizmor`, …) —
  note this blocks direct pushes until checks pass on the same commit, which the two-agent direct-push workflow
  does not do today; leave it off or move agents to PRs first.
- Do **not** enable "Require a pull request" unless you want to change the agents' workflow.

CLI alternative (after enabling signing first, otherwise your next push is rejected):
```
gh api -X POST repos/robertgoley/searchdump-rs/rulesets --input - <<'JSON'
{"name":"main-protection","target":"branch","enforcement":"active",
 "conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},
 "rules":[{"type":"deletion"},{"type":"non_fast_forward"},{"type":"required_signatures"}]}
JSON
```

## 3. Check
`gh api repos/robertgoley/searchdump-rs/rules/branches/main` lists the three rules; an unsigned push is
rejected with `GH013: Repository rule violations`.

Order matters: set up signing on **every** committer (both agents) before activating "Require signed commits", or
their pushes start failing.
