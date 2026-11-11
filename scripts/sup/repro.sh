#!/usr/bin/env bash
# TST-SUP-010 / L-704: reproducible-build check. Builds the release binary twice from the same
# commit and requires an identical SHA-256. The two builds differ in everything that must not
# leak into the binary:
#   - source, vendor, CARGO_HOME and target directories (different paths and depths),
#   - user (container root vs uid 4242), umask, TZ and HOSTNAME,
#   - wall-clock time (the builds run one after the other, the second after a pause).
# Both builds use the same toolchain image *by digest*, run with `--network none` from a
# vendored source tree (as offline-build.sh does), and set
#   SOURCE_DATE_EPOCH=<commit time>, CARGO_INCREMENTAL=0, and
#   RUSTFLAGS=--remap-path-prefix for the source, vendor and CARGO_HOME directories.
# Usage: scripts/sup/repro.sh [image@sha256:...]
#   default image: BUILDER_IMAGE:BUILDER_TAG from the Makefile (pinned by digest).
#   REPRO_PULL=1 (pull the image if missing), REPRO_PAUSE (seconds between builds, default 5),
#   CARGO_BUILD_JOBS (default 2),
#   REPRO_OUT (keep both binaries and sums there).
# Exit: 0 identical; 1 differ (the first differing offsets and differing path strings are
# printed); 2 setup error. Memory: one LTO link at a time, about 2 GB.
# Out of scope here (B / follow-ups): a second machine (B11), image builds with
# `--timestamp`, and `cargo auditable` (tool not installed; see docs/SUPPLY-CHAIN.md).
set -euo pipefail
cd "$(dirname "$0")/../.."
mk_var() { sed -n "s/^$1 *?= *//p" Makefile | head -1; }
IMG="${1:-${REPRO_IMAGE:-$(mk_var BUILDER_IMAGE):$(mk_var BUILDER_TAG)}}"
case "$IMG" in *@sha256:*) ;; *) echo "repro: image must be pinned by digest: $IMG" >&2; exit 2;; esac
if ! podman image exists "$IMG"; then
  [[ "${REPRO_PULL:-0}" == 1 ]] || { echo "repro: image not present locally: $IMG (pull it, or set REPRO_PULL=1)" >&2; exit 2; }
  podman pull -q "$IMG" >/dev/null || { echo "repro: pull failed: $IMG" >&2; exit 2; }
fi
git diff --quiet HEAD -- || echo "repro: note: the working tree has uncommitted changes; building HEAD's tracked files as checked out" >&2
SDE="$(git log -1 --format=%ct HEAD)"
W="$(mktemp -d "${TMPDIR:-/var/tmp}/edrs-repro.XXXXXX")"
cleanup() { podman unshare rm -rf "$W" 2>/dev/null || rm -rf "$W"; }
trap cleanup EXIT
cargo vendor --locked --versioned-dirs "$W/vendor" >"$W/vendor-config.toml" 2>"$W/vendor.log" \
  || { tail -20 "$W/vendor.log"; exit 2; }
git ls-files -z | tar --null -T - -cf "$W/src.tar"

# build <name> <user> <umask> <tz> <src> <vendor> <cargo-home> <target>
# rust-toolchain.toml pins 1.91.1 for host builds; with no network, rustup (official rust images) cannot
# fetch it, so use the image's default toolchain. Images without rustup (dhi.io/rust) are unaffected. L-810f.
# shellcheck disable=SC2016  # expanded inside the container, not here
RTC='if command -v rustup >/dev/null 2>&1; then RUSTUP_TOOLCHAIN=$(rustup toolchain list | sed -n "s/ (.*default.*//p" | head -1); export RUSTUP_TOOLCHAIN; fi;'
build() {
  local name=$1 user=$2 um=$3 tz=$4 src=$5 ven=$6 ch=$7 tgt=$8 d="$W/$1"
  mkdir -p "$d/src" "$d/vendor" "$d/cargo-home" "$d/target"
  tar -xf "$W/src.tar" -C "$d/src"
  cp -a "$W/vendor/." "$d/vendor/"
  sed "s#directory = \".*\"#directory = \"$ven\"#" "$W/vendor-config.toml" >"$d/cargo-home/config.toml"
  chmod -R a+rwX "$d"
  echo "repro: build $name (user $user, $src, started $(date -u +%H:%M:%S) UTC)" >&2
  podman run --rm --network none --name "edl-repro-$name-$$" --user "$user" --hostname "repro-$name" \
    -e HOME=/tmp -e TZ="$tz" -e CARGO_HOME="$ch" -e CARGO_TARGET_DIR="$tgt" -e CARGO_NET_OFFLINE=true \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" -e CARGO_INCREMENTAL=0 -e SOURCE_DATE_EPOCH="$SDE" \
    -e RUSTFLAGS="--remap-path-prefix=$src=/build/src --remap-path-prefix=$ven=/build/vendor --remap-path-prefix=$ch=/build/cargo-home" \
    -v "$d/src:$src:ro,Z" -v "$d/vendor:$ven:ro,Z" -v "$d/cargo-home:$ch:Z" -v "$d/target:$tgt:Z" \
    -w "$src" "$IMG" sh -c "umask $um && $RTC cargo build --offline --locked --release --bin searchdump" \
    >"$W/$name.log" 2>&1 || { tail -30 "$W/$name.log"; exit 2; }
  # build b's files belong to a sub-uid with umask 077: read them inside the user namespace
  podman unshare cat "$d/target/release/searchdump" >"$W/bin-$name"
}
build a 0:0 022 UTC /src /vendor /cargo-home /target
sleep "${REPRO_PAUSE:-5}"
build b 4242:4242 077 Asia/Kolkata /home/builder/work/deeper/searchdump /opt/v2/vendor /var/tmp/ch2 /home/builder/tgt

sa="$(sha256sum <"$W/bin-a" | cut -d' ' -f1)"; sb="$(sha256sum <"$W/bin-b" | cut -d' ' -f1)"
if [[ -n "${REPRO_OUT:-}" ]]; then
  mkdir -p "$REPRO_OUT" && cp "$W/bin-a" "$W/bin-b" "$W/a.log" "$W/b.log" "$REPRO_OUT/"
  printf '%s  bin-a\n%s  bin-b\n' "$sa" "$sb" >"$REPRO_OUT/SHA256SUMS"
fi
echo "repro: image $IMG" >&2
echo "repro: SOURCE_DATE_EPOCH=$SDE commit $(git rev-parse --short HEAD)" >&2
echo "repro: a $sa ($(stat -c %s "$W/bin-a") bytes)" >&2
echo "repro: b $sb ($(stat -c %s "$W/bin-b") bytes)" >&2
if [[ "$sa" == "$sb" ]]; then echo "repro: OK (bit-for-bit identical)" >&2; exit 0; fi
echo "repro: DIFFERENT; first differing bytes:" >&2
cmp -l "$W/bin-a" "$W/bin-b" 2>/dev/null | head -10 >&2 || true
echo "repro: differing build-path strings:" >&2
diff <(strings -n 8 "$W/bin-a" | grep -E '^/|/src/|cargo-home|vendor' | sort -u) \
     <(strings -n 8 "$W/bin-b" | grep -E '^/|/src/|cargo-home|vendor' | sort -u) | head -20 >&2 || true
exit 1
