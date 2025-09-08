#!/bin/bash
set -e

echo "Starting feature-toggle-backend..."
echo "DATABASE_URL: $DATABASE_URL"

# Handle configuration file mounting
if [ -f "/app/config/config.toml" ]; then
    echo "Using mounted config.toml from /app/config/config.toml"
    cp /app/config/config.toml /app/config.toml
else
    echo "No mounted config found, using default config"
    cp /app/config.toml.default /app/config.toml
fi

# Extract password from DATABASE_URL
DB_PASSWORD=$(echo "$DATABASE_URL" | sed -n 's/.*:\/\/.*:\(.*\)@.*/\1/p')
echo "Extracted password: '$DB_PASSWORD'"

# Wait for PostgreSQL to be ready
# echo "Waiting for PostgreSQL to be ready..."
# until PGPASSWORD="$DB_PASSWORD" psql -h postgres_server -U postgres -d feature_toggle -c '\q'; do
#   echo "PostgreSQL is unavailable - sleeping"
#   sleep 1
# done

echo "PostgreSQL is up - executing migrations"

# Run migrations
sqlx migrate run --database-url "${DATABASE_URL}" --source ./migrations

echo "Migrations completed - starting backend application"

# Start the application
exec feature-toggle-backend
