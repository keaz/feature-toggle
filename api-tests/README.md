# FluxGate API Automation Tests

Comprehensive API automation tests for the FluxGate backend REST APIs.

## Prerequisites

- Node.js >= 18.0.0
- pnpm
- FluxGate backend running at `http://localhost:8080`

## Installation

```bash
pnpm install
```

## Running Tests

### Start the Backend

Before running tests, ensure the FluxGate backend is running:

```bash
# From project root
make up
```

### Run All Tests

```bash
pnpm test
```

### Run Specific Test Suites

```bash
pnpm test:environment
pnpm test:context
pnpm test:team
pnpm test:role
pnpm test:user
pnpm test:client
pnpm test:feature
pnpm test:pipeline
pnpm test:criteria
pnpm test:approval
pnpm test:auth
```

### Watch Mode

```bash
pnpm test:watch
```

## Configuration

Tests use the following environment variables (with defaults):

| Variable | Default | Description |
|----------|---------|-------------|
| `API_BASE_URL` | `http://localhost:8080/api/v1` | Base URL for the API |
| `API_USERNAME` | `admin` | Username for authentication |
| `API_PASSWORD` | `password123` | Password for authentication |

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
