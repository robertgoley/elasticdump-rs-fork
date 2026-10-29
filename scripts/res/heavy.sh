#!/usr/bin/env bash
# heavy.sh <GB> <cmd...>
# Run a heavy job (engine containers, loads) under the box-wide lock
# /tmp/edrs-heavy.lock so two agents never run engines at the same time,
# after memguard.sh confirms <GB> GiB is available and no IT suite
# (containers named it-*) is running.
set -euo pipefail
gb="${1:?usage: heavy.sh <GB> <cmd...>}"; shift
here="$(cd "$(dirname "$0")" && pwd)"
exec 9>/tmp/edrs-heavy.lock
echo "heavy: waiting for /tmp/edrs-heavy.lock" >&2
flock 9
if podman ps --filter name=it- --format '{{.Names}}' 2>/dev/null | grep -q .; then
  echo "heavy: an IT suite (it-*) is running; retry later" >&2
  exit 75
fi
"$here/memguard.sh" "$gb"
"$@"
