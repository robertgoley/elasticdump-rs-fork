# Commit provenance audit (TESTING-TODO §2) — 2026-11-02, Grok Bot

Scope: all 351 commits reachable from `main` at 37fa3ac.

## Command
```
git log --format='%an <%ae> | %cn <%ce> | %G?' | sort | uniq -c | sort -rn
gh api repos/robertgoley/searchdump-rs/branches/main/protection
```

## Results
| Commits | Author = committer | Sig | Classification |
|---|---|---|---|
| 255 | Robert Goley <robert@local> | N | owner / owner's coding agent (OpenCode). Non-routable `@local` email: not linkable to a GitHub account |
| 6 | Robert Goley <robertgoley@users.noreply.github.com> | N | owner (incl. Grok Bot commits) |
| 1 | robertgoley <robert.goley@gmail.com> / GitHub | E (c83ea11) | owner via GitHub web UI; GitHub-signed, `E` only because GitHub's web-flow key is not in the local keyring |
| 39 | Kainan Zhu <kainan.zhu@tenyears.tech> | N | upstream tenyears-tech history |
| 17 | Kainan Zhu <dlangu0393@gmail.com> | N | upstream (same person, personal email) |
| 32 | microdog-codev <dlangu0393+codev@gmail.com> | N | upstream (same mailbox, +codev tag — upstream author's agent/alt account) |
| 1 | Microdog <dlangu0393@gmail.com> / GitHub | E (f4d1607) | upstream root commit via GitHub web UI |

No commit by an unknown identity. **Suspicious findings: 0.** Signed commits: 2/351 (both GitHub web-flow); 349 unsigned.
Branch protection on `main`: **none** (`Branch not protected`, HTTP 404).

Caveat: unsigned author fields are self-asserted; this audit shows consistency, not proof of identity. Upstream code is
treated as untrusted regardless.

## Follow-ups (owner action, not done here)
1. Set a real commit email for the local agent (e.g. `robertgoley@users.noreply.github.com`) instead of `robert@local`.
2. Enable SSH commit signing for the owner and each agent (`git config gpg.format ssh; user.signingkey ...; commit.gpgsign true`), register keys on GitHub as signing keys.
3. Branch protection / ruleset on `main`: require signed commits, block force-push and deletion, require CI status checks. Note: with two agents pushing directly to `main`, require-PR rules would break the current workflow; start with signed-commits + no-force-push.
