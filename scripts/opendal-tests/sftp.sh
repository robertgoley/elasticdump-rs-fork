#!/usr/bin/env bash
# L-809a: OpenDAL sftp:// tests against a local SSH server in podman (--network=host):
#   scripts/opendal-tests/sftp.sh openssh|dropbear
# openssh: OpenSSH sshd + sftp-server. dropbear: Dropbear sshd, which has no SFTP subsystem of
# its own and runs OpenSSH's sftp-server binary (the usual pairing).
# OpenDAL's sftp service drives the system `ssh` client, which keeps host keys in
# ~/.ssh/known_hosts: the test removes the [127.0.0.1]:PORT entries it caused (ssh-keygen -R).
# ODT_START_ONLY=1: start the server, write $WORK/sftp-$SERVER.env, leave it running.
set -euo pipefail
SERVER=${1:?usage: sftp.sh openssh|dropbear}
. "$(dirname "$0")/common.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
MULTI=${EDRS_MULTI_BIN:-$(dirname "$BIN")/multielasticdump}
case $SERVER in
  openssh) PORT=${ODT_OPENSSH_PORT:-19922} ;;
  dropbear) PORT=${ODT_DROPBEAR_PORT:-19923} ;;
  *) echo "unknown server $SERVER" >&2; exit 2 ;;
esac
NAME=edrs-odt-$SERVER
IMAGE=localhost/edrs-odt-$SERVER:latest
unset SSH_AUTH_SOCK
podman build --network=host -q -t "$IMAGE" -f "$HERE/containers/$SERVER.Containerfile" "$HERE/containers" >/dev/null

KEYS=$WORK/ssh-$SERVER
mkdir -p "$KEYS"
ssh-keygen -q -t ed25519 -N '' -C edrs-odt -f "$KEYS/id_ed25519"
cp "$KEYS/id_ed25519.pub" "$KEYS/odt"
chmod 755 "$KEYS"; chmod 644 "$KEYS/odt"
forget_host() { ssh-keygen -R "[127.0.0.1]:$PORT" >/dev/null 2>&1 || true; rm -f ~/.ssh/known_hosts.old; }
forget_host

stop_container "$NAME"
if [ "${ODT_START_ONLY:-0}" != 1 ]; then
  trap '[ "${KEEP:-0}" = 1 ] || stop_container "$NAME"; forget_host' EXIT
fi
case $SERVER in
  openssh)
    podman run -d --name "$NAME" --network=host -v "$KEYS/odt:/etc/ssh/keys/odt:ro,Z" "$IMAGE" \
      sh -c "ssh-keygen -A >/dev/null && exec /usr/sbin/sshd -D -e -p $PORT -o ListenAddress=127.0.0.1 \
        -o PasswordAuthentication=no -o KbdInteractiveAuthentication=no \
        -o AuthorizedKeysFile=/etc/ssh/keys/%u -o 'Subsystem sftp /usr/lib/ssh/sftp-server'" >/dev/null ;;
  dropbear)
    podman run -d --name "$NAME" --network=host -v "$KEYS/odt:/keys/odt:ro,Z" "$IMAGE" \
      sh -c "mkdir -p /home/odt/.ssh && cp /keys/odt /home/odt/.ssh/authorized_keys \
        && chown -R odt:odt /home/odt/.ssh && chmod 700 /home/odt/.ssh && chmod 600 /home/odt/.ssh/authorized_keys \
        && exec dropbear -F -E -R -s -p 127.0.0.1:$PORT" >/dev/null ;;
esac
wait_port "$PORT" || { echo "server did not start"; podman logs "$NAME" | tail -n 30; bad "$SERVER start"; summary "sftp ($SERVER)"; }
sleep 1

