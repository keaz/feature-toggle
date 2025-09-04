# JWT Authentication Migration

This document outlines the transition from session-based cookie authentication to JWT-based stateless authentication in the feature-toggle backend.

## Changes Made

### 1. Dependencies Updated

**Removed:**
- `actix-session` - Session management middleware

**Added:**
- `jsonwebtoken` - JWT token creation and validation

### 2. New JWT Middleware

Created `src/middleware/jwt_guard.rs` with:
- `JwtGuard` middleware for validating JWT tokens
- `Claims` struct defining JWT payload structure
- `create_jwt_token()` function for generating tokens
- Token verification logic

### 3. Configuration Changes

**Added to `config.toml`:**
```toml
# JWT secret for token signing and verification
# IMPORTANT: Change this to a secure random string in production
jwt_secret = "your_secure_jwt_secret_change_in_production_minimum_32_characters"
```

**Added to `src/config.rs`:**
- `jwt_secret` field in Config struct
- Default JWT secret for development

### 4. GraphQL Schema Updates

**New Types:**
- `LoginResponse` - Contains user data and JWT token

**Updated Mutations:**
- `login` - Now returns `LoginResponse` with JWT token instead of just `User`

### 5. Authentication Flow Changes

**Before (Session-based):**
1. User logs in via GraphQL mutation
2. Server sets session cookie
3. Subsequent requests verified via session middleware
4. User data retrieved from session

**After (JWT-based):**
1. User logs in via GraphQL mutation
2. Server returns JWT token in response
3. Client stores JWT token (typically in localStorage)
4. Client sends token in `Authorization: Bearer <token>` header
5. JWT middleware validates token and extracts user data

### 6. Data Structure Changes

**Replaced:**
- `SessionUser` struct with `JwtUser` struct
- Session data injection with JWT user data injection

**JWT Claims:**
```rust
pub struct Claims {
    pub sub: String,      // user id
    pub username: String,
    pub is_admin: bool,
    pub exp: usize,       // expiration timestamp
    pub iat: usize,       // issued at timestamp
}
```

### 7. Middleware Stack Updates

**Removed:**
- `SessionMiddleware`
- `SessionGuard` (replaced with pass-through stub)

**Added:**
- `JwtGuard` middleware

**Updated order:**
```rust
App::new()
    .wrap(JwtGuard::new(cfg.allowed_origin.clone(), cfg.jwt_secret.clone()))
    .wrap(AdminGuard::new(...))
    .wrap(AccessLogger)
    .wrap(cors)
```

## Security Considerations

### Token Security
- JWT tokens contain sensitive user information
- Tokens should be transmitted over HTTPS only
- Consider shorter expiration times for sensitive operations

### Secret Management
- JWT secret must be kept secure and unique per environment
- Consider using environment variables for secrets in production
- Secret should be at least 32 characters long

### Token Storage (Frontend)
- localStorage: Vulnerable to XSS but survives browser restarts
- sessionStorage: Cleared on browser close, slightly more secure
- HTTP-only cookies: Most secure but requires additional CSRF protection

## Frontend Migration

Frontend applications need to be updated to:

1. **Store JWT tokens** received from login response
2. **Send tokens** in Authorization header: `Authorization: Bearer <token>`
3. **Handle token expiration** by redirecting to login
4. **Remove session cookie handling**

### Example Frontend Changes

**Login Request:**
```javascript
// Before
const response = await fetch('/graphql', {
  method: 'POST',
  credentials: 'include', // for cookies
  body: JSON.stringify({
    query: 'mutation { login(input: {...}) { id username } }'
  })
});

// After
const response = await fetch('/graphql', {
  method: 'POST',
  body: JSON.stringify({
    query: 'mutation { login(input: {...}) { user { id username } token } }'
  })
});
const { data } = await response.json();
localStorage.setItem('token', data.login.token);
```

**Authenticated Requests:**
```javascript
// Before
const response = await fetch('/graphql', {
  method: 'POST',
  credentials: 'include', // session cookie sent automatically
  body: JSON.stringify({ query: '...' })
});

// After
const token = localStorage.getItem('token');
const response = await fetch('/graphql', {
  method: 'POST',
  headers: {
    'Authorization': `Bearer ${token}`
  },
  body: JSON.stringify({ query: '...' })
});
```

## Testing

Run the included test script to verify JWT authentication:

```bash
./test_jwt.sh
```

This script tests:
- Login mutation returns JWT token
- Authenticated requests work with valid token
- Unauthenticated requests are properly rejected

## Migration Benefits

1. **Stateless**: No server-side session storage required
2. **Scalable**: Easy to scale horizontally without session affinity
3. **Cross-domain**: JWT tokens work across different domains
4. **Mobile-friendly**: Better suited for mobile apps and SPAs
5. **Microservices**: Tokens can be validated by any service with the secret

## Backward Compatibility

- Session-related middleware is stubbed out to prevent breakage
- Admin guard functionality remains unchanged
- All existing GraphQL queries and mutations work the same way
- Database schema unchanged

## Production Checklist

- [ ] Update JWT secret to a secure random value
- [ ] Enable HTTPS in production
- [ ] Configure proper CORS headers
- [ ] Update frontend to use JWT tokens
- [ ] Set appropriate token expiration times
- [ ] Monitor for authentication errors
- [ ] Remove session-related code after full migration
