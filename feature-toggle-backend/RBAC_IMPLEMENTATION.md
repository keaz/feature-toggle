# Role-Based Access Control (RBAC) Implementation

This document describes the role-based access control system that has been implemented for the feature toggle backend.

## Overview

The RBAC system allows users to have one or more roles assigned to them. All roles are predefined in the system and stored in the database.

## Database Schema

### Tables Created

1. **roles** - Stores predefined role definitions
   - `id` (UUID, Primary Key)
   - `name` (VARCHAR(50), Unique) - Role name
   - `description` (TEXT) - Role description
   - `created_at` (TIMESTAMPTZ)
   - `updated_at` (TIMESTAMPTZ)

2. **user_roles** - Junction table for many-to-many relationship between users and roles
   - `id` (UUID, Primary Key)
   - `user_id` (UUID, Foreign Key to users.id)
   - `role_id` (UUID, Foreign Key to roles.id)
   - `assigned_at` (TIMESTAMPTZ)
   - `assigned_by` (UUID, Foreign Key to users.id, nullable)
   - Unique constraint on (user_id, role_id)

### Predefined Roles

The system comes with three predefined roles:

1. **Approver** - Can approve deployment requests and stage changes
2. **Requester** - Can request deployment and stage changes  
3. **Team Admin** - Can manage team settings and members

## Architecture

### Database Layer (`src/database/role.rs`)

- `RoleRepository` trait with methods:
  - `get_all_roles()` - Retrieve all roles in the system
  - `get_role_by_id(id)` - Get a specific role by ID
  - `get_role_by_name(name)` - Get a role by name
  - `get_user_roles(user_id)` - Get all roles assigned to a user
  - `assign_user_roles(user_id, role_ids, assigned_by)` - Assign multiple roles to a user
  - `remove_user_role(user_id, role_id)` - Remove a specific role from a user
  - `user_has_role(user_id, role_name)` - Check if a user has a specific role

### Logic Layer (`src/logic/role.rs`)

- `RoleLogic` trait that provides business logic for role operations
- Handles ID conversions between GraphQL IDs and UUIDs
- Provides proper error handling and validation

### GraphQL API

#### Queries

1. **roles** - Get all roles in the system
   ```graphql
   query {
     roles {
       id
       name
       description
       createdAt
       updatedAt
     }
   }
   ```

2. **userRoles(userId: ID!)** - Get roles assigned to a specific user
   ```graphql
   query {
     userRoles(userId: "user-id") {
       id
       name
       description
       createdAt
       updatedAt
     }
   }
   ```

3. **User.roles** - Complex field on User type to get user's roles
   ```graphql
   query {
     users {
       items {
         id
         username
         roles {
           id
           name
           description
         }
       }
     }
   }
   ```

#### Mutations

1. **assignUserRoles** - Assign multiple roles to a user
   ```graphql
   mutation {
     assignUserRoles(
       userId: "user-id", 
       input: { roleIds: ["role-id-1", "role-id-2"] }
     ) {
       id
       name
       description
     }
   }
   ```

## Usage Examples

### GraphQL Queries

**Get all roles:**
```graphql
query {
  roles {
    id
    name
    description
  }
}
```

**Get user with their roles:**
```graphql
query {
  users {
    items {
      id
      username
      roles {
        name
        description
      }
    }
  }
}
```

**Assign roles to a user:**
```graphql
mutation AssignRoles($userId: ID!, $roleIds: [ID!]!) {
  assignUserRoles(userId: $userId, input: { roleIds: $roleIds }) {
    id
    name
    description
  }
}
```

## Migration

The role system was added via migration `20250903000000_create_roles.sql` which:

1. Creates the `roles` table
2. Creates the `user_roles` junction table  
3. Inserts the three predefined roles
4. Creates appropriate indexes for performance

## Testing

Comprehensive tests have been added at all layers:

- **Database tests** - Test repository methods with mocks
- **Logic tests** - Test business logic and ID conversions
- **GraphQL tests** - Test queries and mutations with mocked dependencies

All tests can be run with:
```bash
cargo test roles --lib
```

## Integration

The role system is fully integrated into the application:

- Role logic is registered in `lib.rs` and provided to the GraphQL schema
- Role repository uses the existing database pool
- Proper error handling using the existing `Error` enum
- Follows the same patterns as other entities in the system

## Security Considerations

- Role assignments track who assigned the role (`assigned_by` field)
- All role operations require authentication (use JWT user context)
- Role names are unique to prevent conflicts
- Database constraints prevent duplicate role assignments
- Proper input validation at all layers

## Future Enhancements

The current implementation provides a solid foundation for:

- Permission-based authorization (roles can be extended with permissions)
- Role-based access control in business logic
- Audit trails for role changes
- Role hierarchies (if needed)
- Dynamic role creation (currently roles are predefined)
