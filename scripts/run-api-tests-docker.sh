#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${COMPOSE_FILE:-$ROOT_DIR/docker-compose.api-tests.yml}"
COMPOSE_PROJECT="${API_TEST_COMPOSE_PROJECT:-feature-toggle-api-tests}"
BACKEND_PORT="${API_TEST_BACKEND_PORT:-18080}"
API_BASE_URL_VALUE="${API_BASE_URL:-http://127.0.0.1:${BACKEND_PORT}/api/v1}"
WAIT_SECONDS="${API_TEST_WAIT_SECONDS:-180}"
KEEP_STACK="${API_TEST_KEEP_STACK:-0}"
CURL_BIN="${CURL_BIN:-$(command -v curl || true)}"

if [[ -z "$CURL_BIN" && -x /usr/bin/curl ]]; then
  CURL_BIN="/usr/bin/curl"
fi

if [[ -z "$CURL_BIN" ]]; then
  echo "curl is required for health checks but was not found in PATH."
  exit 1
fi

compose() {
  docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" "$@"
}

cleanup() {
  if [[ "$KEEP_STACK" == "1" ]]; then
    echo "Keeping Docker stack running (API_TEST_KEEP_STACK=1)."
    return
  fi

  echo "Stopping API test Docker stack..."
  compose down -v --remove-orphans >/dev/null 2>&1 || true
}

trap cleanup EXIT

echo "Starting Postgres for API tests..."
compose up -d postgres

echo "Building backend images..."
compose build backend_seed feature_toggle_backend

echo "Applying migrations and seeding test data..."
compose run -T --no-deps --rm backend_seed

echo "Starting backend container..."
compose up -d feature_toggle_backend

echo "Waiting for backend health endpoint at ${API_BASE_URL_VALUE}/health ..."
deadline=$((SECONDS + WAIT_SECONDS))
until "$CURL_BIN" -fsS "${API_BASE_URL_VALUE}/health" >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    echo "Backend did not become healthy within ${WAIT_SECONDS} seconds."
    echo "Recent backend logs:"
    compose logs --tail=120 feature_toggle_backend || true
    exit 1
  fi
  sleep 2
done

echo "Running API tests against ${API_BASE_URL_VALUE}"
(
  cd "$ROOT_DIR"
  API_BASE_URL="$API_BASE_URL_VALUE" pnpm --dir api-tests test "$@"
)
