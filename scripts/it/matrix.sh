#!/usr/bin/env bash
# Engine matrix shared by scripts/it/run.sh and scripts/load/run.sh (sourced).
ES8_TAG="${ES8_TAG:-8.6.1-debian-11-r0}"
# Regression matrix (exact tags verified against the registries 2026-10-29;
# see TESTING-MATRIX.md for sources and digests):
OFFICIAL_ES=docker.elastic.co/elasticsearch/elasticsearch
ES717_IMAGE="${ES717_IMAGE:-$OFFICIAL_ES:7.17.29}"      # bitnamilegacy stops at 7.17.26
ES710_PLAIN_IMAGE="${ES710_PLAIN_IMAGE:-docker.io/bitnamilegacy/elasticsearch:7.10.2}"  # OSS build
ES710_SECURE_IMAGE="${ES710_SECURE_IMAGE:-$OFFICIAL_ES:7.10.2}"  # bitnami 7.10.2 has no X-Pack security
ES819_IMAGE="${ES819_IMAGE:-$OFFICIAL_ES:8.19.23}"      # bitnamilegacy stops at 8.18.0
OS13_TAG="${OS13_TAG:-1.3.20}"

# Map a matrix profile (es717-plain, os13-secure, ...) onto up.sh's engine
# profile plus image env. Prints the up.sh profile.
matrix_env() {
  local p="$1" variant="${1##*-}"
  case "$p" in
    elasticsearch8-*) export ES_TAG="$ES8_TAG" ES_IMAGE="docker.io/bitnamilegacy/elasticsearch:$ES8_TAG"; echo "elasticsearch-$variant" ;;
    es717-*) export ES_TAG=7.17.29 ES_IMAGE="$ES717_IMAGE"; echo "elasticsearch-$variant" ;;
    es819-*) export ES_TAG=8.19.23 ES_IMAGE="$ES819_IMAGE"; echo "elasticsearch-$variant" ;;
    es710-plain) export ES_TAG=7.10.2 ES_IMAGE="$ES710_PLAIN_IMAGE"; echo elasticsearch-plain ;;
    es710-secure) export ES_TAG=7.10.2 ES_IMAGE="$ES710_SECURE_IMAGE"; echo elasticsearch-secure ;;
    os13-*) export OS_TAG="$OS13_TAG" OS_IMAGE="docker.io/opensearchproject/opensearch:$OS13_TAG"; echo "opensearch-$variant" ;;
    *) echo "$p" ;;
  esac
}

# CLI: `matrix.sh <profile>` prints the up.sh profile and the image env as shell exports.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  up=$(matrix_env "$1"); matrix_env "$1" >/dev/null
  echo "UP_PROFILE=$up"
  for v in ES_TAG ES_IMAGE OS_TAG OS_IMAGE; do [[ -n "${!v:-}" ]] && echo "$v=${!v}"; done
  exit 0
fi
