# Test Setup Guide

## Why Are Tests Failing?

The following tests are failing:
- `database::feature::tests::test_emergency_disable_feature_with_rollback`
- `database::feature::tests::test_emergency_disable_feature_without_rollback`
- `database::feature::tests::test_emergency_enable_feature`
- `database::feature::tests::test_get_features_pending_rollback_with_eligible_features`
- `database::feature::tests::test_kill_switch_fields_persistence`
- `database::feature::tests::test_rollback_scheduling_edge_cases`

### Root Cause

These tests are failing because of the enum case mismatch fix (P1-1). The code has been updated to use UPPERCASE enum values (`SIMPLE`, `CONTEXTUAL`, `WEB`, `BACKEND`), but the database still has CHECK constraints that only allow PascalCase values (`Simple`, `Contextual`, `Web`, `Backend`).

When tests try to insert data with `'SIMPLE'`, the database rejects it because the CHECK constraint expects `'Simple'`.

## Solution

You need to run the database migration before running tests.

### Step 1: Start PostgreSQL

Make sure PostgreSQL is running:
```bash
# macOS with Homebrew
brew services start postgresql

# Or using Docker
docker run -d \
  --name fluxgate-postgres \
  -e POSTGRES_PASSWORD=local123 \
  -e POSTGRES_DB=feature_toggle \
  -p 5432:5432 \
  postgres:16
```

### Step 2: Set DATABASE_URL

```bash
export DATABASE_URL=postgres://postgres:local123@localhost:5432/feature_toggle
```

### Step 3: Run Migration

Option A - Use the migration script:
```bash
cd feature-toggle
./scripts/run-test-migration.sh
```

Option B - Run manually:
```bash
cd feature-toggle/feature-toggle-backend
sqlx migrate run
```

### Step 4: Apply Seed Data (Optional)

```bash
# From feature-toggle directory
psql -U postgres -d feature_toggle -f init.sql
```

### Step 5: Run Tests

```bash
# From feature-toggle directory
cargo test --workspace
```

## Quick Fix for Test Database

If you just want to fix the CHECK constraints quickly:

```bash
psql -U postgres -d feature_toggle -c "
ALTER TABLE features DROP CONSTRAINT IF EXISTS features_feature_type_check;
ALTER TABLE clients DROP CONSTRAINT IF EXISTS clients_client_type_check;

UPDATE features SET feature_type = 'SIMPLE' WHERE feature_type = 'Simple';
UPDATE features SET feature_type = 'CONTEXTUAL' WHERE feature_type = 'Contextual';
UPDATE clients SET client_type = 'WEB' WHERE client_type = 'Web';
UPDATE clients SET client_type = 'BACKEND' WHERE client_type = 'Backend';

ALTER TABLE features ADD CONSTRAINT features_feature_type_check CHECK (feature_type IN ('SIMPLE', 'CONTEXTUAL'));
ALTER TABLE clients ADD CONSTRAINT clients_client_type_check CHECK (client_type IN ('WEB', 'BACKEND'));
"
```

## Verification

After migration, verify the tests pass:

```bash
# Run only the failing tests
cargo test --lib -p feature-toggle-backend emergency
cargo test --lib -p feature-toggle-backend rollback
cargo test --lib -p feature-toggle-backend kill_switch

# Run all enum serialization tests
cargo test --test enum_serialization_test

# Run all tests
cargo test --workspace
```

## Expected Test Results

### Enum Serialization Tests (Should Pass)
```
running 6 tests
test test_client_type_deserialization ... ok
test test_client_type_serialization ... ok
test test_feature_type_deserialization ... ok
test test_feature_type_serialization ... ok
test test_graphql_client_type_serialization ... ok
test test_graphql_feature_type_serialization ... ok

test result: ok. 6 passed; 0 failed
```

### Integration Tests (Should Pass After Migration)
All database feature tests should pass once the migration is applied.

## Troubleshooting

### Error: "password authentication failed for user postgres"

The database credentials in `DATABASE_URL` are incorrect. Update with your actual PostgreSQL password:
```bash
export DATABASE_URL=postgres://postgres:YOUR_PASSWORD@localhost:5432/feature_toggle
```

### Error: "database 'feature_toggle' does not exist"

Create the database:
```bash
psql -U postgres -c "CREATE DATABASE feature_toggle;"
```

### Error: "sqlx-data.json is not up to date"

Run offline mode preparation:
```bash
cd feature-toggle/feature-toggle-backend
cargo sqlx prepare
```

### Tests still failing

1. Verify the migration was applied:
```sql
SELECT conname, pg_get_constraintdef(oid)
FROM pg_constraint
WHERE conname IN ('features_feature_type_check', 'clients_client_type_check');
```

2. Check existing data:
```sql
SELECT DISTINCT feature_type FROM features;
SELECT DISTINCT client_type FROM clients;
```

Should return `SIMPLE`, `CONTEXTUAL`, `WEB`, `BACKEND` (not PascalCase).

3. Reapply seed data:
```bash
psql -U postgres -d feature_toggle -f feature-toggle/init.sql
```

## CI/CD Considerations

When running tests in CI/CD, ensure:

1. The migration runs before tests:
```yaml
- name: Run migrations
  run: |
    cd feature-toggle/feature-toggle-backend
    sqlx migrate run
  env:
    DATABASE_URL: ${{ secrets.DATABASE_URL }}

- name: Run tests
  run: cargo test --workspace
  env:
    DATABASE_URL: ${{ secrets.DATABASE_URL }}
```

2. Use a fresh database for each test run, or ensure migrations are applied

## Related Documentation

- [MIGRATION_GUIDE.md](../MIGRATION_GUIDE.md) - Complete migration guide
- [TASK_TRACKER.md](../TASK_TRACKER.md) - P1-1 implementation details
