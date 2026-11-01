# Container image helpers. Nothing here pushes: tag and push yourself, e.g.
#   podman push $(REGISTRY)/$(IMAGE_NAME):$(IMAGE_TAG)
# scripts/build-image.sh wraps the two main variants (--variant glibc|alpine, --platform, --tag).
#
#   make image                 # default: dhi.io Debian 13 builder -> dhi.io/static glibc, mimalloc (:<ver>, :latest)
#   make image-dhi-alpine      # dhi.io Alpine/musl, musl allocator (:<ver>-alpine)
#   make image-dhi-debian      # dhi.io Debian 13 -dev runtime with a shell (:<ver>-debian-dev)
#   make image-public          # public Debian 13 bases, no login (:<ver>-debian-public)
#   make image-alpine          # public Alpine/musl bases, no login (:<ver>-alpine-public)
#   make test-image            # SUP-017 for IMAGE_VARIANT: non-root, --read-only + /data, binary set, base digest
#   make sbom scan             # syft SBOM + grype/trivy scan of the IMAGE_VARIANT image
REGISTRY   ?= localhost
IMAGE_NAME ?= elasticdump-rs
IMAGE_TAG  ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IMAGE      := $(REGISTRY)/$(IMAGE_NAME):$(IMAGE_TAG)

# Variant used by test-image/save/sbom/scan; selects docker/image-binaries.<variant>.txt and
# the tag suffix (dhi-static | dhi-alpine | dhi-debian | public | alpine)
IMAGE_VARIANT ?= dhi-static
SUFFIX_dhi-static :=
SUFFIX_dhi-alpine := -alpine
SUFFIX_dhi-debian := -debian-dev
SUFFIX_public     := -debian-public
SUFFIX_alpine     := -alpine-public
VARIANT_IMAGE := $(IMAGE)$(SUFFIX_$(IMAGE_VARIANT))

CONTAINER ?= $(shell command -v podman >/dev/null 2>&1 && echo podman || echo docker)

# F-14: every base is pinned by digest (TAG = tag@sha256:...; multi-arch index digests).
# Refresh a digest with `podman pull <image>:<tag>` + `podman image inspect --format '{{.RepoDigests}}'`.
# Default: Docker Hardened Images, Debian 13 / glibc builder -> static glibc runtime (no shell)
BUILDER_IMAGE ?= dhi.io/rust
BUILDER_TAG   ?= 1.99.1-debian13-dev@sha256:be6c6ec91c61ae15b80521052df1c7f60fefd303293563b3dbd9f57e58783809
BASE_IMAGE    ?= dhi.io/static
BASE_TAG      ?= 20250419-glibc-debian13@sha256:dcf8da7f5cdeb490d825814561995ce6c6cf931aa4322dae89f4f0170b0fb693
# Docker Hardened Images, Alpine/musl (the default before L-808b; 0 CVEs at L-808b)
DHI_ALPINE_BUILDER_IMAGE ?= dhi.io/rust
DHI_ALPINE_BUILDER_TAG   ?= 1.99.0-alpine3.23-dev@sha256:2d4387114840f672c75a8cec9414a300e07b285798382c76a466eafec4686b6e
DHI_ALPINE_BASE_IMAGE    ?= dhi.io/alpine-base
DHI_ALPINE_BASE_TAG      ?= 3.24-alpine3.24-dev@sha256:7182e0bc2f186386bb23bc0718508913450ba1876e93cf07e5e3cf263c4dbb63
# Docker Hardened Images, Debian 13 -dev runtime (shell, apt; more CVEs)
DHI_DEBIAN_BUILDER_IMAGE ?= dhi.io/rust
DHI_DEBIAN_BUILDER_TAG   ?= 1.99.1-debian13-dev@sha256:be6c6ec91c61ae15b80521052df1c7f60fefd303293563b3dbd9f57e58783809
DHI_DEBIAN_BASE_IMAGE    ?= dhi.io/debian-base
DHI_DEBIAN_BASE_TAG      ?= trixie-debian13-dev@sha256:cc4e05ff415ee2d83918f2af45c737983209924ef7362c53f00cfcdaa1c94326
# Public fallback (same Debian release, so the same glibc)
PUBLIC_BUILDER_IMAGE ?= docker.io/library/rust
PUBLIC_BUILDER_TAG   ?= 1.99-trixie@sha256:09027f873d45b8ecd1dc86f1fa13b7a62b875c961f017a0e331267b350430137
PUBLIC_BASE_IMAGE    ?= docker.io/library/debian
PUBLIC_BASE_TAG      ?= trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4
# Public musl fallback (Alpine builder + Alpine runtime with bash)
ALPINE_BUILDER_IMAGE ?= docker.io/library/rust
ALPINE_BUILDER_TAG   ?= 1.99.1-alpine3.24@sha256:b1d5dd5a22a09404d3e798c0cf051b689077360374eb505fa5bb93c479693d87
ALPINE_BASE_IMAGE    ?= docker.io/library/alpine
ALPINE_BASE_TAG      ?= 3.24.3@sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a
CARGO_BUILD_JOBS ?= 2

