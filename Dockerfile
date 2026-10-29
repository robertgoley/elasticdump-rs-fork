# syntax=docker/dockerfile:1.7
#
# elasticdump-rs container image.
#
#   builder: Docker Hardened Images Rust (-dev: shell, compiler, cargo), Debian 13
#   runtime: Docker Hardened Images Debian 13 base, -dev variant (bash, coreutils)
#
# Both stages are Debian 13 (glibc 2.41), so the dynamically linked binary
# matches the runtime libc. dhi.io needs `docker login dhi.io` (a free Docker
# account). Without it, build with public bases of the same Debian release:
#
#   podman build --build-arg BUILDER_IMAGE=docker.io/library/rust --build-arg BUILDER_TAG=1.99-trixie \
#                --build-arg BASE_IMAGE=docker.io/library/debian --build-arg BASE_TAG=trixie-slim .
#
# See `make help` and README "Container image".
ARG BUILDER_IMAGE=dhi.io/rust
ARG BUILDER_TAG=1.99.1-debian13-dev
ARG BASE_IMAGE=dhi.io/debian-base
ARG BASE_TAG=trixie-debian13-dev

# ----------------------------------------------------------------- builder
FROM ${BUILDER_IMAGE}:${BUILDER_TAG} AS builder
# Release profile = fat LTO + codegen-units=1: the link step is the memory
# peak (~1.5-2 GB). Keep the job count low on small hosts.
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} \
    CARGO_TERM_COLOR=never
WORKDIR /src
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
    ln -s elasticdump-rs /usr/local/bin/elasticdump \
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
