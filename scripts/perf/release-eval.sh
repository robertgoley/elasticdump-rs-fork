#!/usr/bin/env bash
# Evaluate a published searchdump-rs *release* (or a local binary dir) on a Linux
# host: isolated rootless podman, the usual engine set and seeders, results you
# can commit back from this checkout. See scripts/perf/RELEASE-EVAL.md.
set -euo pipefail
umask 077
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
command -v python3 >/dev/null || { echo 'release-eval needs Python 3.11+.' >&2; exit 2; }
exec python3 "$root/scripts/perf/release_eval.py" "$@"
