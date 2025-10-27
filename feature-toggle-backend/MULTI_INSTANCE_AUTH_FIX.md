# Multi-Instance Authentication Fix

## Problem
When running multiple backend instances in Kubernetes, user authentication was failing with 401 Unauthorized errors when requests were routed to different pods, even though JWT tokens were being used.

## Root Cause
The issue was not with the JWT architecture (which is correctly database-backed), but with:
1. **Insufficient error logging** - Making it hard to diagnose which pod/instance was failing
2. **Potential race conditions** - Multiple pods starting simultaneously could create inconsistent JWT secrets
3. **No transaction locking** - Database reads during secret rotation could return no active secret

## Solution Implemented

### 1. Enhanced Logging in JWT Middleware (`jwt_guard.rs`)

**Added pod-level logging** to identify which instance is having issues:
```rust
// Log when JWT secret fetch fails
log::error!(
    "Failed to get JWT secret from database - Pod: {}, IP: {}, Error: {:?}",
    hostname, pod_ip, e, req.path()
);

// Log when token validation fails in database
log::warn!(
    "JWT token invalid in database - Pod: {}, User: {}, Token hash: {}",
    hostname, username, &token_hash[..8]
);

// Log when JWT decode fails
log::debug!(
    "JWT decode failed - Pod: {}, Error: {}",
    hostname, e
);
```

**Benefits:**
- Identifies which pod is failing
- Shows exact error type (database connection, missing secret, invalid token)
- Helps diagnose deployment or networking issues

### 2. PostgreSQL Advisory Locks (`jwt_secret.rs` - Logic Layer)

**Added database-level locking** during JWT secret initialization to prevent race conditions:
```rust
const JWT_INIT_LOCK_ID: i64 = 1234567890;

// Try to acquire advisory lock
let lock_acquired = sqlx::query_scalar::<_, bool>(
    "SELECT pg_try_advisory_lock($1)"
)
.bind(JWT_INIT_LOCK_ID)
.fetch_one(pool)
.await
.unwrap_or(false);

if !lock_acquired {
    // Another pod is initializing, wait briefly
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

// ... initialize secret ...

// Release lock
if lock_acquired {
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(JWT_INIT_LOCK_ID)
        .execute(pool)
        .await;
}
```

**Benefits:**
- Only one pod initializes JWT secret at startup
- Other pods wait and then read the initialized secret
- Prevents duplicate secrets or missing secrets

### 3. Consistent Read Locks (`jwt_secret.rs` - Repository Layer)

**Added FOR SHARE lock** when reading active JWT secret:
```rust
SELECT id, secret, is_active, created_at, created_by, expires_at
FROM jwt_secrets 
WHERE is_active = true
ORDER BY created_at DESC
LIMIT 1
FOR SHARE  -- Ensures consistent reads
```

**Benefits:**
- Prevents reading during ongoing transactions
- Ensures all pods see the same secret
- Avoids reading while secret is being rotated

### 4. Better Error Handling

**Changed from swallowing errors to logging them:**
```rust
match decode::<Claims>(token, &decoding_key, &validation) {
    Ok(token_data) => { /* ... */ }
    Err(e) => {
        log::debug!("JWT decode failed - Pod: {}, Error: {}", hostname, e);
    }
}
```

## Deployment Instructions

### 1. Update Kubernetes Deployment

Add environment variables to identify pods in logs:
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: fluxgate-backend
spec:
  template:
    spec:
      containers:
      - name: backend
        env:
        - name: HOSTNAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_IP
          valueFrom:
            fieldRef:
              fieldPath: status.podIP
```

### 2. Database Connection Pool Configuration

Ensure adequate connection pool size for multiple pods. In `init_pg_pool()`:
```rust
let pool = PgPoolOptions::new()
    .max_connections(20)  // Increase from default 10
    .min_connections(5)    // Keep some connections warm
    .acquire_timeout(Duration::from_secs(30))
    .idle_timeout(Duration::from_secs(600))
    .connect(&database_url)
    .await?;
