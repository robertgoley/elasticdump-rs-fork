#!/usr/bin/env bash
# TST-INF-004: out-of-process fault proxy (toxiproxy, MIT) for tests that need faults outside
# the test process (Node comparison, CLI-only runs) and for the fault conformance suite.
#   scripts/it/toxiproxy.sh up     start edl-toxiproxy (host network, API 127.0.0.1:$TOXIPROXY_PORT)
#                                  and print `export TOXIPROXY_URL=...`
#   scripts/it/toxiproxy.sh down   remove it
# Proxies listen on 127.0.0.1:19281-19296 (TOXIPROXY_LISTEN_BASE); the image is pinned by digest.
set -euo pipefail
cd "$(dirname "$0")/../.."
NAME=edl-toxiproxy
PORT="${TOXIPROXY_PORT:-19280}"
IMAGE="ghcr.io/shopify/toxiproxy:2.12.0"
digest=$(awk -v i="$IMAGE" '$1 == i {print $2}' scripts/it/images.lock)
[[ -n "$digest" ]] && IMAGE="$IMAGE@$digest"
case "${1:-up}" in
  up)
    podman rm -f "$NAME" >/dev/null 2>&1 || true
    if ss -ltn 2>/dev/null | grep -q "127.0.0.1:$PORT "; then
      echo "toxiproxy: port $PORT is busy (set TOXIPROXY_PORT)" >&2; exit 1
    fi
    podman run -d --name "$NAME" --label searchdump-it=1 --network host "$IMAGE" \
      -host 127.0.0.1 -port "$PORT" >/dev/null
    for _ in $(seq 1 50); do
      if curl -fsS "http://127.0.0.1:$PORT/version" >/dev/null 2>&1; then
        echo "export TOXIPROXY_URL=http://127.0.0.1:$PORT"; exit 0
      fi
      sleep 0.2
    done
    echo "toxiproxy did not start" >&2; podman logs "$NAME" >&2 || true; exit 1
    ;;
  down) podman rm -f "$NAME" >/dev/null 2>&1 || true ;;
  *) echo "usage: $0 up|down" >&2; exit 2 ;;
esac
