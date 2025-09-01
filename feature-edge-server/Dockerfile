FROM rust:1.89.0-slim AS builder

WORKDIR /app

# Copy workspace configuration
COPY Cargo.toml Cargo.lock ./

# Copy all required workspace members with their source code
COPY evaluation-engine/ ./evaluation-engine/
COPY feature-toggle-shared/ ./feature-toggle-shared/
COPY feature-edge-server/ ./feature-edge-server/
COPY feature-toggle-backend/proto/ ./feature-toggle-backend/proto/

# Temporarily modify workspace to exclude feature-toggle-backend
RUN sed -i 's/members = \["evaluation-engine", "feature-edge-server", "feature-toggle-backend", "feature-toggle-shared"\]/members = ["evaluation-engine", "feature-edge-server", "feature-toggle-shared"]/' Cargo.toml

# Install dependencies (including curl for swagger-ui)
RUN apt-get update && apt-get install -y curl libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*

# Build the edge server service
RUN cargo build --release --bin feature-edge-server

# Runtime stage
FROM debian:trixie-slim

# Install runtime dependencies (minimal for the edge server)
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/target/release/feature-edge-server /usr/local/bin/feature-edge-server

# Copy configuration
COPY feature-edge-server/log4rs.yaml ./log4rs.yaml

EXPOSE 8081

# Start the edge server directly
CMD ["feature-edge-server"]
