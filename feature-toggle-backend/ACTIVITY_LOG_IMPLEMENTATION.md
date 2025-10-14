# Activity Log Implementation

## Overview

This document describes the implementation of the Activity Log system for tracking user activities, system events, and administrative actions within the Feature Toggle application.

## Implementation Date
- **Created:** January 2025
- **Status:** Complete and Production-Ready

---

## Architecture

The Activity Log system consists of three main layers:

1. **Database Layer** (`src/database/activity_log.rs`)
   - PostgreSQL table with comprehensive indexing
   - Repository pattern for data access
   - Dynamic filtering and pagination

2. **GraphQL API Layer** (`src/graphql/schema.rs` & `src/graphql/query.rs`)
   - `ActivityLog` and `ActivityLogPage` types
   - `recentActivities` query with filtering
   - Pagination support

3. **Helper Utilities** (`src/utils/activity_logger.rs`)
   - Activity type constants
   - Entity type constants
   - Convenience logging functions

---

## Database Schema

### Migration File
Location: `migrations/20251013000000_create_activity_log.sql`

### Table: `activity_log`

```sql
CREATE TABLE IF NOT EXISTS activity_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    activity_type VARCHAR(50) NOT NULL,
    entity_type VARCHAR(50) NOT NULL,
    entity_id VARCHAR(255) NOT NULL,
    actor_id UUID,
    actor_name VARCHAR(255),
    description TEXT NOT NULL,
    metadata JSONB,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);
```

### Fields

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `id` | UUID | No | Primary key, auto-generated |
| `activity_type` | VARCHAR(50) | No | Type of activity (e.g., "feature_created") |
| `entity_type` | VARCHAR(50) | No | Type of entity affected (e.g., "feature") |
| `entity_id` | VARCHAR(255) | No | ID of the affected entity |
| `actor_id` | UUID | Yes | ID of user who performed the action |
| `actor_name` | VARCHAR(255) | Yes | Name of user who performed the action |
| `description` | TEXT | No | Human-readable description |
| `metadata` | JSONB | Yes | Additional context as JSON |
| `created_at` | TIMESTAMP WITH TIME ZONE | No | When the activity occurred |

### Indexes

The following indexes optimize common query patterns:

1. **created_at_idx**: `created_at DESC` - Time-based queries
2. **activity_type_idx**: `activity_type` - Filter by activity type
3. **entity_type_idx**: `entity_type` - Filter by entity type
4. **entity_id_idx**: `entity_id` - Find activities for specific entity
5. **actor_id_idx**: `actor_id` - Find activities by user
6. **activity_type_created_at_idx**: `(activity_type, created_at DESC)` - Combined filter
7. **entity_type_entity_id_idx**: `(entity_type, entity_id, created_at DESC)` - Entity timeline

### Foreign Keys

- `actor_id` references `users(id)` with `ON DELETE SET NULL`
  - When a user is deleted, their activities remain but actor_id becomes NULL

---

## Database Layer API

### Structs

#### ActivityLogRow
Represents a complete activity log record from the database.

```rust
pub struct ActivityLogRow {
    pub id: Uuid,
    pub activity_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}
```

#### CreateActivityLog
Input struct for creating new activity log entries.

```rust
pub struct CreateActivityLog {
    pub activity_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}
```

#### ActivityLogFilter
Options for filtering activity logs.

```rust
pub struct ActivityLogFilter {
    pub activity_type: Option<String>,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub actor_id: Option<Uuid>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
```

### Repository Trait

```rust
pub trait ActivityLogRepository: Send + Sync {
    async fn create_activity(
        &self,
        activity: CreateActivityLog,
    ) -> Result<ActivityLogRow, sqlx::Error>;

    async fn get_activities(
        &self,
        filter: ActivityLogFilter,
    ) -> Result<Vec<ActivityLogRow>, sqlx::Error>;

    async fn get_activities_paginated(
        &self,
        filter: ActivityLogFilter,
    ) -> Result<(Vec<ActivityLogRow>, i64), sqlx::Error>;

    async fn get_activity_count(
        &self,
        filter: ActivityLogFilter,
    ) -> Result<i64, sqlx::Error>;

    fn clone_box(&self) -> Box<dyn ActivityLogRepository>;
}
```

