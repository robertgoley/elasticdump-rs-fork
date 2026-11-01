#!/usr/bin/env bash
# Build the elasticdump-rs container image (L-808b). Local build only unless --push is given.
#
#   scripts/build-image.sh [--variant glibc|alpine] [--platform linux/amd64|linux/arm64|both]
#                          [--tag REF]... [--name REPO] [--jobs N] [--push] [--dry-run] [--help]
#
# Variants (pins: Makefile; stages: Dockerfile):
#   glibc   (default) dhi.io/rust:*-debian13-dev builder -> dhi.io/static:*-glibc-debian13 runtime,
#           Dockerfile target `runtime` (COPY-only, no shell); mimalloc global allocator.
#           Default tags: REPO:<version> and REPO:latest
#   alpine  dhi.io/rust:*-alpine-dev builder -> dhi.io/alpine-base:*-dev runtime, target
#           `runtime-shell` (bash, apk); musl's own allocator. Default tag: REPO:<version>-alpine
# REPO defaults to localhost/elasticdump-rs; <version> comes from Cargo.toml.
#
# --platform  linux/amd64 | linux/arm64 | both (default: the host's platform). A foreign platform
#             runs the Rust build under emulation (qemu-user-static / binfmt_misc on the host) and
#             is slow; `both` builds a manifest list (podman build --manifest).
# --tag REF   full image reference, repeatable; replaces the default tags.
# --push      push the tags after the build. Registry auth comes only from the container tool's
#             existing login (`podman login <registry>`); this script never reads or takes credentials.
# --dry-run   print the commands without running them.
# dhi.io bases need a pull login first: podman login dhi.io
#
# Environment: CONTAINER (podman | docker; default podman if installed). `both` and --push of a
# manifest list need podman.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }
die() { echo "build-image.sh: $*" >&2; exit 2; }

variant=glibc; platform=""; name=localhost/elasticdump-rs; jobs=2; push=0; dry=0
tags=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) [[ $# -ge 2 ]] || die "--variant needs a value"; variant="$2"; shift 2 ;;
    --variant=*) variant="${1#*=}"; shift ;;
    --platform) [[ $# -ge 2 ]] || die "--platform needs a value"; platform="$2"; shift 2 ;;
    --platform=*) platform="${1#*=}"; shift ;;
    --tag) [[ $# -ge 2 ]] || die "--tag needs a value"; tags+=("$2"); shift 2 ;;
    --tag=*) tags+=("${1#*=}"); shift ;;
    --name) [[ $# -ge 2 ]] || die "--name needs a value"; name="$2"; shift 2 ;;
    --name=*) name="${1#*=}"; shift ;;
    --jobs) [[ $# -ge 2 ]] || die "--jobs needs a value"; jobs="$2"; shift 2 ;;
    --jobs=*) jobs="${1#*=}"; shift ;;
    --push) push=1; shift ;;
    --dry-run) dry=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown option: $1" ;;
  esac
done

C="${CONTAINER:-$(command -v podman >/dev/null 2>&1 && echo podman || echo docker)}"
[[ "$jobs" =~ ^[1-9][0-9]*$ ]] || die "--jobs must be a positive integer"

# Pins live in the Makefile (one place, F-14 checked by scripts/lint-hygiene.sh).
mk() { sed -n "s/^$1[[:space:]]*?=[[:space:]]*//p" Makefile | head -1; }
case "$variant" in
  glibc)  target=runtime;       pre="" ;;
  alpine) target=runtime-shell; pre=DHI_ALPINE_ ;;
  *) die "--variant must be glibc or alpine (got '$variant')" ;;
esac
builder_image=$(mk "${pre}BUILDER_IMAGE"); builder_tag=$(mk "${pre}BUILDER_TAG")
base_image=$(mk "${pre}BASE_IMAGE"); base_tag=$(mk "${pre}BASE_TAG")
for v in builder_image builder_tag base_image base_tag; do
  [[ -n "${!v}" ]] || die "could not read the ${pre}${v^^} pin from the Makefile"
done

case "$platform" in
  "") platforms="" ;;
  linux/amd64|linux/arm64) platforms="$platform" ;;
  both|linux/amd64,linux/arm64|linux/arm64,linux/amd64) platforms="linux/amd64,linux/arm64" ;;
  *) die "--platform must be linux/amd64, linux/arm64 or both (got '$platform')" ;;
esac
multi=0; [[ "$platforms" == *,* ]] && multi=1
if [[ $multi == 1 && "$C" != podman ]]; then die "--platform both needs podman (CONTAINER=$C)"; fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[[ -n "$version" ]] || die "no version in Cargo.toml"
if [[ ${#tags[@]} -eq 0 ]]; then
  if [[ "$variant" == glibc ]]; then tags=("$name:$version" "$name:latest"); else tags=("$name:$version-alpine"); fi
fi

run() { printf '+'; printf ' %q' "$@"; printf '\n'; [[ $dry == 1 ]] || "$@"; }

args=(build --target "$target"
  --build-arg "BUILDER_IMAGE=$builder_image" --build-arg "BUILDER_TAG=$builder_tag"
  --build-arg "BASE_IMAGE=$base_image" --build-arg "BASE_TAG=$base_tag"
  --build-arg "VERSION=$version"
  --build-arg "VCS_REF=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
  --build-arg "BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  --build-arg "CARGO_BUILD_JOBS=$jobs")
[[ -n "$platforms" ]] && args+=(--platform "$platforms")

echo "variant=$variant target=$target platform=${platforms:-host} builder=$builder_image:$builder_tag base=$base_image:$base_tag"
if [[ $multi == 1 ]]; then
  # A manifest list: build into the first tag, then tag the list with the others.
  if [[ $dry == 0 ]] && $C manifest exists "${tags[0]}" 2>/dev/null; then run $C manifest rm "${tags[0]}" >/dev/null; fi
  run $C "${args[@]}" --manifest "${tags[0]}" .
  for t in "${tags[@]:1}"; do run $C tag "${tags[0]}" "$t"; done
else
  targs=(); for t in "${tags[@]}"; do targs+=(-t "$t"); done
  run $C "${args[@]}" "${targs[@]}" .
fi

if [[ $push == 1 ]]; then
  for t in "${tags[@]}"; do
    if [[ $multi == 1 ]]; then run $C manifest push --all "$t" "docker://$t"; else run $C push "$t"; fi
  done
else
  echo "built (local only, not pushed): ${tags[*]}"
fi