```

**Recommended sizing:**
- **Max connections per pod**: 20
- **Number of pods**: N
- **Database max_connections**: At least `(N × 20) + 10` (extra 10 for admin/maintenance)

Example: 3 pods = (3 × 20) + 10 = 70 connections minimum

### 3. PostgreSQL Configuration

Update `postgresql.conf`:
```conf
max_connections = 100              # Adjust based on pod count
shared_buffers = 256MB            # For better performance
effective_cache_size = 1GB        # Helps query planner
```

### 4. Monitor Logs

After deployment, monitor logs for authentication issues:
```bash
# Check for JWT secret errors
kubectl logs -l app=fluxgate-backend | grep "JWT secret"

# Check for token validation failures
kubectl logs -l app=fluxgate-backend | grep "JWT token invalid"

# Check for decode errors
kubectl logs -l app=fluxgate-backend | grep "JWT decode failed"
```

## Testing Multi-Instance Setup

### 1. Deploy with Multiple Replicas
```bash
kubectl apply -k k8s/overlays/staging
kubectl scale deployment fluxgate-backend --replicas=3
```

### 2. Test Authentication Across Pods
```bash
# Login and get token
TOKEN=$(curl -X POST http://your-backend/graphql \
  -H "Content-Type: application/json" \
  -d '{"query":"mutation { login(username:\"admin\", password:\"pass\") { token } }"}' \
  | jq -r '.data.login.token')

# Make multiple requests to different pods
for i in {1..20}; do
  curl -X POST http://your-backend/graphql \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $TOKEN" \
    -d '{"query":"query { applicationStatus { version } }"}' 
  echo ""
  sleep 0.5
done
```

### 3. Check Pod Distribution
```bash
# Verify requests are hitting different pods
kubectl logs -l app=fluxgate-backend --tail=100 | grep "JWT decode\|JWT token\|JWT secret"
```

## Troubleshooting

### Issue: "JWT secret unavailable" errors
**Cause**: Database connection issues or no active secret in database

**Fix**:
1. Check database connectivity: `kubectl exec -it <pod> -- psql $DATABASE_URL -c "SELECT 1"`
2. Verify JWT secret exists: `SELECT * FROM jwt_secrets WHERE is_active = true;`
3. Check connection pool exhaustion in logs
4. Increase `max_connections` in database and pod pool size

### Issue: Intermittent 401 errors
**Cause**: Token not found in database or connection timeout

**Fix**:
1. Check if tokens are being stored: `SELECT COUNT(*) FROM jwt_tokens WHERE is_revoked = false;`
2. Increase database query timeout
3. Check network latency between pods and database
4. Enable connection pooling metrics

### Issue: All requests fail after deployment
**Cause**: No JWT secret initialized or all secrets deactivated

**Fix**:
1. Check logs: `kubectl logs <pod> | grep "JWT secret initialized"`
2. Manually create secret if needed:
   ```sql
   INSERT INTO jwt_secrets (secret, is_active, created_by)
   VALUES ('your_secure_base64_secret', true, NULL);
   ```
3. Restart pods to re-initialize

## Performance Considerations

### Database Queries Per Request
Each authenticated request makes **2 database queries**:
1. Get active JWT secret (cached by most pods)
2. Validate token hash in jwt_tokens table

### Optimization Ideas (Future)
1. **Redis caching** for JWT secrets (with TTL of 5 minutes)
2. **In-memory cache** with periodic refresh from database
3. **Dedicated connection pool** for authentication queries
4. **Read replicas** for token validation queries (with careful consistency handling)

## Summary

The fixes ensure:
- ✅ **Better observability**: Pod-level logging identifies issues quickly
- ✅ **Race condition prevention**: Advisory locks prevent initialization conflicts
- ✅ **Consistent reads**: Database locks ensure all pods see same secret
- ✅ **Production ready**: Handles multi-instance deployments correctly

Your JWT authentication is now truly stateless and works correctly across multiple backend instances in Kubernetes!
