# Makefile for Feature Toggle Docker Services

# Default image names (override with `make TAG=v1.2.3` or `make IMAGE_BACKEND=user/repo`)
IMAGE_BACKEND ?= keaz/flux-gate-backend
IMAGE_EDGE    ?= keaz/flux-gate-edge
# Default TAG comes from the latest git tag on this branch; override with `make TAG=...`
TAG           ?= $(shell git describe --tags --abbrev=0 2>/dev/null || echo latest)
PLATFORMS     ?= linux/amd64,linux/arm64
# Base tag without a trailing '-alpha' to avoid double-alpha tags
TAG_BASE      = $(patsubst %-alpha,%,$(TAG))
# Helpers to iterate per-arch (split comma-separated PLATFORMS into space-separated list)
comma         := ,
space         := $(empty) $(empty)
PLATFORMS_LIST:= $(subst $(comma),$(space),$(PLATFORMS))
# Compose project name (affects default network name `${PROJECT}_default`)
PROJECT       ?= feature-toggle
NETWORK       ?= $(PROJECT)_default

.PHONY: help build build-backend build-edge up down clean logs test \
        login buildx-setup buildx-backend buildx-edge buildx-all \
        push-backend push-edge push-all \
        build-backend-latest-arch build-edge-latest-arch build-all-latest-arch \
        push-backend-latest-arch push-edge-latest-arch push-all-latest-arch \
        status restart-backend restart-edge restart-db logs-backend logs-edge logs-db up-logs

# Default target
help:
	@echo "Available targets:"
	@echo "  build                  - Build all Docker images with docker-compose"
	@echo "  build-backend          - Build only the backend image (local arch)"
	@echo "  build-edge             - Build only the edge server image (local arch)"
	@echo "  buildx-setup           - Create/Use a buildx builder for multi-arch builds"
	@echo "  buildx-backend         - Multi-arch build (no push) for backend: $(IMAGE_BACKEND):$(TAG)"
	@echo "  buildx-edge            - Multi-arch build (no push) for edge:    $(IMAGE_EDGE):$(TAG)"
	@echo "  buildx-all             - Multi-arch build (no push) for both images"
	@echo "  build-backend-alpha-arch - From git TAG, build per-arch alpha tags locally (e.g., v1.2.3-alpha-amd64, v1.2.3-alpha-arm64)"
	@echo "  build-edge-alpha-arch  - From git TAG, build per-arch alpha tags locally for edge"
	@echo "  build-all-alpha-arch   - Run both per-arch alpha builds locally"
	@echo "  push-backend           - Multi-arch build+push backend to Docker Hub"
	@echo "  push-edge              - Multi-arch build+push edge to Docker Hub"
	@echo "  push-all               - Multi-arch build+push both images"
	@echo "  push-backend-alpha-arch- From git TAG, build+push per-arch alpha tags (e.g., v1.2.3-alpha-amd64, v1.2.3-alpha-arm64)"
	@echo "  push-edge-alpha-arch   - From git TAG, build+push per-arch alpha tags for edge"
	@echo "  push-all-alpha-arch    - Run both per-arch alpha pushes"
	@echo "  build-backend-latest-arch - Build per-arch latest tags locally (e.g., amd64-latest, arm64-latest)"
	@echo "  build-edge-latest-arch - Build per-arch latest tags locally for edge"
	@echo "  build-all-latest-arch  - Run both per-arch latest builds locally"
	@echo "  push-backend-latest-arch - Build+push per-arch latest tags (e.g., amd64-latest, arm64-latest)"
	@echo "  push-edge-latest-arch  - Build+push per-arch latest tags for edge"
	@echo "  push-all-latest-arch   - Run both per-arch latest pushes"
	@echo "  login                  - Docker Hub login using DOCKER_USERNAME/DOCKER_PASSWORD or DOCKERHUB_TOKEN"
	@echo "  up                     - Start all services with docker-compose"
	@echo "  up-logs                - Start all services and follow logs"
	@echo "  down                   - Stop all services"
	@echo "  clean                  - Remove all containers, images, and orphans from compose"
	@echo "  logs                   - Follow logs from all services"
	@echo "  logs-backend           - Follow logs from backend service"
	@echo "  logs-edge              - Follow logs from edge server"
	@echo "  logs-db                - Follow logs from database"
	@echo "  test                   - Run Rust tests inside Docker with Postgres via docker-compose"
	@echo ""
	@echo "Variables: TAG=$(TAG) PLATFORMS=$(PLATFORMS) IMAGE_BACKEND=$(IMAGE_BACKEND) IMAGE_EDGE=$(IMAGE_EDGE)"
	@echo "Examples:"
	@echo "  make push-all TAG=v1.2.3"
	@echo "  make push-backend TAG=$$(git describe --tags --abbrev=0) IMAGE_BACKEND=youruser/yourrepo"
	@echo "  make build-backend-alpha-arch IMAGE_BACKEND=youruser/yourrepo"
	@echo "  make push-backend-alpha-arch IMAGE_BACKEND=youruser/yourrepo"
	@echo ""
	@echo "Docker run examples with custom config:"
	@echo "  docker run -d -p 8080:8080 -v ./my-config.toml:/app/config/config.toml:ro $(IMAGE_BACKEND):$(TAG)"
	@echo "  docker run -d -p 8080:8080 -v /path/to/config:/app/config:ro $(IMAGE_BACKEND):$(TAG)"

