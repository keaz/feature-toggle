FROM rust:1.89.0-slim  AS builder

WORKDIR /app
COPY . .

ENV DATABASE_URL=postgres://postgres:local123@postgres_server:5432/feature_toggle

RUN apt-get update && apt-get install -y pkg-config libssl-dev curl && rm -rf /var/lib/apt/lists/*
#RUN apt-get update && apt-get install -y libssl-dev pkg-config
RUN cargo install sqlx-cli --no-default-features --features postgres

#RUN cargo test
RUN cargo build --release
#
FROM debian:trixie-slim
#
#RUN apt-get update && apt-get install -y postgresql-client ca-certificates && rm -rf /var/lib/apt/lists/*
#
WORKDIR /app
COPY --from=builder /app/target/release/feature-toggle-backend /usr/local/bin/feature-toggle-backend
#COPY --from=builder /usr/local/cargo/bin/sqlx /usr/local/bin/sqlx
#COPY --from=builder /app/feature-toggle-backend/migrations /app/migrations
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

#EXPOSE 8080
ENTRYPOINT ["/app/entrypoint.sh"]
