FROM rust:1.89.0-slim AS builder

WORKDIR /app

# Copy workspace configuration
COPY Cargo.toml Cargo.lock ./
COPY .sqlx ./.sqlx

# Copy all required workspace members with their source code
COPY evaluation-engine/ ./evaluation-engine/
COPY feature-toggle-shared/ ./feature-toggle-shared/
COPY feature-toggle-backend/ ./feature-toggle-backend/

# Temporarily modify workspace to exclude feature-edge-server
RUN sed -i 's/members = \["evaluation-engine", "feature-edge-server", "feature-toggle-backend", "feature-toggle-shared"\]/members = ["evaluation-engine", "feature-toggle-backend", "feature-toggle-shared"]/' Cargo.toml

# Install dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Install sqlx-cli for migrations
RUN cargo install sqlx-cli --no-default-features --features postgres

# Build the backend service in offline mode
ENV SQLX_OFFLINE=true
RUN cargo build --release --bin feature-toggle-backend

# Runtime stage
FROM debian:trixie-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y postgresql-client ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/target/release/feature-toggle-backend /usr/local/bin/feature-toggle-backend
COPY --from=builder /usr/local/cargo/bin/sqlx /usr/local/bin/sqlx

# Copy migrations and configuration
COPY feature-toggle-backend/migrations ./migrations
COPY feature-toggle-backend/config.toml ./config.toml
COPY feature-toggle-backend/log4rs.yaml ./log4rs.yaml
COPY init.sql ./init.sql

# Copy entrypoint script
COPY scripts/backend-entrypoint.sh ./entrypoint.sh

# Update config for Docker networking and set permissions
RUN sed -i 's/127.0.0.1:8080/0.0.0.0:8080/g' ./config.toml && \
    chmod +x ./entrypoint.sh

EXPOSE 8080 50051

ENTRYPOINT ["./entrypoint.sh"]
