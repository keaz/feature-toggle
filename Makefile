# Makefile for Feature Toggle Docker Services

.PHONY: help build build-backend build-edge up down clean logs test

# Default target
help:
	@echo "Available targets:"
	@echo "  build         - Build all Docker images"
	@echo "  build-backend - Build only the backend image"
	@echo "  build-edge    - Build only the edge server image"
	@echo "  up            - Start all services with docker-compose"
	@echo "  down          - Stop all services"
	@echo "  clean         - Remove all containers and images"
	@echo "  logs          - Follow logs from all services"
	@echo "  logs-backend  - Follow logs from backend service"
	@echo "  logs-edge     - Follow logs from edge server"
	@echo "  test          - Run tests in containers"
	@echo ""
	@echo "Docker run examples with custom config:"
	@echo "  docker run -d -p 8080:8080 -v ./my-config.toml:/app/config/config.toml:ro keaz/flux-gate-backend"
	@echo "  docker run -d -p 8080:8080 -v /path/to/config:/app/config:ro keaz/flux-gate-backend"

# Build all images
build:
	docker-compose build

# Build individual services
build-backend:
	docker build -f feature-toggle-backend/Dockerfile -t keaz/flux-gate-backend .

build-edge:
	docker build -f feature-edge-server/Dockerfile -t keaz/flux-gate-edge .

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

# Run tests (you might want to customize this)
test:
	docker-compose exec feature_toggle_backend cargo test

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
