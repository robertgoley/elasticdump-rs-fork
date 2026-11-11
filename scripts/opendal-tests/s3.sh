#!/usr/bin/env bash
# L-809a: OpenDAL s3:// tests against a local S3 server in podman (--network=host), over TLS
# with a throw-away CA, including SSE-C.
#   scripts/opendal-tests/s3.sh minio|rustfs
# Env: EDRS_BIN (default target/debug/elasticdump-rs), ODT_DOCS (default 5000), KEEP=1 keeps
# the container. Credentials are random per run and never leave $WORK.
set -euo pipefail
SERVER=${1:?usage: s3.sh minio|rustfs}
. "$(dirname "$0")/common.sh"
MULTI=${EDRS_MULTI_BIN:-$(dirname "$BIN")/multielasticdump}

case $SERVER in
  minio) PORT=${ODT_MINIO_PORT:-19900}; IMAGE=${ODT_MINIO_IMAGE:-docker.io/bitnamilegacy/minio:2025.7.23} ;;
  rustfs) PORT=${ODT_RUSTFS_PORT:-19910}; IMAGE=${ODT_RUSTFS_IMAGE:-docker.io/rustfs/rustfs:latest} ;;
  *) echo "unknown server $SERVER" >&2; exit 2 ;;
esac
NAME=edrs-odt-$SERVER
EP=https://127.0.0.1:$PORT
AK=odt$(openssl rand -hex 6)
SK=$(openssl rand -hex 20)
BUCKET=edrs-odt
TLS=$WORK/tls
make_tls "$TLS"
chmod 755 "$TLS"; chmod 644 "$TLS"/*

stop_container "$NAME"
if [ "${ODT_START_ONLY:-0}" != 1 ]; then
  trap '[ "${KEEP:-0}" = 1 ] || stop_container "$NAME"' EXIT
fi
case $SERVER in
  minio)
    mkdir -p "$TLS/minio"; cp "$TLS/server.crt" "$TLS/minio/public.crt"; cp "$TLS/server.key" "$TLS/minio/private.key"
    chmod 644 "$TLS/minio"/*
    podman run -d --name "$NAME" --network=host --tmpfs /data:rw,mode=1777 \
      -v "$TLS/minio:/certs:ro,Z" -e MINIO_ROOT_USER="$AK" -e MINIO_ROOT_PASSWORD="$SK" \
      --entrypoint /opt/bitnami/minio/bin/minio "$IMAGE" \
      server /data --address "127.0.0.1:$PORT" --console-address "127.0.0.1:$((PORT + 1))" --certs-dir /certs >/dev/null
    ;;
  rustfs)
    mkdir -p "$TLS/rustfs"; cp "$TLS/server.crt" "$TLS/rustfs/rustfs_cert.pem"; cp "$TLS/server.key" "$TLS/rustfs/rustfs_key.pem"
    chmod 755 "$TLS/rustfs"; chmod 644 "$TLS/rustfs"/*
    podman run -d --name "$NAME" --network=host --tmpfs /data:rw,mode=1777 --tmpfs /logs:rw,mode=1777 \
      -v "$TLS/rustfs:/opt/tls:ro,Z" -e RUSTFS_TLS_PATH=/opt/tls \
      -e RUSTFS_ACCESS_KEY="$AK" -e RUSTFS_SECRET_KEY="$SK" \
      -e RUSTFS_ADDRESS="127.0.0.1:$PORT" -e RUSTFS_CONSOLE_ENABLE=false "$IMAGE" >/dev/null
    ;;
esac
wait_port "$PORT" || { podman logs "$NAME" | tail -n 30; bad "$SERVER start"; summary "$SERVER"; }
s3curl() { curl -sS --cacert "$TLS/ca.crt" --aws-sigv4 "aws:amz:us-east-1:s3" --user "$AK:$SK" "$@"; }
for i in $(seq 1 30); do s3curl -o /dev/null -w '%{http_code}' -X PUT "$EP/$BUCKET" 2>/dev/null | grep -q 200 && break; sleep 1; done
log "$SERVER $(podman inspect "$NAME" --format '{{.ImageName}}') at $EP, bucket $BUCKET"

CFG=$WORK/s3.ini
cat >"$CFG" <<INI
endpoint = $EP
region = us-east-1
key = $AK
secret = $SK
ca_file = $TLS/ca.crt
INI
chmod 600 "$CFG"
SSEC=$WORK/ssec.key
head -c 32 /dev/urandom >"$SSEC"; chmod 600 "$SSEC"
CFG_SSEC=$WORK/s3-ssec.ini
{ cat "$CFG"; echo "sse_c_key_file = $SSEC"; } >"$CFG_SSEC"; chmod 600 "$CFG_SSEC"
CFG_WRONG=$WORK/s3-wrong.ini
head -c 32 /dev/urandom >"$WORK/wrong.key"
{ cat "$CFG"; echo "sse_c_key_file = $WORK/wrong.key"; } >"$CFG_WRONG"

if [ "${ODT_START_ONLY:-0}" = 1 ]; then
  printf 'S3_URL=%s\nS3_CFG=%s\nS3_CFG_SSEC=%s\n' "s3://$BUCKET/$SERVER-$(date +%s)" "$CFG" "$CFG_SSEC" >"$WORK/s3-$SERVER.env"
  log "$SERVER running at $EP (stop: podman rm -f $NAME)"
  exit 0
fi
IN=$WORK/in.json
make_input "$IN"
S3=s3://$BUCKET/$SERVER-$(date +%s)
ed() { "$BIN" --quiet "$@"; }
out() { ed --outputStorageConfig "$CFG" "$@"; }
inp() { ed --inputStorageConfig "$CFG" "$@"; }

# 1. local -> s3 -> local, plain
check "$SERVER: local -> s3 (plain)" out --input "$IN" --output "$S3/plain.json"
check "$SERVER: s3 -> local (plain)" inp --input "$S3/plain.json" --output "$WORK/plain.back.json"
check "$SERVER: plain round trip keeps every document" same_docs "$IN" "$WORK/plain.back.json"

# 2. gzip + split parts
check "$SERVER: local -> s3 gzip split (--maxRows 1500)" out --input "$IN" --output "$S3/gz/d.json" --fsCompress --maxRows 1500
: >"$WORK/gz.back.json"
gz_ok=1
for n in 0 1 2 3; do
  inp --input "$S3/gz/d.split-$n.json.gz" --output "$WORK/gz.part$n.json" >>"$WORK/test.log" 2>&1 || gz_ok=0
  cat "$WORK/gz.part$n.json" >>"$WORK/gz.back.json" 2>/dev/null || true
done
if [ $gz_ok = 1 ] && [ "$(s3curl "$EP/$BUCKET/${S3#s3://$BUCKET/}/gz/d.split-0.json.gz" | head -c 2 | od -An -tx1 | tr -d ' ')" = 1f8b ]; then
  ok "$SERVER: 4 gzip parts stored (gzip magic) and read back"
else bad "$SERVER: gzip parts"; fi
check "$SERVER: gzip split round trip keeps every document" same_docs "$IN" "$WORK/gz.back.json"

# 3. zstd
check "$SERVER: local -> s3 zstd" out --input "$IN" --output "$S3/d.json.zst" --compressionType zstd
check "$SERVER: s3 zstd -> local" inp --input "$S3/d.json.zst" --output "$WORK/zst.back.json"
check "$SERVER: zstd round trip keeps every document" same_docs "$IN" "$WORK/zst.back.json"

# 4. overwrite
check_fails "$SERVER: existing object refused without --overwrite" "already exists" out --input "$IN" --output "$S3/plain.json"
check "$SERVER: --overwrite replaces it" out --input "$WORK/zst.back.json" --output "$S3/plain.json" --overwrite

# 5. SSE-C
check "$SERVER: SSE-C write" ed --outputStorageConfig "$CFG_SSEC" --input "$IN" --output "$S3/ssec.json"
check "$SERVER: SSE-C read with the key" ed --inputStorageConfig "$CFG_SSEC" --input "$S3/ssec.json" --output "$WORK/ssec.back.json"
check "$SERVER: SSE-C round trip keeps every document" same_docs "$IN" "$WORK/ssec.back.json"
check_fails "$SERVER: SSE-C object unreadable without the key" "." inp --input "$S3/ssec.json" --output "$WORK/ssec.nokey.json"
check_fails "$SERVER: SSE-C object unreadable with a wrong key" "." ed --inputStorageConfig "$CFG_WRONG" --input "$S3/ssec.json" --output "$WORK/ssec.wrong.json"
code=$(s3curl -o "$WORK/raw.get" -w '%{http_code}' "$EP/$BUCKET/${S3#s3://$BUCKET/}/ssec.json")
if [ "$code" != 200 ] && ! grep -q '"_id"' "$WORK/raw.get"; then ok "$SERVER: plain GET of the SSE-C object refused (HTTP $code)"; else bad "$SERVER: plain GET of SSE-C object returned $code"; fi

# 6. secrets stay out of messages; secrets in URLs refused
check_fails "$SERVER: secret in the URL refused" "not accepted in the URL" ed --input "$IN" --output "$S3/x.json?secret_access_key=$SK"
BADCFG=$WORK/bad.ini
sed "s/^secret = .*/secret = FAKE-odt-wrong-secret-0001/" "$CFG" >"$BADCFG"
msg=$(ed --outputStorageConfig "$BADCFG" --input "$IN" --output "$S3/denied.json" 2>&1 || true)
echo "$msg" >>"$WORK/test.log"
if [ -n "$msg" ] && ! grep -q "FAKE-odt-wrong-secret-0001" <<<"$msg" && ! grep -q "$SK" <<<"$msg"; then ok "$SERVER: wrong secret fails without printing it"; else bad "$SERVER: secret leaked or no error"; fi
msg=$(RUST_LOG=debug "$BIN" --debug --outputStorageConfig "$CFG_SSEC" --input "$IN" --output "$S3/debug.json" 2>&1 || true)
echo "$msg" | tail -n 5 >>"$WORK/test.log"
SSEC_B64=$(base64 -w0 <"$SSEC")
lines=$(grep -c . <<<"$msg" || true)
if [ "$lines" -gt 20 ] && ! grep -qF "$SK" <<<"$msg" && ! grep -qF "$SSEC_B64" <<<"$msg"; then
  ok "$SERVER: debug log ($lines lines, SSE-C run) holds neither the secret key nor the SSE-C key"
