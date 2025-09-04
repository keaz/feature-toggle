# Role-Based Access Control (RBAC) for Feature Stage Deployment

This document explains the RBAC implementation for controlling feature stage deployment operations.

## Overview

The system implements role-based authorization to control who can perform specific feature stage operations:

- **Requester Role**: Can request deployments and rollbacks
- **Approver Role**: Can approve or reject deployment/rollback requests

## Roles and Permissions

### Requester Role
Users with the "Requester" role can perform the following operations:
- Request feature deployment (`DEPLOYMENT_REQUESTED`)
- Request feature rollback (`ROLLBACK_REQUESTED`)

### Approver Role  
Users with the "Approver" role can perform the following operations:
- Approve feature deployment (`DEPLOYED`)
- Reject feature deployment (`DEPLOYMENT_REJECTED`)
- Approve feature rollback (`ROLLBACKED`)
- Reject feature rollback (`ROLLBACK_REJECTED`)

## Implementation Details

### Authorization Logic
The authorization is implemented in `src/logic/authorization.rs` with the `RoleAuthorizer` struct:

```rust
// Check if user can perform the requested operation
RoleAuthorizer::authorize_stage_change_request(&user.roles, request_type)
```

### GraphQL Mutation Integration
The `requestStageChange` mutation in `src/graphql/mutation.rs` includes authorization checks:

1. Extract user roles from JWT token
2. Validate permission based on request type
3. Proceed with operation if authorized
4. Return authorization error if not permitted

### Stage Change Request Types
The system recognizes these stage change request types:

**Requester Operations:**
- `DEPLOYMENT_REQUESTED` - Request to deploy a feature stage
- `ROLLBACK_REQUESTED` - Request to rollback a feature stage

**Approver Operations:**
- `DEPLOYED` - Approve and deploy the feature stage
- `DEPLOYMENT_REJECTED` - Reject the deployment request
- `ROLLBACKED` - Approve and rollback the feature stage  
- `ROLLBACK_REJECTED` - Reject the rollback request

## Usage Examples

### GraphQL Mutations

**Request Deployment (Requester role required):**
```graphql
mutation {
  requestStageChange(
    stageId: "stage-uuid"
    request: DEPLOYMENT_REQUESTED
  ) {
    id
    key
    stages {
      id
      status
    }
  }
}
```

**Approve Deployment (Approver role required):**
```graphql
mutation {
  requestStageChange(
    stageId: "stage-uuid"
    request: DEPLOYED
  ) {
    id
    key
    stages {
      id
      status
    }
  }
}
```

**Request Rollback (Requester role required):**
```graphql
mutation {
  requestStageChange(
    stageId: "stage-uuid"
    request: ROLLBACK_REQUESTED
  ) {
    id
    key
    stages {
      id
      status
    }
  }
}
```

**Reject Rollback (Approver role required):**
```graphql
mutation {
  requestStageChange(
    stageId: "stage-uuid"
    request: ROLLBACK_REJECTED
  ) {
    id
    key
    stages {
      id
      status
    }
  }
}
```

## Error Handling

If a user attempts an unauthorized operation, the system returns a GraphQL error:

```json
{
  "errors": [
    {
      "message": "Only users with 'Requester' role can request deployments or rollbacks"
    }
  ]
}
```

or

```json
{
  "errors": [
    {
      "message": "Only users with 'Approver' role can approve or reject requests"
    }
  ]
}
```

## Role Assignment

Users can be assigned roles using the existing role management system:

```graphql
mutation {
  assignUserRoles(
    userId: "user-uuid"
    input: {
      roleIds: ["role-uuid-1", "role-uuid-2"]
    }
  ) {
    id
    username
    roles {
      name
    }
  }
}
```

## Security Notes

1. **JWT Token Validation**: Authorization checks are performed on every request using roles embedded in JWT tokens
2. **Role Validation**: The system validates that users have the exact required role for each operation
3. **Multiple Roles**: Users can have multiple roles (e.g., both Requester and Approver)
4. **Fail-Safe**: Operations are denied by default if the user lacks the required role

## Testing

The RBAC implementation includes comprehensive tests covering:
- Unit tests for authorization logic
- Integration tests for GraphQL mutations
- Error handling scenarios
- Multiple role scenarios

Run tests with:
```bash
cargo test authorization
cargo test request_stage_change
```