# Build all images (local arch)
build:
	docker-compose build

# Build individual services (local arch)
build-backend:
	docker build -f feature-toggle-backend/Dockerfile -t $(IMAGE_BACKEND):$(TAG) .

build-edge:
	docker build -f feature-edge-server/Dockerfile -t $(IMAGE_EDGE):$(TAG) .

# Multi-arch builder setup
buildx-setup:
	@docker buildx inspect multiarch-builder >/dev/null 2>&1 || docker buildx create --name multiarch-builder --use
	@docker buildx use multiarch-builder
	@docker buildx inspect --bootstrap

# Multi-arch build (no push)
buildx-backend: buildx-setup
	docker buildx build \
		--platform $(PLATFORMS) \
		-f feature-toggle-backend/Dockerfile \
		-t $(IMAGE_BACKEND):$(TAG) \
		--provenance=false \
		--sbom=false \
		--load \
		.

buildx-edge: buildx-setup
	docker buildx build \
		--platform $(PLATFORMS) \
		-f feature-edge-server/Dockerfile \
		-t $(IMAGE_EDGE):$(TAG) \
		--provenance=false \
		--sbom=false \
		--load \
		.

buildx-all: buildx-backend buildx-edge

# Docker Hub login
login:
	@if [ -n "$$DOCKERHUB_TOKEN" ]; then \
		echo "Logging in with DOCKERHUB_TOKEN for $$DOCKER_USERNAME"; \
		echo "$$DOCKERHUB_TOKEN" | docker login -u "$$DOCKER_USERNAME" --password-stdin; \
	elif [ -n "$$DOCKER_PASSWORD" ] && [ -n "$$DOCKER_USERNAME" ]; then \
		echo "$$DOCKER_PASSWORD" | docker login -u "$$DOCKER_USERNAME" --password-stdin; \
	else \
		echo "Set DOCKER_USERNAME and DOCKER_PASSWORD (or DOCKERHUB_TOKEN) to login"; exit 1; \
	fi

# Multi-arch build and push
push-backend: buildx-setup
	docker buildx build \
		--platform $(PLATFORMS) \
		-f feature-toggle-backend/Dockerfile \
		-t $(IMAGE_BACKEND):$(TAG) \
		--provenance=false \
		--sbom=false \
		--push \
		.

push-edge: buildx-setup
	docker buildx build \
		--platform $(PLATFORMS) \
		-f feature-edge-server/Dockerfile \
		-t $(IMAGE_EDGE):$(TAG) \
		--provenance=false \
		--sbom=false \
		--push \
		.

push-all: push-backend push-edge

# Per-arch alpha tags build locally using git tag as base
build-backend-alpha-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Building locally $(IMAGE_BACKEND):$(TAG_BASE)-alpha-$$arch and $$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-toggle-backend/Dockerfile \
	    -t $(IMAGE_BACKEND):$(TAG_BASE)-alpha-$$arch \
	    -t $(IMAGE_BACKEND):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --load \
	    . ; \
	done

