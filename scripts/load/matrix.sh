#!/usr/bin/env bash
# LOAD-004 scenario matrix entry point; see matrix.py for the contract.
#   scripts/load/matrix.sh L0 [--place shared] [--write-baseline]
set -euo pipefail
exec python3 "$(dirname "$0")/matrix.py" "$@"
