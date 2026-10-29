# Container image helpers. Nothing here pushes: tag and push yourself, e.g.
#   podman push $(REGISTRY)/$(IMAGE_NAME):$(IMAGE_TAG)
#
#   make image                 # dhi.io bases (needs `podman login dhi.io` / `docker login dhi.io`)
#   make image-public          # public Debian 13 bases (no login)
#   make test-image            # --version, --help, bash present, non-root
#   make sbom scan             # syft SBOM + grype/trivy scan of the image
REGISTRY   ?= localhost
IMAGE_NAME ?= elasticdump-rs
IMAGE_TAG  ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IMAGE      := $(REGISTRY)/$(IMAGE_NAME):$(IMAGE_TAG)

CONTAINER ?= $(shell command -v podman >/dev/null 2>&1 && echo podman || echo docker)

BUILDER_IMAGE ?= dhi.io/rust
BUILDER_TAG   ?= 1.99.1-debian13-dev
BASE_IMAGE    ?= dhi.io/debian-base
BASE_TAG      ?= trixie-debian13-dev
# Public fallback (same Debian release, so the same glibc)
PUBLIC_BUILDER_IMAGE ?= docker.io/library/rust
PUBLIC_BUILDER_TAG   ?= 1.99-trixie
PUBLIC_BASE_IMAGE    ?= docker.io/library/debian
PUBLIC_BASE_TAG      ?= trixie-slim
CARGO_BUILD_JOBS ?= 2

VCS_REF    := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)
BUILD_DATE := $(shell date -u +%Y-%m-%dT%H:%M:%SZ)

BUILD_ARGS = --build-arg VERSION=$(IMAGE_TAG) --build-arg VCS_REF=$(VCS_REF) \
             --build-arg BUILD_DATE=$(BUILD_DATE) --build-arg CARGO_BUILD_JOBS=$(CARGO_BUILD_JOBS)

.PHONY: help image image-public test-image sbom scan print
help:
	@sed -n '1,9p' Makefile

print:
	@echo "IMAGE=$(IMAGE) CONTAINER=$(CONTAINER)"
	@echo "builder=$(BUILDER_IMAGE):$(BUILDER_TAG) base=$(BASE_IMAGE):$(BASE_TAG)"

image:
	$(CONTAINER) build $(BUILD_ARGS) \
	  --build-arg BUILDER_IMAGE=$(BUILDER_IMAGE) --build-arg BUILDER_TAG=$(BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(BASE_IMAGE) --build-arg BASE_TAG=$(BASE_TAG) \
	  -t $(IMAGE) .

image-public:
	$(CONTAINER) build $(BUILD_ARGS) \
	  --build-arg BUILDER_IMAGE=$(PUBLIC_BUILDER_IMAGE) --build-arg BUILDER_TAG=$(PUBLIC_BUILDER_TAG) \
	  --build-arg BASE_IMAGE=$(PUBLIC_BASE_IMAGE) --build-arg BASE_TAG=$(PUBLIC_BASE_TAG) \
	  -t $(IMAGE) .

test-image:
	$(CONTAINER) run --rm $(IMAGE) --version
	$(CONTAINER) run --rm $(IMAGE) --help >/dev/null
	$(CONTAINER) run --rm --entrypoint /bin/bash $(IMAGE) -c 'echo "bash $$BASH_VERSION, uid $$(id -u)"; test "$$(id -u)" != 0; command -v elasticdump; test -s /etc/ssl/certs/ca-certificates.crt'

sbom:
	syft $(IMAGE) -o cyclonedx-json=sbom-image.cdx.json -o spdx-json=sbom-image.spdx.json

scan:
	grype $(IMAGE) --fail-on high
	trivy image --severity HIGH,CRITICAL --exit-code 1 --ignore-unfixed=false $(IMAGE)
