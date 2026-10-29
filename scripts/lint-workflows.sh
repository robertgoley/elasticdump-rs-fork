#!/usr/bin/env bash
# L-107/L-109: lint GitHub workflows (F-01/F-02/F-03/F-09/F-10 regressions).
# Needs actionlint and zizmor on PATH (L-002 installs them in ~/.cargo/bin).
set -euo pipefail
cd "$(dirname "$0")/.."
actionlint .github/workflows/*.yml
zizmor --offline --min-severity low .github/workflows/
# no remote install script piped to a shell, every action pinned to a SHA,
# every workflow has a permissions block
if grep -nE '^[^#]*curl[^|#]*\|\s*(sudo\s+)?(ba)?sh' .github/workflows/*.yml; then
  echo "remote script piped to a shell (F-01)"; exit 1
fi
if grep -nE 'uses:\s*[^ @]+@[^ ]*' .github/workflows/*.yml | grep -vE '@[0-9a-f]{40}( |$)'; then
  echo "action not pinned to a commit SHA (F-02)"; exit 1
fi
for f in .github/workflows/*.yml; do grep -q '^permissions:' "$f" || { echo "$f: no permissions block"; exit 1; }; done
# F-03: checkout never persists the token
if grep -n 'actions/checkout@' .github/workflows/*.yml >/dev/null; then
  n_co=$(grep -c 'actions/checkout@' .github/workflows/*.yml | awk -F: '{s+=$2} END {print s}')
  n_pc=$(grep -c 'persist-credentials: false' .github/workflows/*.yml | awk -F: '{s+=$2} END {print s}')
  if [[ "$n_pc" -lt "$n_co" ]]; then echo "checkout without persist-credentials: false (F-03)"; exit 1; fi
fi
# F-09: no event/input expressions expanded inside a shell script
if grep -nE '\$\{\{\s*(github\.event\.|inputs\.|github\.head_ref)' .github/workflows/*.yml | grep -vE '^\S+:\s*[A-Z_]+:\s' | grep -vE ':\s*(if|with|name|base|ref):'; then
  echo "event/input expression outside env: (F-09)"; exit 1
fi
# F-10: every cargo install is --locked with an exact --version
if grep -nE '^[^#]*cargo install' .github/workflows/*.yml | grep -vE -- '--locked.*--version|--version.*--locked'; then
  echo "cargo install without --locked --version (F-10)"; exit 1
fi
echo "workflows: OK"
