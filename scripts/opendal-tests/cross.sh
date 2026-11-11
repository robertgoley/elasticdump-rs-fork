#!/usr/bin/env bash
# L-809a: store-to-store transfers without a cluster (local -> s3 -> sftp -> local) with
# searchdump and searchdump-multi --direction copy, and (unless ODT_NO_CLUSTER=1) a
# cluster run: OpenSearch -> s3 dump, s3 -> OpenSearch restore, searchdump-multi dump to and
# load from storage URLs through a staging directory. Uses MinIO and OpenSSH (podman,
# --network=host) and, for the cluster part, a throw-away OpenSearch 2 (security off,
# 512 MB heap, 127.0.0.1 only).
set -euo pipefail
. "$(dirname "$0")/common.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
MULTI=${EDRS_MULTI_BIN:-$(dirname "$BIN")/searchdump-multi}
OS_PORT=${ODT_OS_PORT:-19930}
OS_NAME=edrs-odt-opensearch
# ODT_OS_PODMAN_ARGS: extra podman options for the OpenSearch container only, e.g. a separate
# image store (--root DIR --runroot DIR) when the shared one is damaged.
read -r -a OSP <<<"podman ${ODT_OS_PODMAN_ARGS:-}"
cleanup() {
  [ "${KEEP:-0}" = 1 ] && return
  stop_container edrs-odt-minio; stop_container edrs-odt-openssh; "${OSP[@]}" rm -f "$OS_NAME" >/dev/null 2>&1 || true
  ssh-keygen -R "[127.0.0.1]:${ODT_OPENSSH_PORT:-19922}" >/dev/null 2>&1 || true; rm -f ~/.ssh/known_hosts.old
}
trap cleanup EXIT
export ODT_WORK=$WORK
ODT_START_ONLY=1 "$HERE/s3.sh" minio >>"$WORK/test.log" 2>&1
ODT_START_ONLY=1 "$HERE/sftp.sh" openssh >>"$WORK/test.log" 2>&1
. "$WORK/s3-minio.env"; . "$WORK/sftp-openssh.env"
ed() { "$BIN" --quiet "$@"; }
IN=$WORK/in.json
make_input "$IN"

# searchdump, one file through three stores; gzip kept as given
check "local -> s3 (gzip)" ed --outputStorageConfig "$S3_CFG" --input "$IN" --output "$S3_URL/x.json.gz" --fsCompress
check "s3 -> sftp (gzip)" ed --inputStorageConfig "$S3_CFG" --outputStorageConfig "$SFTP_CFG" \
  --input "$S3_URL/x.json.gz" --output "$SFTP_URL/x.json.gz" --fsCompress
check "sftp -> local" ed --inputStorageConfig "$SFTP_CFG" --input "$SFTP_URL/x.json.gz" --output "$WORK/x.back.json"
check "local -> s3 -> sftp -> local keeps every document" same_docs "$IN" "$WORK/x.back.json"
check "local -> s3 with SSE-C" ed --outputStorageConfig "$S3_CFG_SSEC" --input "$IN" --output "$S3_URL/enc.json"
check "s3 (SSE-C, read with the key) -> sftp" ed --inputStorageConfig "$S3_CFG_SSEC" --outputStorageConfig "$SFTP_CFG" \
  --input "$S3_URL/enc.json" --output "$SFTP_URL/enc.json"

# searchdump-multi: a whole dump directory local -> s3 -> sftp -> local, byte for byte
TREE=$WORK/tree; mkdir -p "$TREE/_cluster" "$TREE/2026.11"
cp "$IN" "$TREE/a.data.json"; gzip -c "$IN" >"$TREE/2026.11/b.data.split-0.json.gz"
gzip -c "$IN" >"$TREE/2026.11/b.data.split-1.json.gz"
if command -v zstd >/dev/null; then zstd -q -c "$IN" >"$TREE/c.data.json.zst"; fi
echo '{"a":{}}' >"$TREE/a.mapping.json"; echo '{}' >"$TREE/_cluster/policy.json"; echo '{"version":1}' >"$TREE/multielasticdump.json"
check "searchdump-multi copy local -> s3" "$MULTI" --direction copy --outputStorageConfig "$S3_CFG" --input "$TREE" --output "$S3_URL/tree"
check "searchdump-multi copy s3 -> sftp" "$MULTI" --direction copy --inputStorageConfig "$S3_CFG" --outputStorageConfig "$SFTP_CFG" --input "$S3_URL/tree" --output "$SFTP_URL/tree"
check "searchdump-multi copy sftp -> local" "$MULTI" --direction copy --inputStorageConfig "$SFTP_CFG" --input "$SFTP_URL/tree" --output "$WORK/tree.back"
check "tree identical after three stores (diff -r: compression, split parts, manifest kept)" diff -r "$TREE" "$WORK/tree.back"

