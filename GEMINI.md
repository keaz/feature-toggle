# Gemini Project Analysis: Feature Toggle System

## Project Overview

This project is a comprehensive feature toggle system, likely named "Flux Gate". It is a multi-component, service-oriented system written primarily in Rust.

The architecture consists of three main parts:
1.  **Central Backend (`feature-toggle-backend`):** A Rust application that serves as the control plane. It provides a REST API (plus WebSocket streams) for managing feature flags, users, environments, etc. It communicates with the database and exposes a gRPC interface for the edge servers.
2.  **Edge Server (`feature-edge-server`):** A lightweight Rust service designed for high-performance evaluation of feature flags. It fetches configuration from the backend via gRPC and evaluates flags based on the context provided by client applications.
3.  **Database:** A PostgreSQL database for persistent storage of all feature flag configurations, user data, and other related entities.

The project is managed as a Rust workspace with several crates:
- `feature-toggle-backend`: The main backend application.
- `feature-edge-server`: The evaluation server.
- `evaluation-engine`: A shared library crate, likely containing the core feature flag evaluation logic.
- `feature-toggle-shared`: A shared library for common data structures and constants.

## Building and Running

The project is fully containerized using Docker and managed via a `Makefile` and `docker-compose.yml`.

### Key Commands

*   **Start all services:** To build the Docker images and start the backend, edge server, and database:
    ```bash
    make up
    ```
    Alternatively, to start and follow logs immediately:
    ```bash
    make up-logs
    ```
    - The backend's REST API will be available at `http://localhost:8080/api/v1`.
    - The edge server's HTTP endpoint will be at `http://localhost:8081`.
    - The PostgreSQL database is exposed on port `5433`.

*   **Stop all services:**
    ```bash
    make down
    ```

*   **Clean the environment:** To stop services, remove containers, and delete the database volume:
    ```bash
    make clean
    ```

*   **View logs:**
    ```bash
    # View logs for all services
    make logs

    # View logs for a specific service
    make logs-backend
    make logs-edge
    ```

*   **Run tests:** The project includes an integrated test command that runs `cargo test` inside a Docker container with a running database.
    ```bash
    make test
    ```

### Building Docker Images

The `Makefile` provides extensive support for building and pushing Docker images, including multi-arch builds for `linux/amd64` and `linux/arm64`.

*   **Build images locally:**
    ```bash
    make build
    ```

*   **Build and push multi-arch images:** (Requires Docker Hub login via `make login`)
    ```bash
    make push-all TAG=v1.0.0
    ```

## Development Conventions

*   **Technology Stack:**
    - **Backend:** Rust, Actix-web (web framework), REST + WebSocket (OpenAPI via utoipa), SQLx (PostgreSQL), Tonic (gRPC).
    - **Database:** PostgreSQL.
    - **Tooling:** Docker, Docker Compose, Makefile.

*   **Database Migrations:** The project uses `sqlx-cli` for database migrations, located in `feature-toggle-backend/migrations`. These are applied automatically by the backend on startup.

*   **Testing:** Tests are intended to be run using the `make test` command, which ensures a consistent testing environment with a fresh database.

*   **Configuration:** Service configuration is managed via `.toml` files (e.g., `feature-edge-server/config.toml`) and environment variables, as defined in `docker-compose.yml`.
