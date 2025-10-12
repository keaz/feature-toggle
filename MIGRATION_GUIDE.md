# Migration Guide: Enum Case Update

## Overview
This guide describes how to apply the GraphQL enum case fix (P1-1) which updates `FeatureType` and `ClientType` enums from PascalCase to UPPERCASE.

## Changes
- `Simple` → `SIMPLE`
- `Contextual` → `CONTEXTUAL`
- `Web` → `WEB`
- `Backend` → `BACKEND`

## Migration Steps

### 1. Run Database Migration

The migration `20251012000000_update_enum_to_uppercase.sql` will:
1. Drop existing CHECK constraints
2. Update all existing data to uppercase
3. Add new CHECK constraints with uppercase values

```bash
cd feature-toggle/feature-toggle-backend
DATABASE_URL=postgres://postgres:your_password@localhost:5432/feature_toggle sqlx migrate run
```

### 2. Apply Seed Data (Optional - for testing)

If you're running in a test environment, apply the updated seed data:

```bash
psql -U postgres -d feature_toggle -f feature-toggle/init.sql
```

### 3. Rebuild and Test

```bash
# From feature-toggle/ directory
cargo build --workspace
cargo test --workspace
```

## Testing the Migration

### Unit Tests
The enum serialization tests verify the Rust enums serialize correctly:

```bash
cargo test --test enum_serialization_test
```

Expected output:
```
running 6 tests
test test_client_type_deserialization ... ok
test test_feature_type_serialization ... ok
test test_graphql_client_type_serialization ... ok
test test_client_type_serialization ... ok
test test_feature_type_deserialization ... ok
test test_graphql_feature_type_serialization ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### Integration Tests
After running the migration, the integration tests should pass:

```bash
DATABASE_URL=postgres://postgres:your_password@localhost:5432/feature_toggle cargo test --lib -p feature-toggle-backend
```

## Rollback (If Needed)

If you need to rollback this change:

```sql
-- Drop the new constraints
ALTER TABLE features DROP CONSTRAINT IF EXISTS features_feature_type_check;
ALTER TABLE clients DROP CONSTRAINT IF EXISTS clients_client_type_check;

-- Revert data to PascalCase
UPDATE features SET feature_type = 'Simple' WHERE feature_type = 'SIMPLE';
UPDATE features SET feature_type = 'Contextual' WHERE feature_type = 'CONTEXTUAL';

UPDATE clients SET client_type = 'Web' WHERE client_type = 'WEB';
UPDATE clients SET client_type = 'Backend' WHERE client_type = 'BACKEND';

-- Re-add old constraints
ALTER TABLE features ADD CONSTRAINT features_feature_type_check
    CHECK (feature_type IN ('Simple', 'Contextual'));

ALTER TABLE clients ADD CONSTRAINT clients_client_type_check
    CHECK (client_type IN ('Web', 'Backend'));
```

## Verification

After migration, verify the changes:

```sql
-- Check features table
SELECT feature_type, COUNT(*) FROM features GROUP BY feature_type;
-- Expected: SIMPLE, CONTEXTUAL

-- Check clients table
SELECT client_type, COUNT(*) FROM clients GROUP BY client_type;
-- Expected: WEB, BACKEND

-- Verify constraints
SELECT conname, pg_get_constraintdef(oid)
FROM pg_constraint
WHERE conname IN ('features_feature_type_check', 'clients_client_type_check');
```

## GraphQL Schema Verification

After deploying the backend, verify the GraphQL schema:

```graphql
query {
  __type(name: "FeatureType") {
    enumValues {
      name
    }
  }
  __type(name: "ClientType") {
    enumValues {
      name
    }
  }
}
```

Expected response:
```json
{
  "data": {
    "__type": {
      "enumValues": [
        { "name": "SIMPLE" },
        { "name": "CONTEXTUAL" }
      ]
    }
  }
}
```

## Notes

- The migration is safe to run multiple times (it uses `DROP CONSTRAINT IF EXISTS`)
- All existing data will be automatically converted to uppercase
- The Rust code uses serde rename attributes to maintain the enum variant names (`Simple`, `Contextual`, etc.) while serializing to uppercase
- No changes needed to the UI code - GraphQL queries will work with the new uppercase values
