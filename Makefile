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

BUILDER_IMAGE ?= dhi.io/rust
BUILDER_TAG   ?= 1.99.0-alpine3.23-dev
BASE_IMAGE    ?= dhi.io/alpine-base
BASE_TAG      ?= 3.24-alpine3.24-dev
# Docker Hardened Images, Debian 13 / glibc (more CVEs than the Alpine default)
DHI_DEBIAN_BUILDER_IMAGE ?= dhi.io/rust
DHI_DEBIAN_BUILDER_TAG   ?= 1.99.1-debian13-dev
DHI_DEBIAN_BASE_IMAGE    ?= dhi.io/debian-base
DHI_DEBIAN_BASE_TAG      ?= trixie-debian13-dev
# Public fallback (same Debian release, so the same glibc)
PUBLIC_BUILDER_IMAGE ?= docker.io/library/rust
PUBLIC_BUILDER_TAG   ?= 1.99-trixie
PUBLIC_BASE_IMAGE    ?= docker.io/library/debian
PUBLIC_BASE_TAG      ?= trixie-slim
# Public musl fallback (Alpine builder + Alpine runtime with bash)
ALPINE_BUILDER_IMAGE ?= docker.io/library/rust
ALPINE_BUILDER_TAG   ?= 1.99.1-alpine3.24
ALPINE_BASE_IMAGE    ?= docker.io/library/alpine
ALPINE_BASE_TAG      ?= 3.24.3
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
