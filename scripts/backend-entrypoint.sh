#!/bin/bash
set -e

echo "Starting feature-toggle-backend..."
# Never print DATABASE_URL itself: it contains the database password.
echo "DATABASE_URL: $(echo "$DATABASE_URL" | sed 's#://[^@]*@#://***@#')"

# Handle configuration file mounting
if [ -f "/app/config/config.toml" ]; then
    echo "Using mounted config.toml from /app/config/config.toml"
    cp /app/config/config.toml /app/config.toml
else
    echo "No mounted config found, using default config"
    cp /app/config.toml.default /app/config.toml
fi

# Wait for PostgreSQL to be ready
# DB_PASSWORD=$(echo "$DATABASE_URL" | sed -n 's/.*:\/\/.*:\(.*\)@.*/\1/p')
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
