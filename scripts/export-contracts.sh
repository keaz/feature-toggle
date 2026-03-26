#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${1:-$ROOT_DIR/feature-toggle-backend/contracts/generated}"

echo "Exporting contracts to: $OUTPUT_DIR"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}" \
  cargo run -p feature-toggle-backend --bin export-contracts -- --output "$OUTPUT_DIR"