### Usage Examples

#### Creating an Activity Log

```rust
use crate::database::activity_log::{activity_log_repository, CreateActivityLog};
use uuid::Uuid;

let repo = activity_log_repository(pool.clone());

let activity = CreateActivityLog {
    activity_type: "feature_created".to_string(),
    entity_type: "feature".to_string(),
    entity_id: feature_id.to_string(),
    actor_id: Some(user_id),
    actor_name: Some(user_name.clone()),
    description: format!("Feature '{}' created", feature_name),
    metadata: Some(serde_json::json!({
        "feature_name": feature_name,
        "feature_key": feature_key,
        "enabled": true
    })),
};

let result = repo.create_activity(activity).await?;
```

#### Querying Activities with Filters

```rust
use crate::database::activity_log::ActivityLogFilter;
use chrono::{Utc, Duration};

// Get all feature-related activities in the last 7 days
let filter = ActivityLogFilter {
    entity_type: Some("feature".to_string()),
    from_date: Some(Utc::now() - Duration::days(7)),
    limit: Some(50),
    ..Default::default()
};

let activities = repo.get_activities(filter).await?;
```

#### Paginated Query

```rust
// Get page 2 of user activities (20 per page)
let filter = ActivityLogFilter {
    actor_id: Some(user_id),
    limit: Some(20),
    offset: Some(20), // (page - 1) * page_size
    ..Default::default()
};

let (activities, total_count) = repo.get_activities_paginated(filter).await?;
println!("Showing {} of {} total activities", activities.len(), total_count);
```

---

## GraphQL API

### Types

#### ActivityLog

```graphql
type ActivityLog {
  id: ID!
  activityType: String!
  entityType: String!
  entityId: String!
  actorId: ID
  actorName: String
  description: String!
  metadata: JSON
  createdAt: DateTime!
}
```

#### ActivityLogPage

```graphql
type ActivityLogPage {
  items: [ActivityLog!]!
  pageNumber: Int!
  pageSize: Int!
  total: Int!
}
```

### Query

#### recentActivities

```graphql
type Query {
  recentActivities(
    activityType: String
    entityType: String
    entityId: String
    actorId: ID
    fromDate: DateTime
    toDate: DateTime
    pageNumber: Int
    pageSize: Int
  ): ActivityLogPage!
}
```

**Parameters:**
- `activityType`: Filter by activity type (e.g., "feature_created")
- `entityType`: Filter by entity type (e.g., "feature")
- `entityId`: Filter by specific entity ID
- `actorId`: Filter by user who performed the action
- `fromDate`: Filter activities after this date
- `toDate`: Filter activities before this date
- `pageNumber`: Page number (default: 1)
- `pageSize`: Items per page (default: 20)

### GraphQL Query Examples

#### Get Recent Activities (Latest 20)

```graphql
query {
  recentActivities {
    items {
      id
      activityType
      entityType
      entityId
      actorName
      description
      createdAt
    }
    pageNumber
    pageSize
    total
  }
}
```

#### Get Feature-Related Activities

```graphql
query {
  recentActivities(entityType: "feature", pageSize: 50) {
    items {
      id
      activityType
      description
      actorName
      metadata
      createdAt
    }
    total
  }
}
```

#### Get Activities by Specific User

```graphql
query {
  recentActivities(actorId: "user-uuid-here", pageNumber: 1, pageSize: 25) {
    items {
      id
      activityType
      entityType
      entityId
      description
      createdAt
    }
    pageNumber
    pageSize
    total
  }
}
```

#### Get Activities by Type and Date Range

```graphql
query {
  recentActivities(
    activityType: "feature_deployed"
    fromDate: "2025-01-01T00:00:00Z"
    toDate: "2025-01-31T23:59:59Z"
  ) {
    items {
      id
      description
      entityId
      actorName
      metadata
      createdAt
    }
    total
  }
}
```

#### Get Kill Switch Activations

```graphql
query {
  recentActivities(activityType: "kill_switch_activated") {
    items {
      id
      description
      entityId
      actorName
      metadata
      createdAt
    }
    total
  }
}
```

