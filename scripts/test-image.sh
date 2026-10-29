#!/usr/bin/env bash
# Image hardening tests (TST-SUP-017, L-211); `make test-image` runs this.
#   scripts/test-image.sh <image> [variant]   variant selects docker/image-binaries.<variant>.txt
#                                             (default dhi-alpine; --update rewrites it after review)
# Checks:
#   1. --version / --help work; the entrypoint is elasticdump-rs; bash and the CA bundle are present
#   2. it runs as non-root (65532), and the working dir is /data
#   3. read-only root filesystem: a file->file dump into a writable /data volume works under
#      --read-only --cap-drop ALL --security-opt no-new-privileges, and writing anywhere else fails
#   4. no executables beyond the documented set (docker/image-binaries.<variant>.txt)
#   5. the base image label is pinned by digest (org.opencontainers.image.base.name has @sha256:)
# Image package SBOM diff: scripts/sup/sbom-pr.sh with IMAGE_SBOM_BASE/HEAD (SUP-011).
set -uo pipefail
cd "$(dirname "$0")/.."
IMG="${1:?usage: test-image.sh <image> [variant] [--update]}"; VAR="${2:-dhi-alpine}"
C="${CONTAINER:-podman}"; fail=0
ok() { echo "PASS $*"; }; bad() { echo "FAIL $*"; fail=1; }
$C run --rm "$IMG" --version >/dev/null && ok "--version" || bad "--version"
$C run --rm "$IMG" --help >/dev/null && ok "--help" || bad "--help"
$C run --rm --entrypoint /bin/bash "$IMG" -c 'command -v elasticdump >/dev/null && test -s /etc/ssl/certs/ca-certificates.crt' \
  && ok "bash, elasticdump symlink, CA bundle" || bad "bash/symlink/CA bundle"
u=$($C run --rm --entrypoint /bin/sh "$IMG" -c 'id -u'); [[ "$u" != 0 && -n "$u" ]] && ok "non-root (uid $u)" || bad "runs as root"
wd=$($C image inspect "$IMG" --format '{{.Config.WorkingDir}}'); [[ "$wd" == /data ]] && ok "workdir /data" || bad "workdir $wd"
W=$(mktemp -d); trap 'rm -rf "$W"; $C volume rm -f edl-ti-$$ >/dev/null 2>&1' EXIT
printf '{"_index":"i","_id":"1","_source":{"a":1}}\n{"_index":"i","_id":"2","_source":{"b":"x"}}\n' > "$W/in.ndjson"; chmod 644 "$W/in.ndjson"
$C volume create edl-ti-$$ >/dev/null
if $C run --rm --read-only --cap-drop ALL --security-opt no-new-privileges --network none \
     -v "$W/in.ndjson:/in/in.ndjson:ro,Z" -v edl-ti-$$:/data "$IMG" --input /in/in.ndjson --output /data/out.ndjson >/dev/null 2>&1 \
   && [[ "$($C run --rm -v edl-ti-$$:/data --entrypoint /bin/sh "$IMG" -c 'wc -l < /data/out.ndjson')" == 2 ]]; then
  ok "read-only rootfs + writable /data volume: dump OK (cap-drop ALL, no-new-privileges, no network)"
else bad "read-only rootfs dump"; fi
$C run --rm --read-only --entrypoint /bin/sh "$IMG" -c 'touch /usr/local/bin/x 2>/dev/null || touch /etc/x 2>/dev/null' \
  && bad "rootfs writable under --read-only" || ok "rootfs not writable under --read-only"
$C run --rm --read-only --entrypoint /bin/sh "$IMG" -c 'find / -xdev -type f -perm -u+x 2>/dev/null | sort' > "$W/bins.txt"
BL="docker/image-binaries.$VAR.txt"
if [[ "${3:-}" == --update ]]; then cp "$W/bins.txt" "$BL"; echo "updated $BL"; fi
if [[ -f "$BL" ]]; then
  extra=$(comm -13 "$BL" "$W/bins.txt")
  [[ -z "$extra" ]] && ok "executables within documented set ($(wc -l < "$W/bins.txt") files, $BL)" || { bad "undocumented executables:"; echo "$extra" | sed 's/^/  + /'; }
else bad "no baseline $BL (review, then rerun with: $0 $IMG $VAR --update)"; fi
base=$($C image inspect "$IMG" --format '{{index .Labels "org.opencontainers.image.base.name"}}')
[[ "$base" == *@sha256:* ]] && ok "base pinned by digest ($base)" || bad "base not pinned by digest: $base"
exit $fail
