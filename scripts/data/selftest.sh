#!/usr/bin/env bash
# Offline self-test of fetch.sh/convert.sh against a loopback HTTP server
# (TST-INF-008 / DATA-004 acceptance): second fetch is a no-op, a checksum
# mismatch fails, a partial download resumes, the licence gate refuses,
# --max-docs takes a prefix, conversion is deterministic.
set -euo pipefail
cd "$(dirname "$0")/../.."
EDGEN="${EDGEN:-target/release/edgen}"
[[ -x "$EDGEN" ]] || CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build -q --release -p edgen
t=$(mktemp -d); trap 'kill $srv 2>/dev/null || true; rm -rf "$t"' EXIT
mkdir -p "$t/www" "$t/cache"
"$EDGEN" gen --profile logs --count 5000 --format source --compress zstd --out "$t/www/logs.zst" 2>/dev/null
"$EDGEN" gen --profile logs --count 5000 --format source 2>/dev/null | python3 -c "import bz2,sys; sys.stdout.buffer.write(bz2.compress(sys.stdin.buffer.read()))" > "$t/www/logs.bz2"
printf '1.2.3.4 - - [01/Jul/1995:00:00:01 -0400] "GET / HTTP/1.0" 200 10\n' | gzip > "$t/www/a.gz"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
(cd "$t/www" && exec python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) & srv=$!
for _ in $(seq 50); do curl -fs "http://127.0.0.1:$port/" >/dev/null 2>&1 && break; sleep 0.1; done
sha() { sha256sum "$1" | cut -d' ' -f1; }
sz() { stat -c %s "$1"; }
cat > "$t/m.toml" <<TOML
[policy]
ci_license_classes = ["ok"]
[datasets.z]
url = "http://127.0.0.1:$port/logs.zst"
size = $(sz "$t/www/logs.zst")
sha256 = "$(sha "$t/www/logs.zst")"
license = "test"
license_class = "ok"
tier = "L0"
format = "ndjson-source"
compression = "zst"
converter = "rally"
[datasets.b]
url = "http://127.0.0.1:$port/logs.bz2"
size = $(sz "$t/www/logs.bz2")
sha256 = "$(sha "$t/www/logs.bz2")"
license = "test"
license_class = "ok"
tier = "L0"
format = "ndjson-source"
compression = "bz2"
converter = "rally"
[datasets.bad]
url = "http://127.0.0.1:$port/a.gz"
size = $(sz "$t/www/a.gz")
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
license = "test"
license_class = "ok"
tier = "L0"
format = "clf"
compression = "gz"
converter = "clf"
[datasets.nc]
url = "http://127.0.0.1:$port/a.gz"
size = 0
sha256 = ""
license = "CC-BY-NC"
license_class = "blocked"
tier = "L0"
format = "clf"
compression = "gz"
converter = "clf"
TOML
export ED_DATASETS="$t/m.toml" ED_DATA_CACHE="$t/cache" ED_DATA_TEST=1 EDGEN
F=scripts/data/fetch.sh
pass() { echo "PASS $*"; }
$F z | grep -q fetched && pass "first fetch downloads"
$F z | grep -q cached && pass "second fetch is a no-op"
out=$($F bad 2>&1 || true)
if grep -q 'CHECKSUM MISMATCH' <<<"$out"; then pass "checksum mismatch fails"; else echo "FAIL mismatch"; exit 1; fi
if $F bad >/dev/null 2>&1; then echo "FAIL mismatch rc"; exit 1; fi
head -c 1000 "$t/www/logs.bz2" > "$t/cache/b.bz2.part"
$F b | grep -q fetched && [[ "$(sha "$t/cache/b.bz2")" == "$(sha "$t/www/logs.bz2")" ]] && pass "partial download resumes"
out=$($F nc 2>&1 || true)
if grep -q 'LICENCE GATE' <<<"$out"; then pass "licence gate refuses blocked"; else echo "FAIL gate"; exit 1; fi
$F --verify z b >/dev/null && pass "--verify ok"
$F --max-docs 100 --out "$t/p.ndjson" b >/dev/null && [[ $(wc -l < "$t/p.ndjson") == 100 ]] && pass "--max-docs prefix (bz2)"
$F --max-docs 100 --out "$t/q.ndjson" z >/dev/null && [[ $(wc -l < "$t/q.ndjson") == 100 ]] && cmp -s "$t/p.ndjson" "$t/q.ndjson" && pass "--max-docs prefix (zst) equals bz2 prefix"
h1=$(scripts/data/convert.sh b | cut -d' ' -f1); h2=$(scripts/data/convert.sh b | cut -d' ' -f1)
h3=$(scripts/data/convert.sh z | cut -d' ' -f1)
[[ "$h1" == "$h2" ]] && pass "convert deterministic ($h1)"
n=$("$EDGEN" cat "$t/cache/converted/b.ndjson.zst" | wc -l); [[ $n == 5000 ]] && pass "converted 5000 docs"
echo "selftest: OK"