if [ "${ODT_NO_CLUSTER:-0}" != 1 ]; then
  OS=http://127.0.0.1:$OS_PORT
  "${OSP[@]}" rm -f "$OS_NAME" >/dev/null 2>&1 || true
  "${OSP[@]}" run -d --name "$OS_NAME" --network=host -e discovery.type=single-node -e DISABLE_SECURITY_PLUGIN=true \
    -e DISABLE_INSTALL_DEMO_CONFIG=true -e http.port=$OS_PORT -e transport.port=$((OS_PORT + 1)) -e network.host=127.0.0.1 \
    -e OPENSEARCH_JAVA_OPTS="-Xms512m -Xmx512m" \
    "${ODT_OS_IMAGE:-docker.io/opensearchproject/opensearch:2}" >/dev/null
  for i in $(seq 1 120); do curl -sf "$OS/_cluster/health" >/dev/null && break; sleep 2; done
  check "OpenSearch up ($("${OSP[@]}" inspect "$OS_NAME" --format '{{.ImageName}}'))" curl -sf "$OS/"
  for idx in odt-a odt-b; do
    # "big" is beyond a long: map it as keyword (the _source keeps the exact number)
    curl -sf -X PUT -H 'Content-Type: application/json' "$OS/$idx" \
      -d '{"settings":{"number_of_replicas":0},"mappings":{"properties":{"big":{"type":"keyword"}}}}' >/dev/null
    python3 - "$IN" "$idx" >"$WORK/$idx.bulk" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    d = json.loads(line)
    print(json.dumps({"index": {"_index": sys.argv[2], "_id": d["_id"]}}))
    print(json.dumps(d["_source"], ensure_ascii=False))
PY
    curl -sf -H 'Content-Type: application/x-ndjson' --data-binary @"$WORK/$idx.bulk" "$OS/_bulk?refresh=true" >"$WORK/$idx.bulk.out"
  done
  count() { curl -s "$OS/$1/_count" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("count", -1))'; }
  check "OpenSearch seeded" test "$(count odt-a)" = "$DOCS"
  check "searchdump OpenSearch -> s3 (zstd, split)" ed --outputStorageConfig "$S3_CFG" --input "$OS/odt-a" --output "$S3_URL/os/odt-a.json" --compressionType zstd --maxRows 2000
  check "searchdump mapping OpenSearch -> s3" ed --outputStorageConfig "$S3_CFG" --input "$OS/odt-a" --output "$S3_URL/os/odt-a.mapping.json" --type mapping
  check "searchdump mapping s3 -> OpenSearch (new index)" ed --inputStorageConfig "$S3_CFG" --input "$S3_URL/os/odt-a.mapping.json" --output "$OS/odt-r" --type mapping
  for n in 0 1 2; do
    ed --inputStorageConfig "$S3_CFG" --input "$S3_URL/os/odt-a.split-$n.json.zst" --output "$OS/odt-r" >>"$WORK/test.log" 2>&1 || true
  done
  curl -sf -X POST "$OS/odt-r/_refresh" >/dev/null
  check "searchdump s3 parts -> OpenSearch restore, all documents" test "$(count odt-r)" = "$DOCS"
  check "searchdump-multi dump OpenSearch -> s3:// (staged)" "$MULTI" --direction dump --input "$OS" --output "$S3_URL/multi" \
    --outputStorageConfig "$S3_CFG" --match '^odt-[ab]$' --compression zstd3 --maxRows 2000 --logDir "$WORK/multi-logs"
  check "searchdump-multi copy of that dump s3 -> sftp" "$MULTI" --direction copy --inputStorageConfig "$S3_CFG" --outputStorageConfig "$SFTP_CFG" \
    --input "$S3_URL/multi" --output "$SFTP_URL/multi"
  check "searchdump-multi load sftp:// -> OpenSearch (staged, --suffix -copy)" "$MULTI" --direction load --input "$SFTP_URL/multi" \
    --inputStorageConfig "$SFTP_CFG" --output "$OS" --suffix=-copy --logDir "$WORK/multi-logs"
  curl -sf -X POST "$OS/_refresh" >/dev/null
  check "loaded indices complete" test "$(count odt-a-copy)" = "$DOCS" -a "$(count odt-b-copy)" = "$DOCS"
fi
summary "cross-store"
