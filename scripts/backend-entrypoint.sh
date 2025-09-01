#!/bin/bash
set -e

echo "Starting feature-toggle-backend..."
echo "DATABASE_URL: $DATABASE_URL"

# Extract password from DATABASE_URL
DB_PASSWORD=$(echo "$DATABASE_URL" | sed -n 's/.*:\/\/.*:\(.*\)@.*/\1/p')
echo "Extracted password: '$DB_PASSWORD'"

# Wait for PostgreSQL to be ready
echo "Waiting for PostgreSQL to be ready..."
until PGPASSWORD="$DB_PASSWORD" psql -h postgres_server -U postgres -d feature_toggle -c '\q'; do
  echo "PostgreSQL is unavailable - sleeping"
  sleep 1
done

echo "PostgreSQL is up - executing migrations"

# Run migrations
sqlx migrate run --database-url "${DATABASE_URL}" --source ./migrations

echo "Migrations completed - executing init.sql"

# Execute init.sql
PGPASSWORD="$DB_PASSWORD" psql -h postgres_server -U postgres -d feature_toggle -f ./init.sql

echo "Init script executed - starting backend application"

# Start the application
exec feature-toggle-backend