CFG=$WORK/sftp-$SERVER.ini
printf 'key_file = %s\nknown_hosts = accept\n' "$KEYS/id_ed25519" >"$CFG"
chmod 600 "$CFG"
URL=sftp://odt@127.0.0.1:$PORT/home/odt
if [ "${ODT_START_ONLY:-0}" = 1 ]; then
  printf 'SFTP_URL=%s\nSFTP_CFG=%s\nSFTP_PORT=%s\n' "$URL" "$CFG" "$PORT" >"$WORK/sftp-$SERVER.env"
  log "$SERVER running at 127.0.0.1:$PORT (stop: podman rm -f $NAME)"
  exit 0
fi
log "$SERVER at 127.0.0.1:$PORT ($(podman exec "$NAME" sh -c 'dropbear -V 2>&1 | head -n 1; sshd -V 2>&1 | head -n 1' 2>/dev/null | grep -v 'not found' | head -n 1))"

IN=$WORK/in.json
make_input "$IN"
ed() { "$BIN" --quiet "$@"; }

# strict host checking (the default) refuses the unknown test server
STRICT=$WORK/sftp-$SERVER-strict.ini
printf 'key_file = %s\n' "$KEYS/id_ed25519" >"$STRICT"
check_fails "$SERVER: unknown host key refused with known_hosts=strict (default)" "known_hosts" \
  ed --outputStorageConfig "$STRICT" --input "$IN" --output "$URL/strict.json"
check "$SERVER: local -> sftp (plain)" ed --outputStorageConfig "$CFG" --input "$IN" --output "$URL/plain.json"
check "$SERVER: sftp -> local (plain)" ed --inputStorageConfig "$CFG" --input "$URL/plain.json" --output "$WORK/plain.back.json"
check "$SERVER: plain round trip keeps every document" same_docs "$IN" "$WORK/plain.back.json"
check "$SERVER: file is on the server" podman exec "$NAME" test -s /home/odt/plain.json
check "$SERVER: local -> sftp gzip split (--maxRows 2000)" ed --outputStorageConfig "$CFG" --input "$IN" --output "$URL/gz/d.json" --fsCompress --maxRows 2000
: >"$WORK/gz.back.json"
for n in 0 1 2; do
  ed --inputStorageConfig "$CFG" --input "$URL/gz/d.split-$n.json.gz" --output "$WORK/gz.$n.json" >>"$WORK/test.log" 2>&1 && cat "$WORK/gz.$n.json" >>"$WORK/gz.back.json"
done
check "$SERVER: gzip split round trip keeps every document" same_docs "$IN" "$WORK/gz.back.json"
check "$SERVER: parts are gzip on the server" sh -c "podman exec $NAME head -c 2 /home/odt/gz/d.split-0.json.gz | od -An -tx1 | grep -q '1f 8b'"
check_fails "$SERVER: existing file refused without --overwrite" "already exists" ed --outputStorageConfig "$CFG" --input "$IN" --output "$URL/plain.json"
check "$SERVER: --overwrite replaces it" ed --outputStorageConfig "$CFG" --input "$IN" --output "$URL/plain.json" --overwrite
check_fails "$SERVER: password in the URL refused" "SSH key" ed --input "$IN" --output "sftp://odt:FAKE-pw-1@127.0.0.1:$PORT/home/odt/x.json"

TREE=$WORK/tree; mkdir -p "$TREE/_cluster" "$TREE/g1"
cp "$IN" "$TREE/odt.data.json"; gzip -c "$IN" >"$TREE/g1/odt2.data.split-0.json.gz"
echo '{}' >"$TREE/_cluster/policy.json"; echo '{"version":1}' >"$TREE/multielasticdump.json"
check "$SERVER: multielasticdump copy local dir -> sftp" "$MULTI" --direction copy --outputStorageConfig "$CFG" --input "$TREE" --output "$URL/tree"
check "$SERVER: multielasticdump copy sftp -> local dir" "$MULTI" --direction copy --inputStorageConfig "$CFG" --input "$URL/tree" --output "$WORK/tree.back"
check "$SERVER: copied tree identical (diff -r)" diff -r "$TREE" "$WORK/tree.back"

summary "sftp ($SERVER)"
