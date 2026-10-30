#!/usr/bin/env bash
# INF-012 / F-14: every image CI can run is pinned by digest, and the digests
# recorded in TESTING-MATRIX.md match scripts/it/images.lock. A re-pushed tag
# therefore cannot change what runs without a diff to images.lock.
# Exit 0 = all pinned and consistent; 1 = drift (each problem printed).
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
lock="$root/scripts/it/images.lock"
matrix_md="$root/TESTING-MATRIX.md"
# shellcheck source=matrix.sh
source "$root/scripts/it/matrix.sh"
fail=0
bad() { echo "check-pins: $*" >&2; fail=1; }
digest_of() { awk -v i="$1" '$1 == i {print $2}' "$lock"; }

# 1. every engine image of every profile is in images.lock
images=()
for os_tag in 2 3; do images+=("docker.io/opensearchproject/opensearch:$os_tag"); done
images+=("docker.io/bitnamilegacy/elasticsearch:9.1.2-debian-12-r0")
for p in elasticsearch8-plain es717-plain es710-plain es710-secure es819-plain os13-plain; do
  img=$( (unset ES_IMAGE OS_IMAGE; matrix_env "$p" >/dev/null; echo "${ES_IMAGE:-${OS_IMAGE:-}}") )
  images+=("$img")
done
images+=("ghcr.io/shopify/toxiproxy:2.12.0")
for img in "${images[@]}"; do
  [[ -n "$(digest_of "$img")" ]] || bad "$img is not pinned in scripts/it/images.lock"
done

# 2. lock entries are well formed and recorded in TESTING-MATRIX.md
while read -r img d; do
  [[ -z "$img" || "$img" == \#* ]] && continue
  [[ "$d" =~ ^sha256:[0-9a-f]{64}$ ]] || { bad "malformed digest for $img: $d"; continue; }
  grep -qF "$d" "$matrix_md" || bad "$img digest $d is not recorded in TESTING-MATRIX.md"
done < "$lock"

# 3. Dockerfile bases and compose images carry a digest
grep -E '^ARG (BUILDER|BASE)_TAG=' "$root/Dockerfile" | grep -vq '@sha256:' && bad "Dockerfile base tag without @sha256"
while read -r line; do bad "docker-compose.yaml: unpinned $line"; done \
  < <(grep -E '^\s*image:' "$root/docker-compose.yaml" | grep -v '@sha256:' || true)

[[ $fail == 0 ]] && echo "check-pins: ${#images[@]} profile images pinned; images.lock consistent with TESTING-MATRIX.md"
exit $fail
