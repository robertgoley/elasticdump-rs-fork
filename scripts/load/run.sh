#!/usr/bin/env bash
# Load runner entry point; see run.py for the full contract.
#   scripts/load/run.sh L0 dump-restore-logs [--engine es717-plain] [--place box]
set -euo pipefail
exec python3 "$(dirname "$0")/run.py" "$@"
