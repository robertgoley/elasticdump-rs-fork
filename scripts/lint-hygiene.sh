#!/usr/bin/env bash
# SUP-018 / L-109: static regression checks for repo-hygiene findings
# F-04, F-11 (part), F-12, F-13, F-14, F-15 (TEST-REQUIREMENTS §7.3).
# The workflow findings (F-01/02/03/09/10) are in scripts/lint-workflows.sh;
# F-11's "tests don't write to the CWD" is scripts/check-clean-tree.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
bad() { echo "FAIL $1: $2" >&2; fail=1; }

# F-04: no tracked-needed fixture may be ignored; every fixture is tracked.
if [[ -n "$(git ls-files --others --ignored --exclude-standard -- tests/fixtures)" ]]; then
  bad F-04 "ignored files under tests/fixtures: $(git ls-files --others --ignored --exclude-standard -- tests/fixtures | tr '\n' ' ')"
fi
git ls-files --error-unmatch tests/fixtures/node/stored_scripts.jsonl >/dev/null 2>&1 \
  || bad F-04 "tests/fixtures/node/stored_scripts.jsonl is not tracked"

# F-11: integration tests must not open relative test_query_* paths.
if grep -nE '"test_query_' tests/integration_test.rs | grep -v 'scratch(' >/dev/null; then
  bad F-11 "tests/integration_test.rs writes test_query_* outside scratch()"
fi

# F-12: compose binds loopback only and keeps security on.
if grep -nE '^\s*-\s*"?[0-9]+:[0-9]+"?\s*$' docker-compose.yaml >/dev/null; then
  bad F-12 "docker-compose.yaml publishes a port on all interfaces"
fi
grep -q 'xpack.security.enabled=true' docker-compose.yaml || bad F-12 "compose security is not enabled"
if grep -nE 'ELASTIC_PASSWORD=[^$]' docker-compose.yaml >/dev/null; then
  bad F-12 "compose has a literal ELASTIC_PASSWORD"
fi

# F-13: IT harness: umask 077, private IT_DIR outside /tmp, no fixed secrets
# (except OpenSearch < 2.12, whose demo config only accepts admin/admin).
grep -q '^umask 077' scripts/it/up.sh || bad F-13 "scripts/it/up.sh lacks umask 077"
for f in scripts/it/up.sh scripts/it/run.sh scripts/it/down.sh; do
  if grep -nE '^IT_DIR="\$\{IT_DIR:-/tmp' "$f" >/dev/null; then bad F-13 "$f defaults IT_DIR to /tmp"; fi
done
if grep -nE 'it-secret|ADMIN_PW="?[A-Za-z]' scripts/it/up.sh | grep -vE 'gen_secret|OS_LEGACY_ADMIN == 1' >/dev/null; then
  bad F-13 "fixed IT secret in scripts/it/up.sh"
fi

# F-14: image references carry a digest.
while read -r line; do
  [[ "$line" == *@sha256:* ]] || bad F-14 "Makefile: $line"
done < <(grep -E '^[A-Z_]*_TAG[[:space:]]+\?=' Makefile | grep -v '^IMAGE_TAG')
while read -r line; do
  [[ "$line" == *@sha256:* ]] || bad F-14 "Dockerfile: $line"
done < <(grep -E '^ARG (BUILDER|BASE)_TAG=' Dockerfile)
while read -r line; do
  [[ "$line" == *@sha256:* ]] || bad F-14 "compose: $line"
done < <(grep -E '^\s*image:' docker-compose.yaml)
grep -qE '^pin\(\)|^pin \(\)' scripts/it/up.sh || bad F-14 "scripts/it/up.sh has no pin()"
while read -r img dig; do
  [[ -z "$img" || "$img" == \#* ]] && continue
  [[ "$dig" =~ ^sha256:[0-9a-f]{64}$ ]] || bad F-14 "images.lock: bad digest for $img"
done < <(cat scripts/it/images.lock 2>/dev/null)
[[ -f scripts/it/images.lock ]] || bad F-14 "scripts/it/images.lock missing"

# F-15 (Grok Bot, 2026-11-02): no Trojan Source in tracked files: bidi controls (U+202A-202E, U+2066-2069),
# zero-width / invisible (U+200B-200F, U+2060-2064, U+FEFF BOM, U+00AD, U+180E). Matched as raw UTF-8 bytes
# (LC_ALL=C) so it needs only git's PCRE. Vendored crates are not tracked; see
# security/findings/2026-11-02-hidden-code-scan.md for the vendor scan. Allow-list: HIDDEN_ALLOW (path regex).
HIDDEN_ALLOW='^$'
hidden_re='\xE2\x80[\x8B-\x8F\xAA-\xAE]|\xE2\x81[\xA0-\xA4\xA6-\xA9]|\xEF\xBB\xBF|\xC2\xAD|\xE1\xA0\x8E'
hits=$(LC_ALL=C git grep -I -l -P "$hidden_re" -- . 2>/dev/null | grep -Ev "$HIDDEN_ALLOW" || true)
[[ -z "$hits" ]] || bad F-15 "bidi/invisible Unicode in: $(echo "$hits" | tr '\n' ' ')"

if [[ $fail -ne 0 ]]; then exit 1; fi
echo "hygiene lint: clean (F-04 F-11 F-12 F-13 F-14 F-15)"
