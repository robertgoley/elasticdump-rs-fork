# syntax=docker/dockerfile:1.7
#
# elasticdump-rs container image. scripts/build-image.sh (or the Makefile) picks the variant.
#
# Default (target `runtime`, tags <ver> and latest): Docker Hardened Images, Debian 13 / glibc,
# mimalloc as the global allocator (the default for *-linux-gnu builds, see Cargo.toml)
#   builder: dhi.io/rust:1.99.1-debian13-dev
#   runtime: dhi.io/static:20250419-glibc-debian13 (glibc, libgcc_s, CA bundle, tzdata,
#            uid 65532; no shell and no package manager, so the final stage is COPY-only)
# Base comparison (size, CVEs, arch): results/local/L-808b.md, SECURITY-CVE.md.
#
# Alpine/musl variant (target `runtime-shell`, tag <ver>-alpine; musl's own allocator):
#   builder: dhi.io/rust:1.99.0-alpine3.23-dev, runtime: dhi.io/alpine-base:3.24-alpine3.24-dev
#   (shell + apk; bash, ca-certificates and libgcc are added, packages upgraded at build time)
#   Pins: DHI_ALPINE_* in the Makefile, passed through the ARGs below.
#
# Other bases through the same ARGs and the `runtime-shell` target (see Makefile):
#   make image-dhi-debian  dhi.io/rust:1.99.1-debian13-dev -> dhi.io/debian-base:trixie-debian13-dev (glibc)
#   make image-alpine      rust:1.99.1-alpine3.24 -> alpine:3.24.3 (public, musl)
#   make image-public      rust:1.99-trixie -> debian:trixie-slim (public, glibc)
# Builder and runtime always use the same libc. dhi.io needs `podman login dhi.io` /
# `docker login dhi.io` (pull only; credentials never enter the image).
ARG BUILDER_IMAGE=dhi.io/rust
ARG BUILDER_TAG=1.99.1-debian13-dev@sha256:be6c6ec91c61ae15b80521052df1c7f60fefd303293563b3dbd9f57e58783809
ARG BASE_IMAGE=dhi.io/static
ARG BASE_TAG=20250419-glibc-debian13@sha256:dcf8da7f5cdeb490d825814561995ce6c6cf931aa4322dae89f4f0170b0fb693

# ----------------------------------------------------------------- builder
FROM ${BUILDER_IMAGE}:${BUILDER_TAG} AS builder
# Release profile = fat LTO + codegen-units=1: the link step is the memory
# peak (~1.5-2 GB). Keep the job count low on small hosts.
ARG CARGO_BUILD_JOBS=2
# CARGO_HOME pinned so the registry cache mount below works for every builder
# (official rust images already use it; Alpine-packaged cargo defaults to ~/.cargo).
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} \
    CARGO_HOME=/usr/local/cargo \
    CARGO_TERM_COLOR=never
WORKDIR /src
# Alpine builders (the Alpine variants) need libc headers
# and a CA bundle; Debian/DHI builders already have both.
RUN if command -v apk >/dev/null 2>&1; then apk add --no-cache musl-dev ca-certificates; fi
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Cargo.toml declares the test targets; their sources must exist to load it.
COPY tests ./tests
# Workspace members (test-only tools; not built into the image).
COPY tools ./tools
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=edrs-registry \
    --mount=type=cache,target=/src/target,id=edrs-target \
    cargo build --locked --release --bin elasticdump-rs \
 && install -D -m 0755 target/release/elasticdump-rs /out/elasticdump-rs \
 && /out/elasticdump-rs --version
# CA bundle for runtimes that lack one (runtime-shell copies it only when missing).
RUN install -D -m 0644 /etc/ssl/certs/ca-certificates.crt /out/ca-certificates.crt
# Root tree for the COPY-only `runtime` stage (bases without a shell): the binary, the
# `elasticdump` symlink for scripts written for the Node tool, and an empty /data.
RUN install -D -m 0755 /out/elasticdump-rs /out/root/usr/local/bin/elasticdump-rs \
 && ln -s elasticdump-rs /out/root/usr/local/bin/elasticdump \
 && install -d -m 0755 /out/root/data

# ----------------------------------------------------------------- base (labels, env)
FROM ${BASE_IMAGE}:${BASE_TAG} AS base
ARG VERSION=0.1.0
ARG VCS_REF=unknown
ARG BUILD_DATE=unknown
ARG BASE_IMAGE
ARG BASE_TAG
LABEL org.opencontainers.image.title="elasticdump-rs" \
      org.opencontainers.image.description="Import and export tools for Elasticsearch and OpenSearch (elasticdump-compatible, in Rust)" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.source="https://github.com/elasticsearch-dump/elasticsearch-dump" \
      org.opencontainers.image.base.name="${BASE_IMAGE}:${BASE_TAG}"
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

# ----------------------------------------------------------------- runtime-shell
# Bases with a shell (Alpine, Debian -dev, debian:trixie-slim): `--target runtime-shell`.
FROM base AS runtime-shell
COPY --from=builder /out/elasticdump-rs /usr/local/bin/elasticdump-rs
# - `elasticdump` symlink for scripts written for the Node tool
# - CA bundle only when the base image has none
# - /data: writable working directory for dumps (mount a volume here)
RUN --mount=type=bind,from=builder,source=/out/ca-certificates.crt,target=/tmp/ca.crt \
    if command -v apk >/dev/null 2>&1; then \
      apk upgrade --no-cache && apk add --no-cache bash ca-certificates libgcc; \
    fi \
 && ln -s elasticdump-rs /usr/local/bin/elasticdump \
 && if [ ! -s /etc/ssl/certs/ca-certificates.crt ]; then \
      mkdir -p /etc/ssl/certs && cp /tmp/ca.crt /etc/ssl/certs/ca-certificates.crt; \
    fi \
 && mkdir -p /data && chown 65532:65532 /data
# 65532 = "nonroot" in Docker Hardened Images (and distroless); numeric so it
# also works on bases without that passwd entry.
USER 65532:65532
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/elasticdump-rs"]
CMD ["--help"]

# ----------------------------------------------------------------- runtime (default)
# Shell-less glibc base (dhi.io/static): no RUN, only COPY. The base already ships
# glibc, libgcc_s, the CA bundle and the 65532 nonroot user.
FROM base AS runtime
COPY --from=builder /out/root/usr/local/bin/ /usr/local/bin/
COPY --from=builder --chown=65532:65532 /out/root/data /data
USER 65532:65532
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/elasticdump-rs"]
CMD ["--help"]