---

## Helper Utilities

### Activity Type Constants

Location: `src/utils/activity_logger.rs`

#### Feature Activities
```rust
pub const FEATURE_CREATED: &str = "feature_created";
pub const FEATURE_UPDATED: &str = "feature_updated";
pub const FEATURE_DELETED: &str = "feature_deleted";
pub const FEATURE_DEPLOYED: &str = "feature_deployed";
pub const FEATURE_ENABLED: &str = "feature_enabled";
pub const FEATURE_DISABLED: &str = "feature_disabled";
pub const KILL_SWITCH_ACTIVATED: &str = "kill_switch_activated";
pub const KILL_SWITCH_DEACTIVATED: &str = "kill_switch_deactivated";
```

#### User Activities
```rust
pub const USER_CREATED: &str = "user_created";
pub const USER_UPDATED: &str = "user_updated";
pub const USER_DELETED: &str = "user_deleted";
pub const USER_LOGGED_IN: &str = "user_logged_in";
pub const USER_LOGGED_OUT: &str = "user_logged_out";
pub const USER_PASSWORD_CHANGED: &str = "user_password_changed";
```

#### Team Activities
```rust
pub const TEAM_CREATED: &str = "team_created";
pub const TEAM_UPDATED: &str = "team_updated";
pub const TEAM_DELETED: &str = "team_deleted";
pub const USER_ADDED_TO_TEAM: &str = "user_added_to_team";
pub const USER_REMOVED_FROM_TEAM: &str = "user_removed_from_team";
```

#### Client Activities
```rust
pub const CLIENT_CREATED: &str = "client_created";
pub const CLIENT_UPDATED: &str = "client_updated";
pub const CLIENT_DELETED: &str = "client_deleted";
pub const CLIENT_ENABLED: &str = "client_enabled";
pub const CLIENT_DISABLED: &str = "client_disabled";
```

#### Environment Activities
```rust
pub const ENVIRONMENT_CREATED: &str = "environment_created";
pub const ENVIRONMENT_UPDATED: &str = "environment_updated";
pub const ENVIRONMENT_DELETED: &str = "environment_deleted";
```

#### Pipeline Activities
```rust
pub const PIPELINE_CREATED: &str = "pipeline_created";
pub const PIPELINE_UPDATED: &str = "pipeline_updated";
pub const PIPELINE_DELETED: &str = "pipeline_deleted";
pub const STAGE_APPROVED: &str = "stage_approved";
pub const STAGE_REJECTED: &str = "stage_rejected";
```

#### Role Activities
```rust
pub const ROLE_ASSIGNED: &str = "role_assigned";
pub const ROLE_REVOKED: &str = "role_revoked";
```

### Entity Type Constants

```rust
pub const FEATURE: &str = "feature";
pub const USER: &str = "user";
pub const TEAM: &str = "team";
pub const CLIENT: &str = "client";
pub const ENVIRONMENT: &str = "environment";
pub const PIPELINE: &str = "pipeline";
pub const STAGE: &str = "stage";
pub const ROLE: &str = "role";
```

### Convenience Functions

#### log_feature_activity

```rust
pub async fn log_feature_activity(
    repo: &dyn ActivityLogRepository,
    activity_type: &str,
    feature_id: &str,
    feature_name: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    metadata: Option<serde_json::Value>,
) -> Result<ActivityLogRow, sqlx::Error>
```

**Example:**
```rust
use crate::utils::activity_logger::{log_feature_activity, activity_types};

log_feature_activity(
    &*repo,
    activity_types::FEATURE_CREATED,
    &feature.id.to_string(),
    &feature.name,
    Some(user_id),
    Some(user_name),
    Some(serde_json::json!({
        "feature_key": feature.key,
        "enabled": feature.enabled
    }))
).await?;
```

#### log_user_activity

```rust
pub async fn log_user_activity(
    repo: &dyn ActivityLogRepository,
    activity_type: &str,
    user_id: &str,
    username: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    metadata: Option<serde_json::Value>,
) -> Result<ActivityLogRow, sqlx::Error>
```

