#!/usr/bin/env bash
# Live cluster-to-cluster pairs (TST-API-072): the `cross` profile (one OpenSearch and one
# Elasticsearch node, small heaps, memory-guarded) once per pair, sequentially. Each pair runs
# the cross:: cluster tests plus it::xmatrix::cluster_to_cluster_pair_every_type, which checks
# every type in both directions against tests/golden/cross-expectations.toml.
#
#   scripts/it/xpairs.sh            # default pairs below
#   XPAIRS="os13:es717 2:es819" scripts/it/xpairs.sh
#
# A pair is "<OS_TAG or os13>:<es717|es819|es9|es86>". The ES OSS 7.10 build is not paired
# (no ILM; the cross:: policy test needs it); the full 9-image pair set in parallel is phase B.
set -uo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=matrix.sh
source scripts/it/matrix.sh
pairs="${XPAIRS:-2:es9 os13:es717 3:es819 2:es86}"
rc=0
for pr in $pairs; do
  os=${pr%%:*}; es=${pr#*:}
  (
    unset OS_TAG OS_IMAGE ES_TAG ES_IMAGE
    case "$os" in
      os13) export OS_TAG="$OS13_TAG" OS_IMAGE="docker.io/opensearchproject/opensearch:$OS13_TAG" ;;
      *) export OS_TAG="$os" ;;
    esac
    case "$es" in
      es717) matrix_env es717-plain >/dev/null ;;
      es819) matrix_env es819-plain >/dev/null ;;
      es86) matrix_env elasticsearch8-plain >/dev/null ;;
      es9) ;;
      *) echo "unknown ES side $es" >&2; exit 2 ;;
    esac
    export ES_TAG ES_IMAGE OS_TAG OS_IMAGE
    CROSS_REQUIRED=1 IT_LEGACY=0 CARGO_JOBS="${CARGO_JOBS:-2}" scripts/it/run.sh cross xmatrix::cluster_to_cluster_pair_every_type --nocapture
  ) && echo "XPAIR $pr ok" || { echo "XPAIR $pr FAILED"; rc=1; }
done
exit $rc
