# JWT Secret Diagnostics Guide

## Expected Database State

### JWT Secrets Table
You should see **exactly ONE active secret**:

```sql
SELECT 
    id,
    LEFT(secret, 20) || '...' as secret_preview,
    is_active,
    created_at,
    created_by,
    expires_at
FROM jwt_secrets
ORDER BY created_at DESC;
```

**Expected Output:**
```
id                                   | secret_preview           | is_active | created_at          | created_by | expires_at
-------------------------------------|--------------------------|-----------|---------------------|------------|------------
550e8400-e29b-41d4-a716-446655440000 | dGVzdF9zZWNyZXRfa...    | true      | 2025-10-26 10:30:00 | NULL       | NULL
```

### Why Only One Active Secret?

This is **correct and intentional**:

1. **Database Constraint**: `CREATE UNIQUE INDEX idx_jwt_secrets_active_unique ON jwt_secrets (is_active) WHERE is_active = TRUE;`
2. **Purpose**: Ensures all backend pods use the same secret
3. **Multi-Instance Safe**: All pods read from the same database record

## Diagnostic Queries

### 1. Check Active Secret
```sql
-- Should return exactly 1 row
SELECT COUNT(*) as active_secrets_count 
FROM jwt_secrets 
WHERE is_active = true;
```
**Expected**: `active_secrets_count = 1`

### 2. Check All Secrets (Including History)
```sql
-- Shows rotation history
SELECT 
    id,
    LEFT(secret, 30) as secret_start,
    is_active,
    created_at,
    CASE 
        WHEN created_by IS NULL THEN 'System (Auto-Init)'
        ELSE 'Admin User: ' || created_by::text
    END as created_by_info
FROM jwt_secrets
ORDER BY created_at DESC;
```

### 3. Check JWT Tokens
```sql
-- Should show tokens created by users
SELECT 
    COUNT(*) as total_tokens,
    COUNT(*) FILTER (WHERE is_revoked = false AND expires_at > NOW()) as active_tokens,
    COUNT(*) FILTER (WHERE is_revoked = true) as revoked_tokens,
    COUNT(*) FILTER (WHERE expires_at <= NOW()) as expired_tokens
FROM jwt_tokens;
```

### 4. Check Recent Token Activity
```sql
-- Shows recent login activity
SELECT 
    jt.id,
    jt.user_id,
    u.username,
    jt.created_at as logged_in_at,
    jt.expires_at,
    jt.is_revoked,
    CASE 
        WHEN jt.expires_at <= NOW() THEN 'Expired'
        WHEN jt.is_revoked THEN 'Revoked'
        ELSE 'Active'
    END as status
FROM jwt_tokens jt
JOIN users u ON jt.user_id = u.id
ORDER BY jt.created_at DESC
LIMIT 10;
```

## Verify Multi-Instance Setup

### Check Backend Pods
```bash
# See how many backend pods are running
kubectl get pods -l app=fluxgate-backend

# Check logs for JWT secret initialization
kubectl logs -l app=fluxgate-backend | grep "JWT secret initialized"

# Should see one line per pod, all successful
```

### Test Authentication Flow

1. **Login and get token:**
```bash
TOKEN=$(curl -X POST http://your-backend/graphql \
  -H "Content-Type: application/json" \
  -d '{"query":"mutation { login(username:\"admin\", password:\"password\") { token user { username } } }"}' \
  | jq -r '.data.login.token')

echo "Token: $TOKEN"
```

2. **Verify token was stored:**
```sql
-- Run in database
SELECT COUNT(*) FROM jwt_tokens WHERE is_revoked = false;
-- Should increase by 1 after login
```

3. **Make authenticated request:**
```bash
curl -X POST http://your-backend/graphql \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"query":"query { applicationStatus { version } }"}' \
  | jq
```

4. **Check logs for which pod handled it:**
```bash
kubectl logs -l app=fluxgate-backend --tail=50 | grep -E "JWT|Pod:"
```