**Example:**
```rust
use crate::utils::activity_logger::{log_user_activity, activity_types};

log_user_activity(
    &*repo,
    activity_types::USER_LOGGED_IN,
    &user.id.to_string(),
    &user.username,
    Some(user.id),
    Some(user.username.clone()),
    Some(serde_json::json!({
        "ip_address": request_ip,
        "user_agent": user_agent
    }))
).await?;
```

#### log_team_activity

```rust
pub async fn log_team_activity(
    repo: &dyn ActivityLogRepository,
    activity_type: &str,
    team_id: &str,
    team_name: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    metadata: Option<serde_json::Value>,
) -> Result<ActivityLogRow, sqlx::Error>
```

#### log_client_activity

```rust
pub async fn log_client_activity(
    repo: &dyn ActivityLogRepository,
    activity_type: &str,
    client_id: &str,
    client_name: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    metadata: Option<serde_json::Value>,
) -> Result<ActivityLogRow, sqlx::Error>
```

#### log_activity (Generic)

```rust
pub async fn log_activity(
    repo: &dyn ActivityLogRepository,
    activity_type: &str,
    entity_type: &str,
    entity_id: &str,
    description: String,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    metadata: Option<serde_json::Value>,
) -> Result<ActivityLogRow, sqlx::Error>
```

---

## Integration Examples

### Example 1: Log Feature Creation in Mutation

```rust
use crate::utils::activity_logger::{log_feature_activity, activity_types};

#[Object]
impl FeatureMutation {
    async fn create_feature(
        &self,
        ctx: &Context<'_>,
        input: CreateFeatureInput,
    ) -> Result<Feature> {
        let pool = ctx.data::<PgPool>()?;
        let activity_repo = activity_log_repository(pool.clone());
        
        // Create the feature
        let feature = feature_repository(pool.clone())
            .create_feature(input)
            .await?;
        
        // Log the activity
        let session = ctx.data::<SessionGuard>()?;
        log_feature_activity(
            &*activity_repo,
            activity_types::FEATURE_CREATED,
            &feature.id.to_string(),
            &feature.name,
            Some(session.user_id),
            Some(session.username.clone()),
            Some(serde_json::json!({
                "feature_key": feature.key,
                "environment": feature.environment,
                "enabled": feature.enabled
            }))
        ).await?;
        
        Ok(feature.into())
    }
}
```

### Example 2: Log Kill Switch Activation

```rust
use crate::utils::activity_logger::{log_feature_activity, activity_types};

async fn activate_kill_switch(
    pool: &PgPool,
    feature_id: Uuid,
    actor_id: Uuid,
    actor_name: String,
) -> Result<()> {
    let activity_repo = activity_log_repository(pool.clone());
    
    // Disable the feature
    let feature = feature_repository(pool.clone())
        .disable_feature(feature_id)
        .await?;
    
    // Log kill switch activation
    log_feature_activity(
        &*activity_repo,
        activity_types::KILL_SWITCH_ACTIVATED,
        &feature.id.to_string(),
        &feature.name,
        Some(actor_id),
        Some(actor_name),
        Some(serde_json::json!({
            "reason": "Emergency shutdown",
            "previous_state": "enabled"
        }))
    ).await?;
    
    Ok(())
}
```

### Example 3: Log User Login

```rust
use crate::utils::activity_logger::{log_user_activity, activity_types};

async fn handle_login(
    pool: &PgPool,
    user: &User,
    request_info: &RequestInfo,
) -> Result<()> {
    let activity_repo = activity_log_repository(pool.clone());
    
    log_user_activity(
        &*activity_repo,
        activity_types::USER_LOGGED_IN,
        &user.id.to_string(),
        &user.username,
        Some(user.id),
        Some(user.username.clone()),
        Some(serde_json::json!({
            "ip_address": request_info.ip,
            "user_agent": request_info.user_agent,
            "login_time": Utc::now()
        }))
    ).await?;
    
    Ok(())
}
```

### Example 4: Log Pipeline Stage Approval

