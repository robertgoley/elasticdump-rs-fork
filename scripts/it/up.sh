#!/usr/bin/env bash
# Start a single-node OpenSearch container for the integration suite.
#
#   scripts/it/up.sh opensearch-plain|opensearch-secure
#
# Writes $IT_DIR/env (source it) with IT_ES_URL, IT_PROFILE and, for the
# secure profile, IT_ADMIN_USER/IT_ADMIN_PW/IT_CA/IT_CLIENT_CERT/IT_CLIENT_KEY/
# IT_CLIENT_KEY_ENC/IT_CLIENT_KEY_PASS. Containers carry the label
# elasticdump-rs-it=1 so down.sh can always find them.
set -euo pipefail

PROFILE="${1:-opensearch-plain}"
OS_TAG="${OS_TAG:-2}"
OS_IMAGE="${OS_IMAGE:-docker.io/opensearchproject/opensearch:${OS_TAG}}"
RUN_ID="${RUN_ID:-$$}"
IT_DIR="${IT_DIR:-${TMPDIR:-/tmp}/elasticdump-rs-it}"
HEAP="${OS_HEAP:-512m}"
mkdir -p "$IT_DIR"

engine() { podman "$@"; }

case "$PROFILE" in
  opensearch-plain)
    PORT="${PORT:-19200}"
    NAME="it-os-plain-${RUN_ID}"
    engine run -d --name "$NAME" --label elasticdump-rs-it=1 \
      -p "127.0.0.1:${PORT}:9200" \
      -e discovery.type=single-node \
      -e DISABLE_SECURITY_PLUGIN=true -e DISABLE_INSTALL_DEMO_CONFIG=true \
      -e "OPENSEARCH_JAVA_OPTS=-Xms${HEAP} -Xmx${HEAP}" \
      "$OS_IMAGE" >/dev/null
    URL="http://127.0.0.1:${PORT}"
    CURL=(curl -fsS)
    ;;
  opensearch-secure)
    PORT="${PORT:-19201}"
    NAME="it-os-sec-${RUN_ID}"
    ADMIN_PW="${IT_ADMIN_PW:-Str0ng!Passw0rd#2026}"
    engine run -d --name "$NAME" --label elasticdump-rs-it=1 \
      -p "127.0.0.1:${PORT}:9200" \
      -e discovery.type=single-node \
      -e "OPENSEARCH_INITIAL_ADMIN_PASSWORD=${ADMIN_PW}" \
      -e "OPENSEARCH_JAVA_OPTS=-Xms${HEAP} -Xmx${HEAP}" \
      "$OS_IMAGE" >/dev/null
    URL="https://localhost:${PORT}"
    CURL=(curl -fsS -k -u "admin:${ADMIN_PW}")
    ;;
  *)
    echo "unknown profile: $PROFILE" >&2
    exit 2
    ;;
esac

echo "waiting for $NAME at $URL ..." >&2
ok=0
for _ in $(seq 1 90); do
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
  echo "export IT_ES_URL=${URL}"
  echo "export IT_CONTAINER=${NAME}"
} > "$IT_DIR/env"

if [[ "$PROFILE" == opensearch-secure ]]; then
  CFG=/usr/share/opensearch/config
  engine cp "$NAME:$CFG/root-ca.pem" "$IT_DIR/root-ca.pem"
  engine cp "$NAME:$CFG/kirk.pem" "$IT_DIR/client.pem"
  engine cp "$NAME:$CFG/kirk-key.pem" "$IT_DIR/client-key.pem"
  openssl pkcs8 -topk8 -v2 aes-256-cbc -in "$IT_DIR/client-key.pem" \
    -out "$IT_DIR/client-key-enc.pem" -passout pass:it-secret 2>/dev/null
  {
    echo "export IT_ADMIN_USER=admin"
    printf "export IT_ADMIN_PW=%q\n" "$ADMIN_PW"
    echo "export IT_CA=$IT_DIR/root-ca.pem"
    echo "export IT_CLIENT_CERT=$IT_DIR/client.pem"
    echo "export IT_CLIENT_KEY=$IT_DIR/client-key.pem"
    echo "export IT_CLIENT_KEY_ENC=$IT_DIR/client-key-enc.pem"
    echo "export IT_CLIENT_KEY_PASS=it-secret"
  } >> "$IT_DIR/env"
fi

echo "ready: $URL (env: $IT_DIR/env)" >&2