## Common Issues and Solutions

### Issue: No Active Secret in Database
**Symptoms:** 
- `SELECT COUNT(*) FROM jwt_secrets WHERE is_active = true;` returns 0
- Logs show: "No active JWT secret available"

**Solution:**
```sql
-- Manually create a secret
INSERT INTO jwt_secrets (secret, is_active, created_by)
VALUES (
    encode(gen_random_bytes(32), 'base64'),  -- Generate secure random secret
    true,
    NULL
);
```

Or restart the backend pods to trigger automatic initialization.

### Issue: Multiple Active Secrets (Database Constraint Error)
**Symptoms:**
- Error: `duplicate key value violates unique constraint "idx_jwt_secrets_active_unique"`

**Solution:**
```sql
-- Manually fix: Deactivate all secrets
UPDATE jwt_secrets SET is_active = false;

-- Keep only the most recent one active
UPDATE jwt_secrets 
SET is_active = true 
WHERE id = (
    SELECT id FROM jwt_secrets 
    ORDER BY created_at DESC 
    LIMIT 1
);
```

### Issue: Tokens Not Being Stored
**Symptoms:**
- Can login but subsequent requests fail with 401
- `jwt_tokens` table is empty

**Check:**
```sql
-- See if any tokens exist
SELECT COUNT(*) FROM jwt_tokens;

-- Check for database errors in application logs
kubectl logs -l app=fluxgate-backend | grep -i "error.*jwt_token"
```

### Issue: Authentication Works on Some Pods, Not Others
**Symptoms:**
- Intermittent 401 errors
- Logs show different pods with different errors

**Diagnose:**
```bash
# Check if all pods can reach database
for pod in $(kubectl get pods -l app=fluxgate-backend -o name); do
    echo "Testing $pod"
    kubectl exec $pod -- sh -c 'psql $DATABASE_URL -c "SELECT 1"' || echo "DB connection failed for $pod"
done

# Check if all pods have same secret
kubectl exec -it <pod-1> -- sh -c 'psql $DATABASE_URL -c "SELECT LEFT(secret, 20) FROM jwt_secrets WHERE is_active = true"'
kubectl exec -it <pod-2> -- sh -c 'psql $DATABASE_URL -c "SELECT LEFT(secret, 20) FROM jwt_secrets WHERE is_active = true"'
# Should return same value
```

## Monitoring Queries

### Dashboard Query: JWT Secret Status
```sql
SELECT 
    (SELECT COUNT(*) FROM jwt_secrets WHERE is_active = true) as active_secrets,
    (SELECT COUNT(*) FROM jwt_secrets WHERE is_active = false) as inactive_secrets,
    (SELECT created_at FROM jwt_secrets WHERE is_active = true) as active_secret_created_at,
    (SELECT COUNT(*) FROM jwt_tokens WHERE is_revoked = false AND expires_at > NOW()) as active_tokens,
    (SELECT COUNT(DISTINCT user_id) FROM jwt_tokens WHERE is_revoked = false AND expires_at > NOW()) as active_users;
```

### Alert Query: No Active Secret
```sql
-- Alert if no active secret exists
SELECT 
    CASE 
        WHEN COUNT(*) = 0 THEN 'CRITICAL: No active JWT secret found!'
        WHEN COUNT(*) > 1 THEN 'ERROR: Multiple active JWT secrets found!'
        ELSE 'OK'
    END as status
FROM jwt_secrets 
WHERE is_active = true;
```

## Summary

✅ **ONE active JWT secret is correct** - All pods use the same secret from database  
✅ **Advisory locks prevent race conditions** - Safe multi-pod startup  
✅ **Historical secrets remain** - Audit trail with `is_active = false`  
✅ **Automatic rotation supported** - Generate new secret, old ones auto-deactivated  

Your JWT setup is working as designed! If you're experiencing authentication issues, use the diagnostic queries above to identify the specific problem.
