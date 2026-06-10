# syntax=docker/dockerfile:1

FROM rust:1.89.0-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
# Copy workspace configuration first
COPY Cargo.toml Cargo.lock ./

# Copy all required workspace members
COPY evaluation-engine/ ./evaluation-engine/
COPY feature-toggle-shared/ ./feature-toggle-shared/
COPY feature-edge-server/ ./feature-edge-server/
COPY feature-toggle-backend/ ./feature-toggle-backend/

# Create a temporary workspace configuration that excludes feature-toggle-backend
RUN printf '[workspace]\nresolver = "3"\nmembers = [\n    "evaluation-engine",\n    "feature-edge-server",\n    "feature-toggle-shared",\n]' > Cargo.toml

RUN cargo chef prepare --recipe-path recipe.json --bin feature-edge-server

FROM chef AS builder

# Install dependencies once (including curl for swagger-ui)
RUN apt-get update && apt-get install -y curl libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json

# Build dependencies - this layer will be cached unless Cargo.toml or Cargo.lock changes
RUN cargo chef cook --release --recipe-path recipe.json --bin feature-edge-server

# Copy source code and build
COPY Cargo.toml Cargo.lock ./
COPY evaluation-engine/ ./evaluation-engine/
COPY feature-toggle-shared/ ./feature-toggle-shared/
COPY feature-edge-server/ ./feature-edge-server/
COPY feature-toggle-backend/ ./feature-toggle-backend/

# Create workspace config
RUN printf '[workspace]\nresolver = "3"\nmembers = [\n    "evaluation-engine",\n    "feature-edge-server",\n    "feature-toggle-shared",\n]' > Cargo.toml

# Build the edge server service using docker profile for smaller size
RUN cargo build --release --bin feature-edge-server

# Runtime stage
FROM debian:trixie-slim

# Install runtime dependencies (minimal for the edge server)
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary (note: docker profile outputs to target/docker/)
COPY --from=builder /app/target/release/feature-edge-server /usr/local/bin/feature-edge-server

# Copy configuration
COPY feature-edge-server/log4rs.yaml ./log4rs.yaml

EXPOSE 8081

# Start the edge server directly
CMD ["feature-edge-server"]
