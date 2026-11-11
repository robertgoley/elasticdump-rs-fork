#!/usr/bin/env bash
# TST-BRK-SRV-006 under a real memory cap: a server that answers GET / and then streams an
# endless search response (chunked, no Content-Length). Under a 512 MB cgroup (cg-run.sh)
# the dump must stop with the --maxResponseMb error (exit 1), not be OOM-killed (137).
#
#   scripts/res/huge-response.sh [path/to/searchdump]   (default target/debug/searchdump)
#
# HUGE_EXTRA="--maxResponseMb 0" shows the uncapped behaviour (OOM kill, 137).
# Exit 0 = pass, 1 = fail, 77 = skipped (no cgroup v2 / sudo).
set -uo pipefail
cd "$(dirname "$0")/../.."
bin="${1:-target/debug/searchdump}"
[[ -x "$bin" ]] || { echo "missing $bin (cargo build first)" >&2; exit 1; }
work=$(mktemp -d)
trap 'kill "$srv" 2>/dev/null; rm -rf "$work"' EXIT
python3 - "$work/port" <<'PY' &
import socket, sys, threading
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0)); s.listen(8)
open(sys.argv[1], "w").write(str(s.getsockname()[1]))
INFO = b'{"version":{"distribution":"opensearch","number":"2.19.0"}}'
def serve(c):
    try:
        f = c.makefile("rb")
        while True:
            line = f.readline()
            if not line:
                return
            method = line.split()[0]
            n = 0
            while True:
                h = f.readline()
                if h in (b"\r\n", b""):
                    break
                if h.lower().startswith(b"content-length:"):
                    n = int(h.split(b":")[1])
            f.read(n)
            if method == b"GET":
                c.sendall(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: %d\r\n\r\n%s" % (len(INFO), INFO))
                continue
            c.sendall(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n")
            head = b'{"_scroll_id":"s","hits":{"total":{"value":1,"relation":"eq"},"hits":[{"_id":"1","_source":{"blob":"'
            c.sendall(b"%x\r\n%s\r\n" % (len(head), head))
            chunk = b"x" * (1 << 20)
            frame = b"%x\r\n%s\r\n" % (len(chunk), chunk)
            while True:
                c.sendall(frame)
    except OSError:
        pass
    finally:
        c.close()
while True:
    c, _ = s.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
PY
srv=$!
for _ in $(seq 50); do [[ -s "$work/port" ]] && break; sleep 0.1; done
port=$(cat "$work/port")
scripts/res/cg-run.sh 512M "$bin" --input "http://127.0.0.1:$port/idx" --output "$work/out.json" \
  --noImplicitRetry ${HUGE_EXTRA:-} > "$work/stdout" 2> "$work/stderr"
rc=$?
cat "$work/stderr" | tail -5
echo "huge-response: exit $rc"
[[ $rc == 77 ]] && exit 77
if [[ $rc == 1 ]] && grep -q -- "--maxResponseMb" "$work/stderr" && [[ ! -e "$work/out.json" ]]; then
  echo "huge-response: PASS (clear error under 512 MB, nothing committed)"; exit 0
fi
echo "huge-response: FAIL"; exit 1
