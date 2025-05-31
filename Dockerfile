FROM rust:latest as builder

WORKDIR /app
COPY . .

ENV DATABASE_URL=postgres://postgres:local123@postgres_server:5432/feature_toggle

RUN apt-get update && apt-get install -y libssl-dev pkg-config
RUN cargo install sqlx-cli --no-default-features --features postgres
RUN cd feature-toggle-backend && \
    sqlx database create && \
    sqlx migrate run
RUN cargo test
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/feature-toggle-backend /usr/local/bin/feature-toggle-backend

EXPOSE 8080
CMD ["feature-toggle-backend"]

