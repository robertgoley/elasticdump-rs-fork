#!/usr/bin/env bash
# TST-SUP-015 / L-210: secret scanning + canaries.
#   scripts/sup/secret-scan.sh            full history + uncommitted tracked changes
#   scripts/sup/secret-scan.sh BASE..HEAD only the commits of a PR
# 1. gitleaks over git history (redacted output; findings fail).
# 2. gitleaks over tracked files in the working tree (target/ and other ignored paths are
#    build output and are skipped: rustls/webpki test PEMs end up embedded there).
# 3. the canary suite (tests/secrets.rs: credentials, proxy userinfo, key passphrases and
#    header values must never appear in stdout/stderr/debug output/files) — BRK-CLI-007 subset.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v gitleaks >/dev/null || { echo "gitleaks not installed"; exit 2; }
if [[ -n "${1:-}" ]]; then
  gitleaks git --no-banner --redact --log-opts="$1" .
else
  gitleaks git --no-banner --redact .
fi
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
git ls-files -z | tar --null -T - -cf - | tar -xf - -C "$T"
gitleaks dir --no-banner --redact "$T"
cargo test -q --test secrets
echo "secret-scan: OK"