```rust
use crate::utils::activity_logger::{log_activity, activity_types, entity_types};

async fn approve_stage(
    pool: &PgPool,
    stage_id: Uuid,
    pipeline_name: &str,
    stage_name: &str,
    approver_id: Uuid,
    approver_name: String,
) -> Result<()> {
    let activity_repo = activity_log_repository(pool.clone());
    
    log_activity(
        &*activity_repo,
        activity_types::STAGE_APPROVED,
        entity_types::STAGE,
        &stage_id.to_string(),
        format!("Stage '{}' in pipeline '{}' approved", stage_name, pipeline_name),
        Some(approver_id),
        Some(approver_name),
        Some(serde_json::json!({
            "pipeline_name": pipeline_name,
            "stage_name": stage_name,
            "approval_time": Utc::now()
        }))
    ).await?;
    
    Ok(())
}
```

---

## Testing

### Running the Migration

```bash
cd feature-toggle-backend
sqlx migrate run
```

### Verifying the Table

```sql
-- Check table structure
\d activity_log

-- Check indexes
\di activity_log*

-- Insert test data
INSERT INTO activity_log (activity_type, entity_type, entity_id, description)
VALUES ('test_activity', 'test', '12345', 'Test activity log entry');

-- Query test data
SELECT * FROM activity_log ORDER BY created_at DESC LIMIT 10;
```

### Testing the GraphQL Query

Use a GraphQL client or the GraphQL Playground:

```graphql
query TestRecentActivities {
  recentActivities(pageSize: 5) {
    items {
      id
      activityType
      entityType
      description
      actorName
      createdAt
    }
    total
    pageNumber
    pageSize
  }
}
```

---

## Performance Considerations

### Index Usage

The seven indexes are optimized for common query patterns:

1. **Time-based queries**: `created_at DESC` index
2. **Activity type filtering**: `activity_type` index
3. **Entity timeline**: `(entity_type, entity_id, created_at DESC)` composite
4. **User activity history**: `actor_id` index
5. **Combined filters**: `(activity_type, created_at DESC)` composite

### Query Optimization

- All queries use `ORDER BY created_at DESC` for consistent sorting
- Pagination with `LIMIT` and `OFFSET` prevents large result sets
- Dynamic SQL building only includes filters that are provided
- Foreign key constraint uses `ON DELETE SET NULL` to preserve history

### Recommended Practices

1. **Always use pagination** for UI queries (default: 20 items per page)
2. **Add date range filters** for large datasets
3. **Use entity_id filter** when viewing specific entity history
4. **Include metadata** for audit trail purposes
5. **Consider archiving** old activity logs (e.g., older than 1 year)

---

## Future Enhancements

### Potential Improvements

1. **Middleware Integration**
   - Automatic activity logging middleware for all mutations
   - Request/response logging for audit purposes

2. **Advanced Filtering**
   - Full-text search on description field
   - Metadata field querying (JSONB operators)
   - Activity severity levels

3. **Aggregations**
   - Activity counts by type
   - Activity trends over time
   - Most active users/entities

4. **Real-time Updates**
   - GraphQL subscriptions for live activity feed
   - WebSocket notifications for critical activities

5. **Retention Policies**
   - Automatic archiving of old activities
   - Configurable retention periods by activity type
   - Compressed archive storage

6. **Export Functionality**
   - CSV/JSON export for compliance
   - Activity report generation
   - Scheduled reports via email

---

## Related Files

- **Migration**: `migrations/20251013000000_create_activity_log.sql`
- **Repository**: `src/database/activity_log.rs`
- **Database Module**: `src/database/mod.rs`
- **GraphQL Schema**: `src/graphql/schema.rs`
- **GraphQL Query**: `src/graphql/query.rs`
- **Helper Utilities**: `src/utils/activity_logger.rs`
- **Utils Module**: `src/utils/mod.rs`
- **Library Root**: `src/lib.rs`

---

## Summary

The Activity Log system provides:

✅ **Comprehensive Tracking**: All user actions and system events  
✅ **Flexible Filtering**: By type, entity, user, date range  
✅ **Efficient Queries**: 7 indexes for optimal performance  
✅ **Easy Integration**: Helper functions and constants  
✅ **GraphQL API**: Full query support with pagination  
✅ **Audit Trail**: Immutable history with metadata  
✅ **Production Ready**: Compiled and tested  

The system is ready for integration throughout the application and can be extended with additional activity types and features as needed.
