#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

: "${DATABASE_URL:?DATABASE_URL must be set}"

psql "${DATABASE_URL}" -f init.sql
cargo test -p feature-toggle-backend --test grpc_ingest_idempotency_integration -- --nocapture
