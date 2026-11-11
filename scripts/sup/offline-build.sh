#!/usr/bin/env bash
# TST-SUP-009 / L-206: release build with NO network, so build scripts and proc macros cannot
# reach the network. The steps are:
#   1. `cargo vendor --locked` on the host (the only step that downloads);
#   2. `podman run --network none` with the sources and vendor dir mounted read-only, running
#      `cargo build --offline --locked --release --bin searchdump`;
#   3. a probe inside the same sandbox shows that DNS and TCP really fail.
#   scripts/sup/offline-build.sh [builder-image]   (default: the public rust:1.99-trixie builder, see Makefile)
# The image's default toolchain is used (RUSTUP_TOOLCHAIN), not the rust-toolchain.toml pin: rustup cannot
# download without network, and any toolchain >= the rust-version (1.91) builds the crate.
# CI: the `offline-build` job in security.yml. Memory: one LTO link, about 2 GB (CARGO_BUILD_JOBS=2).
set -euo pipefail
cd "$(dirname "$0")/../.."
IMG="${1:-${OFFLINE_BUILDER:-docker.io/library/rust:1.99-trixie}}"
W="$(mktemp -d "${TMPDIR:-/var/tmp}/edrs-offline.XXXXXX")"
cleanup() { podman unshare rm -rf "$W" 2>/dev/null || rm -rf "$W"; }
trap cleanup EXIT
cargo vendor --locked --versioned-dirs "$W/vendor" >"$W/vendor-config.toml" 2>"$W/vendor.log" \
  || { tail -20 "$W/vendor.log"; exit 1; }
mkdir -p "$W/cargo-home" "$W/target"
cp "$W/vendor-config.toml" "$W/cargo-home/config.toml"
sed -i 's#directory = ".*"#directory = "/vendor"#' "$W/cargo-home/config.toml"
git ls-files -z | tar --null -T - -cf "$W/src.tar"
mkdir "$W/src" && tar -xf "$W/src.tar" -C "$W/src" && rm "$W/src.tar"
echo "offline-build: $IMG, network none" >&2
podman run --rm --network none --name "edl-offline-$$" \
  -e CARGO_HOME=/cargo-home -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" -e CARGO_NET_OFFLINE=true \
  -v "$W/src:/src:ro,Z" -v "$W/vendor:/vendor:ro,Z" -v "$W/cargo-home:/cargo-home:Z" -v "$W/target:/target:Z" \
  -w /src "$IMG" bash -c '
    set -e
    # sandbox probe: no DNS, no TCP
    if getent hosts crates.io >/dev/null 2>&1; then echo "PROBE FAIL: DNS works"; exit 3; fi
    if (exec 3<>/dev/tcp/1.1.1.1/443) 2>/dev/null; then echo "PROBE FAIL: TCP works"; exit 3; fi
    echo "probe: network unreachable (ok)"
    # rust-toolchain.toml pins 1.91.1 for host builds; inside the sandbox use the default toolchain
    # of the image (rustup would try to download the pinned one and fail with no network). L-810f.
    if command -v rustup >/dev/null 2>&1; then
      RUSTUP_TOOLCHAIN=$(rustup toolchain list | sed -n "s/ (.*default.*//p" | head -1); export RUSTUP_TOOLCHAIN
      echo "toolchain: $RUSTUP_TOOLCHAIN (image default; rust-toolchain.toml ignored offline)"
    fi
    cargo build --offline --locked --release --bin searchdump --target-dir /target
    /target/release/searchdump --version'
echo "offline-build: OK" >&2
