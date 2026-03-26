#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "Running contract compatibility test..."
SQLX_OFFLINE="${SQLX_OFFLINE:-true}" \
  cargo test -p feature-toggle-backend --test contract_compatibility_test -- contract_hashes_match_baseline --exact
