#!/usr/bin/env bash
# L-108 / INF-014: run the no-cluster test suites and fail if they leave any
# new file in the working tree (tracked, untracked or ignored, except
# target/). Catches tests that write into the CWD (F-11) and stray files such
# as `test_query_*.json` or `oom`.
set -euo pipefail
cd "$(dirname "$0")/.."
snap() { git status --porcelain --ignored --untracked-files=all | grep -v ' target/' | sort; }
before=$(snap)
cargo test --lib --bins --test cli_contract --test injection --test secrets -q "$@" >/dev/null
after=$(snap)
if [[ "$before" != "$after" ]]; then
  echo "tests changed the working tree:"; diff <(echo "$before") <(echo "$after") || true
  exit 1
fi
echo "working tree unchanged by tests"
