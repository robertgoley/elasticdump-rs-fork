# Container image helpers. Nothing here pushes: tag and push yourself, e.g.
#   podman push $(REGISTRY)/$(IMAGE_NAME):$(IMAGE_TAG)
#
#   make image                 # dhi.io Alpine bases, the default (needs `podman login dhi.io`)
#   make image-dhi-debian      # dhi.io Debian 13 bases (needs the login)
#   make image-public          # public Debian 13 bases (no login)
#   make image-alpine          # public Alpine/musl bases (no login, fewest CVEs)
#   make test-image            # --version, --help, bash present, non-root
#   make sbom scan             # syft SBOM + grype/trivy scan of the image
REGISTRY   ?= localhost
IMAGE_NAME ?= elasticdump-rs
IMAGE_TAG  ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IMAGE      := $(REGISTRY)/$(IMAGE_NAME):$(IMAGE_TAG)

CONTAINER ?= $(shell command -v podman >/dev/null 2>&1 && echo podman || echo docker)

# F-14: every base is pinned by digest (TAG = tag@sha256:...). Refresh a
# digest with `podman pull <image>:<tag>` + `podman images --digests`.
BUILDER_IMAGE ?= dhi.io/rust
BUILDER_TAG   ?= 1.99.0-alpine3.23-dev@sha256:2d4387114840f672c75a8cec9414a300e07b285798382c76a466eafec4686b6e
BASE_IMAGE    ?= dhi.io/alpine-base
BASE_TAG      ?= 3.24-alpine3.24-dev@sha256:7182e0bc2f186386bb23bc0718508913450ba1876e93cf07e5e3cf263c4dbb63
# Docker Hardened Images, Debian 13 / glibc (more CVEs than the Alpine default)
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

.PHONY: help image image-dhi-debian image-public image-alpine test-image save sbom scan print
help:
	@sed -n '1,11p' Makefile

print:
	@echo "IMAGE=$(IMAGE) CONTAINER=$(CONTAINER)"
	@echo "builder=$(BUILDER_IMAGE):$(BUILDER_TAG) base=$(BASE_IMAGE):$(BASE_TAG)"

image:
	$(CONTAINER) build $(BUILD_ARGS) \
	  --build-arg BUILDER_IMAGE=$(BUILDER_IMAGE) --build-arg BUILDER_TAG=$(BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(BASE_IMAGE) --build-arg BASE_TAG=$(BASE_TAG) \
	  -t $(IMAGE) .

image-dhi-debian:
	$(CONTAINER) build $(BUILD_ARGS) \
	  --build-arg BUILDER_IMAGE=$(DHI_DEBIAN_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(DHI_DEBIAN_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(DHI_DEBIAN_BASE_IMAGE) --build-arg BASE_TAG=$(DHI_DEBIAN_BASE_TAG) \
	  -t $(IMAGE) .

image-public:
	$(CONTAINER) build $(BUILD_ARGS) \
	  --build-arg BUILDER_IMAGE=$(PUBLIC_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(PUBLIC_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(PUBLIC_BASE_IMAGE) --build-arg BASE_TAG=$(PUBLIC_BASE_TAG) \
	  -t $(IMAGE) .

image-alpine:
	$(CONTAINER) build $(BUILD_ARGS) \
	  --build-arg BUILDER_IMAGE=$(ALPINE_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(ALPINE_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(ALPINE_BASE_IMAGE) --build-arg BASE_TAG=$(ALPINE_BASE_TAG) \
	  -t $(IMAGE) .

test-image:
	$(CONTAINER) run --rm $(IMAGE) --version
	$(CONTAINER) run --rm $(IMAGE) --help >/dev/null
	$(CONTAINER) run --rm --entrypoint /bin/bash $(IMAGE) -c 'echo "bash $$BASH_VERSION, uid $$(id -u)"; test "$$(id -u)" != 0; command -v elasticdump; test -s /etc/ssl/certs/ca-certificates.crt'

save:
	$(CONTAINER) save -o image.tar $(IMAGE)

sbom: save
	syft docker-archive:image.tar -o cyclonedx-json=sbom-image.cdx.json -o spdx-json=sbom-image.spdx.json

scan: save
	grype docker-archive:image.tar --fail-on high
	trivy image --input image.tar --severity HIGH,CRITICAL --exit-code 1
