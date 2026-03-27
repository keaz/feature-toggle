# FluxGate API Automation Tests

Comprehensive API automation tests for the FluxGate backend REST APIs.

## Prerequisites

- Node.js >= 18.0.0
- pnpm
- Docker + Docker Compose plugin (`docker compose`)

## Installation

```bash
pnpm install
```

## Running Tests

### Recommended: Run Against Docker Compose Stack

This runs API tests against an isolated Docker environment:
- PostgreSQL in Docker
- backend in Docker
- SQLx migrations first, then seed fixtures from `init.sql`

```bash
pnpm --dir api-tests run test:docker
```

Useful variants:

```bash
# Keep stack running after tests (for debugging)
pnpm --dir api-tests run test:docker:keep

# Tear down stack manually
pnpm --dir api-tests run docker:down
```

Manual Docker flow (when you want explicit control):

```bash
pnpm --dir api-tests run docker:up
pnpm --dir api-tests run docker:seed
pnpm --dir api-tests run docker:start-backend
pnpm --dir api-tests test
pnpm --dir api-tests run docker:down
```

`docker:seed` runs non-interactive (`-T --no-deps --rm`) so it exits automatically after migrations and seeding are complete.

### Manual Local Backend (Optional)

If you already run backend locally, run tests directly:

```bash
pnpm --dir api-tests test
```

### Run Specific Test Suites

```bash
pnpm --dir api-tests test:environment
pnpm --dir api-tests test:context
pnpm --dir api-tests test:team
pnpm --dir api-tests test:role
pnpm --dir api-tests test:user
pnpm --dir api-tests test:client
pnpm --dir api-tests test:feature
pnpm --dir api-tests test:pipeline
pnpm --dir api-tests test:criteria
pnpm --dir api-tests test:approval
pnpm --dir api-tests test:auth
```

### Watch Mode

```bash
pnpm --dir api-tests test:watch
```

## Configuration

Tests use the following environment variables (with defaults):

| Variable | Default | Description |
|----------|---------|-------------|
| `API_BASE_URL` | `http://127.0.0.1:18080/api/v1` | Base URL for the API |
| `API_USERNAME` | `api-test-admin` | Username for authentication |
| `API_PASSWORD` | `password123` | Password for authentication |

For `test:docker`, the runner sets:
- `API_BASE_URL=http://127.0.0.1:18080/api/v1`

Runner controls:
- `API_TEST_BACKEND_PORT` (default `18080`)
- `API_TEST_WAIT_SECONDS` (default `180`)
- `API_TEST_KEEP_STACK=1` (skip automatic teardown)
- `API_TEST_COMPOSE_PROJECT` (default `feature-toggle-api-tests`)

## Test Structure

```
api-tests/
├── src/
│   ├── tests/              # Test files
│   │   ├── environment.test.ts
│   │   ├── context.test.ts
│   │   ├── team.test.ts
│   │   ├── role.test.ts
│   │   ├── user.test.ts
│   │   ├── client.test.ts
│   │   ├── feature.test.ts
│   │   ├── pipeline.test.ts
│   │   ├── criteria.test.ts
│   │   ├── approval.test.ts
│   │   └── auth.test.ts
│   └── utils/              # Shared utilities
│       ├── api-client.ts   # HTTP client with auth
│       ├── test-fixtures.ts # Test data generators
│       ├── test-utils.ts   # Assertion helpers
│       └── test-setup.ts   # Jest setup
├── package.json
├── tsconfig.json
├── jest.config.js
└── README.md
```

## Test Coverage

Each test file covers:

- **CRUD Operations**: Create, Read, Update, Delete
- **Authentication**: Valid/invalid tokens, unauthenticated access
- **Validation**: Empty fields, invalid formats, duplicate entries
- **Authorization**: Admin-only operations
- **Edge Cases**: Non-existent resources, invalid UUIDs

## API Endpoints Tested

| Module | Endpoints |
|--------|-----------|
| Environment | List, Get, Create, Update, Delete |
| Context | List, Get, Create, Update, Delete |
| Team | List, Create, Update |
| Role | List, Create, Delete |
| User | List, Get, Create, Update, Team/Role assignment |
| Client | List, Get, Create, Update |
| Feature | List, Get, Create, Update, Toggle, Emergency Disable |
| Pipeline | List, Get, Create, Update |
| Criteria | List, Create, Update, Delete |
| Approval | List requests, Get/Create/Update/Delete policies, Approve/Reject |
| Auth | Login, Logout, Status, Password Reset |
