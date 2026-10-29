#!/usr/bin/env bash
# L-107: lint GitHub workflows (F-01/F-02/F-03/F-09/F-10 regressions).
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
echo "workflows: OK"