VCS_REF    := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)
BUILD_DATE := $(shell date -u +%Y-%m-%dT%H:%M:%SZ)

BUILD_ARGS = --build-arg VERSION=$(IMAGE_TAG) --build-arg VCS_REF=$(VCS_REF) \
             --build-arg BUILD_DATE=$(BUILD_DATE) --build-arg CARGO_BUILD_JOBS=$(CARGO_BUILD_JOBS)

.PHONY: help image image-dhi-alpine image-dhi-debian image-public image-alpine test-image save sbom scan print
help:
	@sed -n '1,12p' Makefile

print:
	@echo "IMAGE=$(IMAGE) VARIANT_IMAGE=$(VARIANT_IMAGE) CONTAINER=$(CONTAINER)"
	@echo "builder=$(BUILDER_IMAGE):$(BUILDER_TAG) base=$(BASE_IMAGE):$(BASE_TAG)"

# shell-less static base: the default `runtime` stage (COPY only)
image:
	$(CONTAINER) build $(BUILD_ARGS) --target runtime \
	  --build-arg BUILDER_IMAGE=$(BUILDER_IMAGE) --build-arg BUILDER_TAG=$(BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(BASE_IMAGE) --build-arg BASE_TAG=$(BASE_TAG) \
	  -t $(IMAGE) -t $(REGISTRY)/$(IMAGE_NAME):latest .

image-dhi-alpine:
	$(CONTAINER) build $(BUILD_ARGS) --target runtime-shell \
	  --build-arg BUILDER_IMAGE=$(DHI_ALPINE_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(DHI_ALPINE_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(DHI_ALPINE_BASE_IMAGE) --build-arg BASE_TAG=$(DHI_ALPINE_BASE_TAG) \
	  -t $(IMAGE)$(SUFFIX_dhi-alpine) .

image-dhi-debian:
	$(CONTAINER) build $(BUILD_ARGS) --target runtime-shell \
	  --build-arg BUILDER_IMAGE=$(DHI_DEBIAN_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(DHI_DEBIAN_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(DHI_DEBIAN_BASE_IMAGE) --build-arg BASE_TAG=$(DHI_DEBIAN_BASE_TAG) \
	  -t $(IMAGE)$(SUFFIX_dhi-debian) .

image-public:
	$(CONTAINER) build $(BUILD_ARGS) --target runtime-shell \
	  --build-arg BUILDER_IMAGE=$(PUBLIC_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(PUBLIC_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(PUBLIC_BASE_IMAGE) --build-arg BASE_TAG=$(PUBLIC_BASE_TAG) \
	  -t $(IMAGE)$(SUFFIX_public) .

image-alpine:
	$(CONTAINER) build $(BUILD_ARGS) --target runtime-shell \
	  --build-arg BUILDER_IMAGE=$(ALPINE_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(ALPINE_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(ALPINE_BASE_IMAGE) --build-arg BASE_TAG=$(ALPINE_BASE_TAG) \
	  -t $(IMAGE)$(SUFFIX_alpine) .

test-image:
	CONTAINER=$(CONTAINER) scripts/test-image.sh $(VARIANT_IMAGE) $(IMAGE_VARIANT)

save:
	$(CONTAINER) save -o image.tar $(VARIANT_IMAGE)

sbom: save
	syft docker-archive:image.tar -o cyclonedx-json=sbom-image.cdx.json -o spdx-json=sbom-image.spdx.json

scan: save
	grype -c docker/grype-ignore.yaml docker-archive:image.tar --fail-on high
	trivy image --input image.tar --severity HIGH,CRITICAL --exit-code 1
