#!/usr/bin/env bash
# Start a single-node OpenSearch container for the integration suite.
#
#   scripts/it/up.sh opensearch-plain|opensearch-secure|elasticsearch-plain|elasticsearch-secure
#
# OpenSearch: docker.io/opensearchproject/opensearch:${OS_TAG:-2}.
# Elasticsearch: docker.io/bitnamilegacy/elasticsearch:${ES_TAG} (pinned below),
# configured through the bitnami env conventions (ELASTICSEARCH_HEAP_SIZE,
# ELASTICSEARCH_ENABLE_SECURITY, ELASTICSEARCH_TLS_USE_PEM, ...). The image
# runs as UID 1001 (GID 0), so the bind-mounted data dir is chowned inside
# the rootless user namespace with `podman unshare`.
#
# Writes $IT_DIR/env (source it) with IT_ES_URL, IT_PROFILE and, for the
# secure profile, IT_ADMIN_USER/IT_ADMIN_PW/IT_CA/IT_CLIENT_CERT/IT_CLIENT_KEY/
# IT_CLIENT_KEY_ENC/IT_CLIENT_KEY_PASS. Containers carry the label
# elasticdump-rs-it=1 so down.sh can always find them.
set -euo pipefail

PROFILE="${1:-opensearch-plain}"
OS_TAG="${OS_TAG:-2}"
OS_IMAGE="${OS_IMAGE:-docker.io/opensearchproject/opensearch:${OS_TAG}}"
# Highest version tag in bitnamilegacy/elasticsearch as of 2026-10-28
# (registry tag list: 9.1.2 and 9.1.2-debian-12-r0; no 9.2+).
# Override with ES_TAG=8.6.1-debian-11-r0 (or `run.sh elasticsearch8*`) for the
# pinned ES 8 regression target; the bitnami env and config layout are the same.
ES_TAG="${ES_TAG:-9.1.2-debian-12-r0}"
ES_IMAGE="${ES_IMAGE:-docker.io/bitnamilegacy/elasticsearch:${ES_TAG}}"
# Official Elastic images (docker.elastic.co/elasticsearch/elasticsearch:*)
# take plain `elasticsearch.yml` settings as env vars and run as UID 1000;
# bitnami images use their own env conventions (see below).
ES_OFFICIAL=0
[[ "$ES_IMAGE" == docker.elastic.co/* ]] && ES_OFFICIAL=1
# F-14: pin by digest from images.lock (IT_UNPINNED=1 to use the bare tag).
pin() {
  local img="$1" d
  [[ "$img" == *@sha256:* || "${IT_UNPINNED:-0}" == 1 ]] && { echo "$img"; return; }
  d=$(awk -v i="$img" '$1 == i {print $2}' "$(dirname "$0")/images.lock" 2>/dev/null)
  if [[ -n "$d" ]]; then echo "$img@$d"; else echo "warning: $img is not pinned in images.lock" >&2; echo "$img"; fi
}
OS_IMAGE=$(pin "$OS_IMAGE")
ES_IMAGE=$(pin "$ES_IMAGE")
# OpenSearch < 2.12 ignores OPENSEARCH_INITIAL_ADMIN_PASSWORD; its demo
# config always uses admin/admin.
OS_LEGACY_ADMIN=0
[[ "$OS_TAG" == 1* || "$OS_TAG" =~ ^2\.([0-9]|1[01])(\.|$) ]] && OS_LEGACY_ADMIN=1
RUN_ID="${RUN_ID:-$$}"
# F-13: per-run secrets live in a private directory (not world-readable /tmp
# defaults): umask 077, IT_DIR mode 0700, random passwords and key passphrase.
umask 077
IT_DIR="${IT_DIR:-${XDG_RUNTIME_DIR:-$HOME/.cache}/elasticdump-rs-it}"
gen_secret() { printf 'It-%s-Aa1!' "$(openssl rand -hex 12)"; }
KEY_PASS="${IT_KEY_PASS:-$(openssl rand -hex 16)}"
HEAP="${OS_HEAP:-512m}"
# Elasticsearch heap (default: same as OS_HEAP). ES 9 at 512m is ~950 MB RSS;
# on a box with ~1.5 GB free use ES_HEAP=320m IT_LEAN_JVM=1.
ES_HEAP="${ES_HEAP:-$HEAP}"
# Extra JVM flags for small boxes (IT_LEAN_JVM=1): small code cache, direct
# memory and thread stacks. No GC override: both launchers pin G1 and refuse a
# second collector ("Multiple garbage collectors selected").
LEAN_OPTS=""
if [[ "${IT_LEAN_JVM:-0}" == 1 ]]; then
  LEAN_OPTS="-XX:ReservedCodeCacheSize=64m -XX:MaxDirectMemorySize=128m -Xss512k -XX:CICompilerCount=2"
fi
mkdir -p "$IT_DIR" && chmod 700 "$IT_DIR"
# TST-INF-007 (d): every container gets memory/CPU caps. Rootless podman with
# the cgroupfs manager accepts but does not enforce --memory here (L-003);
# they apply on hosts with cgroup v2 delegation (CI runners).
# shellcheck source=../res/guards.sh
source "$(dirname "$0")/../res/guards.sh"
_heap_max=$(( $(mb_of "$ES_HEAP") > $(mb_of "$HEAP") ? $(mb_of "$ES_HEAP") : $(mb_of "$HEAP") ))
CAPS=(--memory "${IT_CONTAINER_MEM:-$(( _heap_max * 2 + 512 ))m}" --cpus "${IT_CONTAINER_CPUS:-2}")
# A pre-existing env file keeps its old mode on `>`; recreate it 0600.
rm -f "$IT_DIR/env"

engine() { podman "$@"; }

# Data dir owned by the image's non-root user (1001:0) inside the userns.
es_data_dir() {
  local d="$IT_DIR/es-data-$1"
  mkdir -p "$d"
  podman unshare chown -R 1001:0 "$d"
  echo "$d"
}

# Test PKI for the secured Elasticsearch profile: CA, node cert for
# localhost/127.0.0.1, a client cert and an encrypted PKCS#8 client key.
es_pki() {
  local d="$IT_DIR/es-certs"
  rm -rf "$d"; mkdir -p "$d"
  (
    cd "$d"
    openssl req -x509 -newkey rsa:2048 -nodes -days 3 -subj "/CN=edrs-it-ca" \
      -keyout ca.key -out ca.crt 2>/dev/null
    openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout tls.key -out tls.csr 2>/dev/null
    printf "subjectAltName=DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth,clientAuth\n" > node.ext
    openssl x509 -req -in tls.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 3 \
      -extfile node.ext -out tls.crt 2>/dev/null
    openssl req -newkey rsa:2048 -nodes -subj "/CN=edrs-client" -keyout client-key.pem -out client.csr 2>/dev/null
    printf "extendedKeyUsage=clientAuth\n" > client.ext
    openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 3 \
      -extfile client.ext -out client.pem 2>/dev/null
    openssl pkcs8 -topk8 -v2 aes-256-cbc -in client-key.pem -out client-key-enc.pem \
      -passout "pass:$KEY_PASS" 2>/dev/null
    # Optional client certificates on HTTP (presented certs are verified
    # against the CA; authentication stays basic auth: the PKI realm is a
    # paid feature and bitnami exposes no switch for it).
    printf "xpack.security.http.ssl.client_authentication: optional\nxpack.ml.enabled: false\n" > my_elasticsearch.yml
    # readable by the container's non-root user; IT_DIR (0700) keeps them
    # private from other host users
    chmod 644 ./*
  )
  chmod 755 "$d"
  echo "$d"
}

case "$PROFILE" in
  opensearch-plain)
    PORT="${PORT:-19200}"
    NAME="it-os-plain-${RUN_ID}"
    engine run -d --name "$NAME" --label elasticdump-rs-it=1 "${CAPS[@]}" \
      -p "127.0.0.1:${PORT}:9200" \
      -e discovery.type=single-node \
      -e DISABLE_SECURITY_PLUGIN=true -e DISABLE_INSTALL_DEMO_CONFIG=true \
      -e "OPENSEARCH_JAVA_OPTS=-Xms${HEAP} -Xmx${HEAP} ${LEAN_OPTS}" \
      "$OS_IMAGE" >/dev/null
    URL="http://127.0.0.1:${PORT}"
    CURL=(curl -fsS)
    ;;
  opensearch-secure)
    PORT="${PORT:-19201}"
    NAME="it-os-sec-${RUN_ID}"
    ADMIN_PW="${IT_ADMIN_PW:-$(gen_secret)}"
    [[ $OS_LEGACY_ADMIN == 1 ]] && ADMIN_PW=admin
    engine run -d --name "$NAME" --label elasticdump-rs-it=1 "${CAPS[@]}" \
      -p "127.0.0.1:${PORT}:9200" \
      -e discovery.type=single-node \
      -e "OPENSEARCH_INITIAL_ADMIN_PASSWORD=${ADMIN_PW}" \
      -e "OPENSEARCH_JAVA_OPTS=-Xms${HEAP} -Xmx${HEAP} ${LEAN_OPTS}" \
      "$OS_IMAGE" >/dev/null
    URL="https://localhost:${PORT}"
    CURL=(curl -fsS -k -u "admin:${ADMIN_PW}")
    ;;
  elasticsearch-plain)
    PORT="${PORT:-19202}"
    if [[ $ES_OFFICIAL == 1 ]]; then
      NAME="it-es-plain-${RUN_ID}"
      engine run -d --name "$NAME" --label elasticdump-rs-it=1 "${CAPS[@]}" \
        -p "127.0.0.1:${PORT}:9200" \
        -e discovery.type=single-node \
        -e xpack.security.enabled=false \
        -e xpack.ml.enabled=false \
        -e "ES_JAVA_OPTS=-Xms${ES_HEAP} -Xmx${ES_HEAP} ${LEAN_OPTS}" \
        "$ES_IMAGE" >/dev/null
      URL="http://127.0.0.1:${PORT}"
      CURL=(curl -fsS)
    else
    NAME="it-es-plain-${RUN_ID}"
    DATA=$(es_data_dir "$NAME")
    printf "xpack.ml.enabled: false\n" > "$IT_DIR/es-plain.yml"
    chmod 644 "$IT_DIR/es-plain.yml"
    # (its my_elasticsearch.yml replaces the generated config, so the 7.10.2
    # OSS image gets none: it has no X-Pack settings to turn off anyway)
    MOUNTS=(-v "$DATA:/bitnami/elasticsearch/data:Z")
    [[ "$ES_TAG" != 7.10* ]] && MOUNTS+=(-v "$IT_DIR/es-plain.yml:/opt/bitnami/elasticsearch/config/my_elasticsearch.yml:ro,Z")
    engine run -d --name "$NAME" --label elasticdump-rs-it=1 "${CAPS[@]}" \
      -p "127.0.0.1:${PORT}:9200" \
      "${MOUNTS[@]}" \
      -e "ELASTICSEARCH_HEAP_SIZE=${ES_HEAP}" \
      -e "ES_JAVA_OPTS=${LEAN_OPTS}" \
      -e ELASTICSEARCH_ENABLE_SECURITY=false \
      -e ELASTICSEARCH_DISABLE_GC_LOGS=true \
      "$ES_IMAGE" >/dev/null
    URL="http://127.0.0.1:${PORT}"
    CURL=(curl -fsS)
    fi
    ;;
  elasticsearch-secure)
    PORT="${PORT:-19203}"
    NAME="it-es-sec-${RUN_ID}"
    ADMIN_PW="${IT_ADMIN_PW:-$(gen_secret)}"
    if [[ $ES_OFFICIAL == 1 ]]; then
      CERTS=$(es_pki)
      C=/usr/share/elasticsearch/config/certs
      engine run -d --name "$NAME" --label elasticdump-rs-it=1 "${CAPS[@]}" \
        -p "127.0.0.1:${PORT}:9200" \
        -v "$CERTS:$C:ro,Z" \
        -e discovery.type=single-node \
        -e xpack.ml.enabled=false \
        -e xpack.security.enabled=true \
        -e "ELASTIC_PASSWORD=${ADMIN_PW}" \
        -e xpack.security.http.ssl.enabled=true \
        -e "xpack.security.http.ssl.key=$C/tls.key" \
        -e "xpack.security.http.ssl.certificate=$C/tls.crt" \
        -e "xpack.security.http.ssl.certificate_authorities=$C/ca.crt" \
        -e xpack.security.http.ssl.client_authentication=optional \
        -e "ES_JAVA_OPTS=-Xms${ES_HEAP} -Xmx${ES_HEAP} ${LEAN_OPTS}" \
        "$ES_IMAGE" >/dev/null
      URL="https://localhost:${PORT}"
      CURL=(curl -fsS --cacert "$CERTS/ca.crt" -u "elastic:${ADMIN_PW}")
    else
    DATA=$(es_data_dir "$NAME")
    CERTS=$(es_pki)
    # ES 9 entitlements only let the SSL module read files under config/
    engine run -d --name "$NAME" --label elasticdump-rs-it=1 "${CAPS[@]}" \
      -p "127.0.0.1:${PORT}:9200" \
      -v "$DATA:/bitnami/elasticsearch/data:Z" \
      -v "$CERTS:/opt/bitnami/elasticsearch/config/certs:ro,Z" \
      -v "$CERTS/my_elasticsearch.yml:/opt/bitnami/elasticsearch/config/my_elasticsearch.yml:ro,Z" \
      -e "ELASTICSEARCH_HEAP_SIZE=${ES_HEAP}" \
      -e "ES_JAVA_OPTS=${LEAN_OPTS}" \
      -e ELASTICSEARCH_DISABLE_GC_LOGS=true \
      -e ELASTICSEARCH_ENABLE_SECURITY=true \
      -e "ELASTICSEARCH_PASSWORD=${ADMIN_PW}" \
      -e ELASTICSEARCH_ENABLE_REST_TLS=true \
      -e ELASTICSEARCH_TLS_USE_PEM=true \
      -e ELASTICSEARCH_TLS_VERIFICATION_MODE=certificate \
      -e ELASTICSEARCH_NODE_CERT_LOCATION=/opt/bitnami/elasticsearch/config/certs/tls.crt \
      -e ELASTICSEARCH_NODE_KEY_LOCATION=/opt/bitnami/elasticsearch/config/certs/tls.key \
      -e ELASTICSEARCH_CA_CERT_LOCATION=/opt/bitnami/elasticsearch/config/certs/ca.crt \
      "$ES_IMAGE" >/dev/null
    URL="https://localhost:${PORT}"
    CURL=(curl -fsS --cacert "$CERTS/ca.crt" -u "elastic:${ADMIN_PW}")
    fi
    ;;
  *)
    echo "unknown profile: $PROFILE" >&2
    exit 2
    ;;
esac

echo "waiting for $NAME at $URL ..." >&2
ok=0
for _ in $(seq 1 120); do
  if [[ "$(engine inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null)" != true ]]; then
    break
  fi
  if "${CURL[@]}" "${URL}/_cluster/health?wait_for_status=yellow&timeout=5s" >/dev/null 2>&1; then
    ok=1
    break
  fi
  sleep 2
done
if [[ $ok -ne 1 ]]; then
  echo "cluster did not become healthy; container logs:" >&2
  engine logs --tail 80 "$NAME" >&2 || true
  exit 1
fi

{
  echo "export IT_PROFILE=${PROFILE}"
  echo "export IT_ENGINE=${PROFILE%%-*}"
  echo "export IT_ES_URL=${URL}"
  echo "export IT_CONTAINER=${NAME}"
} > "$IT_DIR/env"

if [[ "$PROFILE" == opensearch-secure ]]; then
  CFG=/usr/share/opensearch/config
  engine cp "$NAME:$CFG/root-ca.pem" "$IT_DIR/root-ca.pem"
  engine cp "$NAME:$CFG/kirk.pem" "$IT_DIR/client.pem"
  engine cp "$NAME:$CFG/kirk-key.pem" "$IT_DIR/client-key.pem"
  openssl pkcs8 -topk8 -v2 aes-256-cbc -in "$IT_DIR/client-key.pem" \
    -out "$IT_DIR/client-key-enc.pem" -passout "pass:$KEY_PASS" 2>/dev/null
  {
    echo "export IT_ADMIN_USER=admin"
    printf "export IT_ADMIN_PW=%q\n" "$ADMIN_PW"
    echo "export IT_CA=$IT_DIR/root-ca.pem"
    echo "export IT_CLIENT_CERT=$IT_DIR/client.pem"
    echo "export IT_CLIENT_KEY=$IT_DIR/client-key.pem"
    echo "export IT_CLIENT_KEY_ENC=$IT_DIR/client-key-enc.pem"
    printf "export IT_CLIENT_KEY_PASS=%q\n" "$KEY_PASS"
  } >> "$IT_DIR/env"
fi

if [[ "$PROFILE" == elasticsearch-secure ]]; then
  {
    echo "export IT_ADMIN_USER=elastic"
    printf "export IT_ADMIN_PW=%q\n" "$ADMIN_PW"
    echo "export IT_CA=$CERTS/ca.crt"
    echo "export IT_CLIENT_CERT=$CERTS/client.pem"
    echo "export IT_CLIENT_KEY=$CERTS/client-key.pem"
    echo "export IT_CLIENT_KEY_ENC=$CERTS/client-key-enc.pem"
    printf "export IT_CLIENT_KEY_PASS=%q\n" "$KEY_PASS"
    # client certs do not authenticate on their own here (see es_pki)
    echo "export IT_CERT_AUTH=0"
  } >> "$IT_DIR/env"
elif [[ "$PROFILE" == opensearch-secure ]]; then
  echo "export IT_CERT_AUTH=1" >> "$IT_DIR/env"
fi

echo "ready: $URL (env: $IT_DIR/env)" >&2