build-edge-alpha-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Building locally $(IMAGE_EDGE):$(TAG_BASE)-alpha-$$arch and $$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-edge-server/Dockerfile \
	    -t $(IMAGE_EDGE):$(TAG_BASE)-alpha-$$arch \
	    -t $(IMAGE_EDGE):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --load \
	    . ; \
	done

build-all-alpha-arch: build-backend-alpha-arch build-edge-alpha-arch

# Per-arch alpha tags push using git tag as base
push-backend-alpha-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Pushing $(IMAGE_BACKEND):$(TAG_BASE)-alpha-$$arch and $$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-toggle-backend/Dockerfile \
	    -t $(IMAGE_BACKEND):$(TAG_BASE)-alpha-$$arch \
	    -t $(IMAGE_BACKEND):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --push \
	    . ; \
	done

push-edge-alpha-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Pushing $(IMAGE_EDGE):$(TAG_BASE)-alpha-$$arch and $$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-edge-server/Dockerfile \
	    -t $(IMAGE_EDGE):$(TAG_BASE)-alpha-$$arch \
	    -t $(IMAGE_EDGE):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --push \
	    . ; \
	done

push-all-alpha-arch: push-backend-alpha-arch push-edge-alpha-arch

# Per-arch latest tags build locally
build-backend-latest-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Building locally $(IMAGE_BACKEND):$$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-toggle-backend/Dockerfile \
	    -t $(IMAGE_BACKEND):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --load \
	    . ; \
	done

build-edge-latest-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Building locally $(IMAGE_EDGE):$$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-edge-server/Dockerfile \
	    -t $(IMAGE_EDGE):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --load \
	    . ; \
	done

build-all-latest-arch: build-backend-latest-arch build-edge-latest-arch

# Per-arch latest tags push
push-backend-latest-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Pushing $(IMAGE_BACKEND):$$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-toggle-backend/Dockerfile \
	    -t $(IMAGE_BACKEND):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --push \
	    . ; \
	done

push-edge-latest-arch: buildx-setup
	@set -e; \
	for plat in $(PLATFORMS_LIST); do \
	  arch=$${plat#linux/}; \
	  echo "Pushing $(IMAGE_EDGE):$$arch-latest for $$plat"; \
	  docker buildx build \
	    --platform $$plat \
	    -f feature-edge-server/Dockerfile \
	    -t $(IMAGE_EDGE):$$arch-latest \
	    --provenance=false \
	    --sbom=false \
	    --push \
	    . ; \
	done

push-all-latest-arch: push-backend-latest-arch push-edge-latest-arch

# Start services
up:
	docker-compose up -d

# Start services with logs
up-logs:
	docker-compose up

# Stop services
down:
	docker-compose down

# Clean up everything
clean:
	docker-compose down -v --rmi all --remove-orphans

# View logs
logs:
	docker-compose logs -f

logs-backend:
	docker-compose logs -f feature_toggle_backend

logs-edge:
	docker-compose logs -f feature_edge_server

logs-db:
	docker-compose logs -f postgres_server

# Run tests fully inside Docker (Rust toolchain in container + Postgres via compose)
# Requires docker-compose (v1 or v2). Ensures DB is up, then runs cargo test inside rust:1.89.0-slim.
test:
	# Start database
	docker-compose up -d postgres_server
	# Ensure the Rust image is present
	docker pull rust:1.89.0-slim
	# Run tests inside container on the compose network
	docker run --rm \
		--network $(NETWORK) \
		-e DATABASE_URL=postgres://postgres:local123@postgres_server:5432/feature_toggle \
		-v $$PWD:/work -w /work \
		rust:1.89.0-slim bash -lc "apt-get update && apt-get install -y libssl-dev pkg-config && cargo test --all --locked --verbose"

# Health checks
status:
	docker-compose ps

# Restart a specific service
restart-backend:
	docker-compose restart feature_toggle_backend

restart-edge:
	docker-compose restart feature_edge_server

restart-db:
	docker-compose restart postgres_server