else bad "$SERVER: debug log check ($lines lines)"; fi

# 7. multielasticdump: whole dump directory local -> s3 -> local, byte for byte
TREE=$WORK/tree; mkdir -p "$TREE/_cluster" "$TREE/2026.11"
cp "$IN" "$TREE/odt.data.json"; gzip -c "$IN" >"$TREE/2026.11/odt2.data.split-0.json.gz"
gzip -c "$IN" >"$TREE/2026.11/odt2.data.split-1.json.gz"; echo '{"odt":{}}' >"$TREE/odt.mapping.json"
echo '{}' >"$TREE/_cluster/policy.json"; echo '{"version":1}' >"$TREE/multielasticdump.json"
check "$SERVER: multielasticdump copy local dir -> s3" "$MULTI" --direction copy --outputStorageConfig "$CFG" --input "$TREE" --output "$S3/tree"
check "$SERVER: multielasticdump copy s3 -> local dir" "$MULTI" --direction copy --inputStorageConfig "$CFG" --input "$S3/tree" --output "$WORK/tree.back"
check "$SERVER: copied tree identical (diff -r)" diff -r "$TREE" "$WORK/tree.back"
check_fails "$SERVER: copy onto existing objects refused" "already exists" "$MULTI" --direction copy --outputStorageConfig "$CFG" --input "$TREE" --output "$S3/tree"

summary "s3 ($SERVER)"
