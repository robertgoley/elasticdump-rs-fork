#!/usr/bin/env bash
# Build the searchdump container image (L-808b). Local build only unless --push is given.
#
#   scripts/build-image.sh [--type TYPE] [--platform linux/amd64|linux/arm64|both]
#                          [--imagename REF] [--tag REF]... [--name REPO] [--jobs N] [--push]
#                          [--dry-run] [--help]
#
# Build types (pins: Makefile; stages: Dockerfile):
#   dhi-debian  (default, L-810c) dhi.io/rust:*-debian13-dev builder -> dhi.io/debian-base:*-debian13-dev
#               runtime, target `runtime-shell` (bash, apt); mimalloc. Tags: REPO:<version>, REPO:latest
#   dhi-static  dhi.io/rust:*-debian13-dev builder -> dhi.io/static:*-glibc-debian13 runtime, target
#               `runtime` (COPY-only, no shell); mimalloc. Tag: REPO:<version>-static (default before L-810c)
#   dhi-alpine  dhi.io/rust:*-alpine-dev builder -> dhi.io/alpine-base:*-dev runtime, target
#               `runtime-shell` (bash, apk); musl's own allocator. Tag: REPO:<version>-alpine
#   public      rust:*-trixie -> debian:trixie-slim (public bases, no login; not default).
#               Tag: REPO:<version>-debian-public
#   alpine      rust:*-alpine -> alpine (public bases, no login; not default). Tag: REPO:<version>-alpine-public
# --variant glibc|alpine (before L-810c) still works: glibc = dhi-static, alpine = dhi-alpine.
# Every type has the compatibility symlinks elasticdump, elasticdump-rs (-> searchdump) and
# multielasticdump (-> searchdump-multi) in /usr/local/bin.
# REPO defaults to localhost/searchdump-rs; <version> comes from Cargo.toml.
#
# --platform  linux/amd64 | linux/arm64 | both (default: linux/amd64, L-810c). A foreign platform
#             runs the Rust build under emulation (qemu-user-static / binfmt_misc on the host) and
#             is slow; `both` builds a manifest list (podman build --manifest).
# --imagename REF  full repository and tag for the image, e.g. registry.example.com/team/searchdump-rs:1.2.3;
#             replaces the default tags. Same as the SEARCHDUMP_IMAGE environment variable (the option wins).
# --tag REF   additional full image reference, repeatable; with no --imagename it replaces the default tags.
# --push      push the tags after the build. Registry auth comes only from the container tool's
#             existing login (`podman login <registry>`); this script never reads or takes credentials.
# --dry-run   print the commands without running them.
# --preflight IMG  (repeatable) only run the prerequisite checks for these base images, then exit
#             (used by the Makefile image targets).
# Prerequisites: podman (or docker with buildx); no host Rust (crates are vendored in a container;
# VENDOR_ON_HOST=1 uses host cargo >= 1.91 instead). For dhi.io bases a pull
# login first: podman login dhi.io (checked up front; --type public / alpine need no login).
#
# Offline build (L-808i): crates are fetched on the HOST (scripts/vendor-crates.sh into the
# gitignored .vendor/, so the host needs cargo) and the builder's cargo step runs with
# --network=none. Only the host needs registry access (or a proxy). Remove .vendor/ with
# `rm -rf .vendor` when done; it is refreshed on every build.
#
# Environment: SEARCHDUMP_IMAGE (as --imagename); CONTAINER (podman | docker; default podman if installed). `both` and --push of a
# manifest list need podman.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }
die() { echo "build-image.sh: $*" >&2; exit 2; }
# --variant (before L-810c): glibc was the dhi.io static image, alpine the dhi.io Alpine image.
old_variant() { case "$1" in glibc) echo dhi-static ;; alpine) echo dhi-alpine ;; *) echo "$1" ;; esac; }

