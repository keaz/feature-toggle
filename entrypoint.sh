#!/bin/bash
set -e

# Wait for PostgreSQL to be ready
#echo "Waiting for PostgreSQL to be ready..."
#until PGPASSWORD=local123 psql -h postgres_server -U postgres -d feature_toggle -c '\q'; do
#  echo "PostgreSQL is unavailable - sleeping"
#  sleep 1
#done
sleep 10
echo "PostgreSQL is up - executing migrations"

# Run migrations
cd /app
sqlx feature-toggle-backend/migrate run --database-url ${DATABASE_URL}

echo "Migrations completed - running tests"

cargo test
