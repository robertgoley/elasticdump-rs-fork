# syntax=docker/dockerfile:1.7
#
# elasticdump-rs container image.
#
# Default (make image): Docker Hardened Images, Alpine/musl
#   builder: dhi.io/rust:1.99.0-alpine3.23-dev   (cargo, gcc, musl-dev)
#   runtime: dhi.io/alpine-base:3.24-alpine3.24-dev (-dev: shell + apk; bash and
#            ca-certificates are added with apk, plus libgcc because Alpine-packaged
#            rustc links libgcc_s dynamically; packages are upgraded at build time)
# Both stages are musl (the 3.23 builder's musl is older than the runtime's, which
# is the compatible direction). dhi.io needs `podman login dhi.io` / `docker login
# dhi.io` (pull only; credentials never enter the image).
#
# Other bases through the same ARGs (see Makefile):
#   make image-dhi-debian  dhi.io/rust:1.99.1-debian13-dev -> dhi.io/debian-base:trixie-debian13-dev (glibc)
#   make image-alpine      rust:1.99.1-alpine3.24 -> alpine:3.24.3 (public, musl)
#   make image-public      rust:1.99-trixie -> debian:trixie-slim (public, glibc)
# CVE comparison of these bases: SECURITY-CVE.md.
ARG BUILDER_IMAGE=dhi.io/rust
ARG BUILDER_TAG=1.99.0-alpine3.23-dev@sha256:2d4387114840f672c75a8cec9414a300e07b285798382c76a466eafec4686b6e
ARG BASE_IMAGE=dhi.io/alpine-base
ARG BASE_TAG=3.24-alpine3.24-dev@sha256:7182e0bc2f186386bb23bc0718508913450ba1876e93cf07e5e3cf263c4dbb63

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
# Alpine builders (musl fallback, see Makefile image-alpine) need libc headers
# and a CA bundle; Debian/DHI builders already have both.
RUN if command -v apk >/dev/null 2>&1; then apk add --no-cache musl-dev ca-certificates; fi
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Cargo.toml declares the test targets; their sources must exist to load it.
COPY tests ./tests
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=edrs-registry \
    --mount=type=cache,target=/src/target,id=edrs-target \
    cargo build --locked --release --bin elasticdump-rs \
 && install -D -m 0755 target/release/elasticdump-rs /out/elasticdump-rs \
 && /out/elasticdump-rs --version
# CA bundle for runtimes that lack one (copied only when missing, see below).
RUN install -D -m 0644 /etc/ssl/certs/ca-certificates.crt /out/ca-certificates.crt

# ----------------------------------------------------------------- runtime
FROM ${BASE_IMAGE}:${BASE_TAG}
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

ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
# 65532 = "nonroot" in Docker Hardened Images (and distroless); numeric so it
# also works on bases without that passwd entry.
USER 65532:65532
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/elasticdump-rs"]
CMD ["--help"]