btype=dhi-debian; platform=linux/amd64; name=localhost/searchdump-rs; jobs=2; push=0; dry=0
tags=(); pf_images=(); imagename="${SEARCHDUMP_IMAGE:-${ELASTICDUMP_IMAGE:-}}"  # ELASTICDUMP_IMAGE: deprecated alias (L-810a)
while [[ $# -gt 0 ]]; do
  case "$1" in
    --type) [[ $# -ge 2 ]] || die "--type needs a value"; btype="$2"; shift 2 ;;
    --type=*) btype="${1#*=}"; shift ;;
    --variant) [[ $# -ge 2 ]] || die "--variant needs a value"; btype=$(old_variant "$2"); shift 2 ;;
    --variant=*) btype=$(old_variant "${1#*=}"); shift ;;
    --platform) [[ $# -ge 2 ]] || die "--platform needs a value"; platform="$2"; shift 2 ;;
    --platform=*) platform="${1#*=}"; shift ;;
    --imagename) [[ $# -ge 2 ]] || die "--imagename needs a value"; imagename="$2"; shift 2 ;;
    --imagename=*) imagename="${1#*=}"; shift ;;
    --tag) [[ $# -ge 2 ]] || die "--tag needs a value"; tags+=("$2"); shift 2 ;;
    --tag=*) tags+=("${1#*=}"); shift ;;
    --name) [[ $# -ge 2 ]] || die "--name needs a value"; name="$2"; shift 2 ;;
    --name=*) name="${1#*=}"; shift ;;
    --jobs) [[ $# -ge 2 ]] || die "--jobs needs a value"; jobs="$2"; shift 2 ;;
    --jobs=*) jobs="${1#*=}"; shift ;;
    --preflight) [[ $# -ge 2 ]] || die "--preflight needs an image"; pf_images+=("$2"); shift 2 ;;
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
case "$btype" in
  dhi-debian) target=runtime-shell; pre="";          suffix="" ;;
  dhi-static) target=runtime;       pre=DHI_STATIC_; suffix=-static ;;
  dhi-alpine) target=runtime-shell; pre=DHI_ALPINE_; suffix=-alpine ;;
  public)     target=runtime-shell; pre=PUBLIC_;     suffix=-debian-public ;;
  alpine)     target=runtime-shell; pre=ALPINE_;     suffix=-alpine-public ;;
  *) die "--type must be dhi-debian, dhi-static, dhi-alpine, public or alpine (got '$btype')" ;;
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
if [[ -n "$imagename" ]]; then
  # repo[:tag]; a bare repo gets :<version>. Reject whitespace and anything podman would misparse.
  [[ "$imagename" =~ ^[a-z0-9][a-z0-9._/:@-]*$ && "$imagename" != *//* ]] \
    || die "--imagename/SEARCHDUMP_IMAGE '$imagename' is not a valid image reference (lowercase repo[:tag])"
  last="${imagename##*/}"; [[ "$last" == *:* || "$last" == *@* ]] || imagename="$imagename:$version"
  tags=("$imagename" ${tags[@]+"${tags[@]}"})
fi
if [[ ${#tags[@]} -eq 0 ]]; then
  if [[ "$btype" == dhi-debian ]]; then tags=("$name:$version" "$name:latest"); else tags=("$name:$version$suffix"); fi
fi

run() { printf '+'; printf ' %q' "$@"; printf '\n'; [[ $dry == 1 ]] || "$@"; }

# Preflight (fail early with a clear message instead of deep inside the build).
preflight() {
  command -v "${C%% *}" >/dev/null 2>&1 || die "container tool '$C' not found; install podman (preferred) or docker, or set CONTAINER="
  if [[ "${VENDOR_ON_HOST:-0}" == 1 ]]; then   # default: crates are vendored in a container (no host Rust)
    command -v cargo >/dev/null 2>&1 || die "cargo not found on the host: the offline build vendors crates on the host (install Rust >= 1.91 via rustup)"
    local cvo
    cvo=$(cargo --version 2>&1) || die "cargo at $(command -v cargo) does not run from $PWD: ${cvo##*$'\n'}
    If cargo is a distrobox/toolbox wrapper (podman exec into a container), that container cannot see this
    directory. Clone under \$HOME, add this path to the container's mounts, or install Rust on the host (rustup)."
    local cv; cv=$(printf '%s\n' "$cvo" | sed -n 's/^cargo 1\.\([0-9]*\).*/\1/p')
    [[ -z "$cv" || "$cv" -ge 91 ]] || die "host cargo is 1.$cv; Cargo.lock v4 / rust-version need cargo >= 1.91 (rustup update)"
  fi
  if [[ "$C" == *docker* ]]; then
    export DOCKER_BUILDKIT=1   # RUN --network=none / --mount need BuildKit
    docker buildx version >/dev/null 2>&1 || die "docker needs the buildx (BuildKit) plugin for this Dockerfile; install docker-buildx or use podman"
    [[ $multi == 0 ]] || die "--platform both needs podman"
  fi
  local img imgs=("$builder_image:$builder_tag" "$base_image:$base_tag")
  [[ ${#pf_images[@]} -eq 0 ]] || imgs=("${pf_images[@]}")
  for img in "${imgs[@]}"; do
    $C image inspect "$img" >/dev/null 2>&1 && continue
    echo "pulling $img"
    if ! $C pull ${platforms:+--platform "${platforms%%,*}"} "$img" >/dev/null 2>/tmp/build-image-pull.$$; then
      if grep -qiE 'auth|denied|unauthorized|credential' /tmp/build-image-pull.$$; then
        rm -f /tmp/build-image-pull.$$
        die "cannot pull ${img%%@*}: not logged in to ${img%%/*}. Run '$C login ${img%%/*}' (Docker account) first, or build from public bases with --type public (not the default)"
      fi
      cat /tmp/build-image-pull.$$ >&2; rm -f /tmp/build-image-pull.$$; die "cannot pull $img (network/proxy?)"
    fi
    rm -f /tmp/build-image-pull.$$
  done
}
if [[ ${#pf_images[@]} -gt 0 ]]; then preflight; echo "preflight ok: ${pf_images[*]}"; exit 0; fi
[[ $dry == 1 ]] || preflight

run env CONTAINER="$C" scripts/vendor-crates.sh

args=(build --target "$target"
  --build-arg "BUILDER_IMAGE=$builder_image" --build-arg "BUILDER_TAG=$builder_tag"
  --build-arg "BASE_IMAGE=$base_image" --build-arg "BASE_TAG=$base_tag"
  --build-arg "VERSION=$version"
  --build-arg "VCS_REF=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
  --build-arg "BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  --build-arg "CARGO_BUILD_JOBS=$jobs")
[[ -n "$platforms" ]] && args+=(--platform "$platforms")

echo "type=$btype target=$target platform=${platforms:-host} builder=$builder_image:$builder_tag base=$base_image:$base_tag"
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
