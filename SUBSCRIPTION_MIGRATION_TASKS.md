# Backend Subscription Migration Tasks

## Overview
Currently, several dashboard analytics queries are implemented as GraphQL **queries** in `query.rs` instead of **subscriptions** in `subscription.rs`. For real-time dashboard updates, these should be migrated to subscriptions under `FeatureEvaluationSubscription`.

## Current State

### ✅ Already Subscriptions (in subscription.rs)
1. **evaluationRates** - Time-series evaluation metrics
2. **evaluationSummary** - Aggregated KPI statistics  
3. **evaluationDashboard** - Combined rates + summary

### ❌ Currently Queries (in query.rs) - Need Migration
1. **evaluations_by_feature** - Top features by evaluation count
2. **recent_activities** - Activity log feed
3. **feature_growth** - Feature creation time-series

---

## Migration Tasks

### Task SUB-MIGRATE-1: Convert `evaluations_by_feature` to Subscription
**Priority:** High  
**Estimated Time:** 2-3 hours  
**Status:** ❌ Not Started

#### Current Implementation (Query)
```rust
// In query.rs
async fn evaluations_by_feature(
    &self,
    ctx: &Context<'_>,
    from_time: DateTime<Utc>,
    to_time: DateTime<Utc>,
    environment_id: Option<String>,
    client_id: Option<ID>,
    limit: Option<i32>,
    offset: Option<i32>,
) -> GqlResult<Vec<EvaluationByFeature>>
```

#### Target Implementation (Subscription)
```rust
// In subscription.rs under FeatureEvaluationSubscription

#[derive(InputObject, Clone)]
pub struct TopFeaturesInput {
    /// Optional environment ID to filter by
    pub environment_id: Option<String>,
    /// Optional client ID to filter by
    pub client_id: Option<String>,
    /// Duration in hours to look back from current time (max 24 hours)
    pub duration_hours: i32,
    /// Maximum number of features to return
    pub limit: Option<i32>,
}

/// Subscribe to real-time top features by evaluation count
/// Updates every 30 seconds with the latest top performing features
async fn top_features(
    &self,
    ctx: &Context<'_>,
    input: TopFeaturesInput,
) -> impl Stream<Item = GqlResult<Vec<GqlEvaluationByFeature>>>
```

#### Implementation Steps
1. **Add Input Type** to `subscription.rs`:
   - Create `TopFeaturesInput` with validation
   - Duration hours (1-24), limit (1-100)

2. **Add Output Type** to `subscription.rs`:
   - Create `GqlEvaluationByFeature` (may already exist in schema.rs)
   - Fields: feature_key, total_evaluations, successful_evaluations, cached_evaluations, unique_users, last_evaluated_at

3. **Implement Subscription Method**:
   - Add validation for duration_hours and limit
   - Use IntervalStream with 30-second interval
   - Calculate from_time/to_time dynamically (rolling window)
   - Call repository method: `get_evaluations_by_feature`
   - Map results to GraphQL types

4. **Update Repository/Logic Layer** (if needed):
   - Ensure `FeatureEvaluationLogic` trait has the method
   - Implement in `FeatureEvaluationLogicImpl`

5. **Testing**:
   - Unit tests for input validation
   - Integration test for subscription streaming
   - Test with GraphQL Playground

6. **Keep Query for Backward Compatibility** (optional):
   - Can keep the query version initially
   - Mark as deprecated in documentation
   - Remove after frontend migration

#### Frontend Integration Example
```typescript
// Subscribe to top features
const TOP_FEATURES_SUBSCRIPTION = gql`
  subscription TopFeatures($input: TopFeaturesInput!) {
    topFeatures(input: $input) {
      featureKey
      totalEvaluations
      successfulEvaluations
      cachedEvaluations
      uniqueUsers
      lastEvaluatedAt
    }
  }
`;

// Usage in React component
const { data, loading } = useSubscription(TOP_FEATURES_SUBSCRIPTION, {
  variables: {
    input: {
      duration_hours: 24,
      limit: 10
    }
  }
});
```

---

### Task SUB-MIGRATE-2: Convert `recent_activities` to Subscription
**Priority:** High  
**Estimated Time:** 2-3 hours  
**Status:** ❌ Not Started

#### Current Implementation (Query)
```rust
// In query.rs
async fn recent_activities(
    &self,
    ctx: &Context<'_>,
    activity_type: Option<String>,
    entity_type: Option<String>,
    entity_id: Option<String>,
    actor_id: Option<ID>,
    from_date: Option<DateTime<Utc>>,
    to_date: Option<DateTime<Utc>>,
    page_number: Option<i32>,
    page_size: Option<i32>,
) -> GqlResult<ActivityLogPage>
```

#### Target Implementation (Subscription)
```rust
// In subscription.rs under FeatureEvaluationSubscription

#[derive(InputObject, Clone)]
pub struct ActivityFeedInput {
    /// Optional activity type filter (e.g., 'feature_created')
    pub activity_type: Option<String>,
    /// Optional entity type filter (e.g., 'feature', 'user')
    pub entity_type: Option<String>,
    /// Optional entity ID to filter by
    pub entity_id: Option<String>,
    /// Optional actor (user) ID to filter by
    pub actor_id: Option<String>,
    /// Duration in hours to look back (default: 24, max: 168)
    pub duration_hours: Option<i32>,
    /// Maximum number of activities to return (default: 20, max: 100)
    pub limit: Option<i32>,
}

/// Subscribe to real-time activity feed
/// Updates every 30 seconds with new activities
async fn activity_feed(
    &self,
    ctx: &Context<'_>,
    input: ActivityFeedInput,
) -> impl Stream<Item = GqlResult<Vec<GqlActivityLog>>>
```

#### Implementation Steps
1. **Add Input Type** to `subscription.rs`:
   - Create `ActivityFeedInput` with filters
   - Default duration: 24 hours, max: 168 hours (7 days)
   - Default limit: 20, max: 100

2. **Add/Update Output Type**:
   - Use existing `ActivityLog` from schema.rs as `GqlActivityLog`
   - Or create new subscription-specific type

3. **Implement Subscription Method**:
   - Add validation for duration and limit
   - Use IntervalStream with 30-second interval
   - Calculate from_date/to_date dynamically (rolling window)
   - Build `ActivityLogFilter` with parameters
   - Call repository method: `get_activities_paginated`
   - Map results to GraphQL types (drop pagination metadata)

4. **Update Repository Layer** (if needed):
   - Ensure `ActivityLogRepository` is available in subscription context
   - May need to add to dependency injection

5. **Testing**:
   - Test filtering by activity_type, entity_type, entity_id
   - Test actor_id UUID parsing
   - Test duration and limit validation
   - Integration test for real-time updates

6. **Frontend Integration Example**:
```typescript
const ACTIVITY_FEED_SUBSCRIPTION = gql`
  subscription ActivityFeed($input: ActivityFeedInput!) {
    activityFeed(input: $input) {
      id
      activityType
      entityType
      entityId
      actorName
      description
      createdAt
    }
  }
`;
```

---

### Task SUB-MIGRATE-3: Convert `feature_growth` to Subscription
**Priority:** High  
**Estimated Time:** 2-3 hours  
**Status:** ❌ Not Started

#### Current Implementation (Query)
```rust
// In query.rs
async fn feature_growth(
    &self,
    ctx: &Context<'_>,
    from_time: DateTime<Utc>,
    to_time: DateTime<Utc>,
    interval: String,
    team_id: Option<ID>,
) -> GqlResult<Vec<FeatureGrowthPoint>>
```

#### Target Implementation (Subscription)
```rust
// In subscription.rs under FeatureEvaluationSubscription

#[derive(InputObject, Clone)]
pub struct FeatureGrowthInput {
    /// Time interval: 'day', 'week', or 'month'
    pub interval: String,
    /// Duration in days to look back (max 365)
    pub duration_days: i32,
    /// Optional team ID to filter by
    pub team_id: Option<String>,
}

/// Subscribe to real-time feature growth time-series
/// Updates every 60 seconds with feature creation trends
async fn feature_growth(
    &self,
    ctx: &Context<'_>,
    input: FeatureGrowthInput,
) -> impl Stream<Item = GqlResult<Vec<GqlFeatureGrowthPoint>>>
```

#### Implementation Steps
1. **Add Input Type** to `subscription.rs`:
   - Create `FeatureGrowthInput`
   - Validate interval: "day", "week", "month"
   - Validate duration_days: 1-365

2. **Add/Update Output Type**:
   - Use existing `FeatureGrowthPoint` from schema.rs as `GqlFeatureGrowthPoint`

3. **Implement Subscription Method**:
   - Add validation for interval and duration
   - Use IntervalStream with **60-second** interval (slower update for time-series)
   - Calculate from_time/to_time dynamically based on duration_days
   - Parse team_id to UUID if provided
   - Call repository method: `get_feature_growth`
   - Map results to GraphQL types

4. **Update Repository Access** (if needed):
   - Ensure `FeatureRepository` is available in subscription context
   - Add to dependency injection if needed

5. **Testing**:
   - Test interval validation ("day", "week", "month")
   - Test duration_days validation (1-365)
   - Test team_id UUID parsing
   - Test rolling window calculation
   - Integration test for subscription streaming

6. **Frontend Integration Example**:
```typescript
const FEATURE_GROWTH_SUBSCRIPTION = gql`
  subscription FeatureGrowth($input: FeatureGrowthInput!) {
    featureGrowth(input: $input) {
      timeBucket
      teamId
      teamName
      featureCount
      cumulativeCount
    }
  }
`;

// Usage with Recharts
const { data } = useSubscription(FEATURE_GROWTH_SUBSCRIPTION, {
  variables: {
    input: {
      interval: 'day',
      duration_days: 30
    }
  }
});

<LineChart data={data?.featureGrowth || []}>
  <Line dataKey="cumulativeCount" stroke="#8884d8" />
</LineChart>
```

---

## Architectural Considerations

### 1. Subscription Update Intervals
- **evaluationRates, evaluationSummary, evaluationDashboard**: 30 seconds (real-time critical)
- **top_features**: 30 seconds (dashboard KPI card)
- **activity_feed**: 30 seconds (live feed)
- **feature_growth**: 60 seconds (time-series, less critical for real-time)

### 2. Rolling Time Windows
All subscriptions should use **rolling time windows** instead of fixed from/to times:
```rust
// Calculate rolling window
let now = Utc::now();
let from_time = now - chrono::Duration::hours(input.duration_hours as i64);
```

This ensures:
- Always showing "last 24 hours" or "last 30 days"
- No need to manually update time ranges
- Automatic time progression

### 3. Dependency Injection
Subscriptions need access to multiple repositories/logic layers:
```rust
// Existing (in FeatureEvaluationSubscription)
let logic = ctx.data::<Box<dyn FeatureEvaluationLogic>>()?;

// New (needs to be added)
let feature_repo = ctx.data::<Box<dyn FeatureRepository>>()?;
let activity_repo = ctx.data::<Box<dyn ActivityLogRepository>>()?;
```

**Action Required**: Update `main.rs` or `lib.rs` to register these in GraphQL context:
```rust
Schema::build(Query, MutationRoot, SubscriptionRoot)
    .data::<Box<dyn FeatureEvaluationLogic>>(Box::new(feature_eval_logic))
    .data::<Box<dyn FeatureRepository>>(Box::new(feature_repo))
    .data::<Box<dyn ActivityLogRepository>>(Box::new(activity_repo))
    .finish()
```

### 4. Error Handling
Follow existing subscription pattern:
```rust
// Early validation
if input.duration_hours < 1 || input.duration_hours > 24 {
    return Box::pin(futures_util::stream::once(async {
        Err("Duration must be between 1 and 24 hours".into())
    })) as std::pin::Pin<Box<dyn Stream<Item = GqlResult<_>> + Send>>;
}

// Graceful error streaming
match repo.get_data(...).await {
    Ok(data) => Ok(data),
    Err(e) => Err(format!("Failed to get data: {}", e).into()),
}
```

### 5. Query Deprecation Strategy
**Option A: Keep Both (Recommended)**
- Keep queries for backward compatibility
- Add subscriptions for new features
- Document migration path for frontend

**Option B: Deprecate Queries**
- Mark queries with `#[graphql(deprecation = "Use subscription instead")]`
- Set removal timeline (e.g., 6 months)
- Update all frontend code first

**Option C: Remove Queries**
- Only if frontend has been fully migrated
- Coordinate with frontend team
- Update all documentation

**Recommendation**: Use Option A initially, then Option B after frontend migration.

---

## Testing Strategy

### 1. Unit Tests
For each subscription, add tests in `subscription.rs`:
```rust
#[tokio::test]
async fn test_top_features_input_validation() {
    let invalid_duration = TopFeaturesInput {
        environment_id: None,
        client_id: None,
        duration_hours: 0, // Invalid
        limit: Some(10),
    };
    assert!(invalid_duration.duration_hours < 1);
}

#[tokio::test]
async fn test_activity_feed_limit_validation() {
    let invalid_limit = ActivityFeedInput {
        activity_type: None,
        entity_type: None,
        entity_id: None,
        actor_id: None,
        duration_hours: Some(24),
        limit: Some(101), // Invalid: exceeds max
    };
    assert!(invalid_limit.limit.unwrap() > 100);
}
```

### 2. Integration Tests
Test subscription streaming with actual data:
```rust
#[tokio::test]
async fn test_top_features_subscription_streams() {
    // Setup schema with mock repositories
    let schema = build_test_schema();
    
    // Subscribe
    let stream = schema
        .execute_stream(
            Request::new(
                r#"subscription { 
                    topFeatures(input: { durationHours: 1 }) { 
                        featureKey 
                    } 
                }"#
            )
        );
    
    // Collect first 3 emissions
    let results: Vec<_> = stream.take(3).collect().await;
    assert_eq!(results.len(), 3);
}
```

### 3. Manual Testing with GraphQL Playground
```graphql
# Test 1: Top Features Subscription
subscription {
  topFeatures(input: {
    durationHours: 24
    limit: 10
  }) {
    featureKey
    totalEvaluations
    successfulEvaluations
  }
}

# Test 2: Activity Feed Subscription
subscription {
  activityFeed(input: {
    activityType: "feature_created"
    durationHours: 24
    limit: 20
  }) {
    activityType
    description
    actorName
    createdAt
  }
}

# Test 3: Feature Growth Subscription
subscription {
  featureGrowth(input: {
    interval: "day"
    durationDays: 30
  }) {
    timeBucket
    featureCount
    cumulativeCount
  }
}
```

---

## Implementation Order

### Phase 1: Core Dashboard Subscriptions (Week 1)
1. **SUB-MIGRATE-1**: `top_features` subscription (2-3 hours)
   - Most critical for dashboard KPI card
   - Replaces `evaluations_by_feature` query

2. **SUB-MIGRATE-3**: `feature_growth` subscription (2-3 hours)
   - Needed for feature growth chart
   - Replaces `feature_growth` query

### Phase 2: Activity Feed (Week 2)
3. **SUB-MIGRATE-2**: `activity_feed` subscription (2-3 hours)
   - Replaces `recent_activities` query
   - Enhances user experience with live updates

### Phase 3: Testing & Documentation (Week 2)
4. Comprehensive integration testing (2-3 hours)
5. Update API documentation (1-2 hours)
6. Frontend migration guide (1-2 hours)

**Total Estimated Time**: 10-14 hours

---

## Migration Checklist

### Backend Implementation
- [ ] SUB-MIGRATE-1: Implement `top_features` subscription
  - [ ] Add `TopFeaturesInput` type
  - [ ] Add `GqlEvaluationByFeature` type (if needed)
  - [ ] Implement subscription method
  - [ ] Add unit tests
  - [ ] Add integration tests
  
- [ ] SUB-MIGRATE-2: Implement `activity_feed` subscription
  - [ ] Add `ActivityFeedInput` type
  - [ ] Add/update `GqlActivityLog` type
  - [ ] Implement subscription method
  - [ ] Register `ActivityLogRepository` in GraphQL context
  - [ ] Add unit tests
  - [ ] Add integration tests

- [ ] SUB-MIGRATE-3: Implement `feature_growth` subscription
  - [ ] Add `FeatureGrowthInput` type
  - [ ] Add/update `GqlFeatureGrowthPoint` type
  - [ ] Implement subscription method
  - [ ] Register `FeatureRepository` in GraphQL context
  - [ ] Add unit tests
  - [ ] Add integration tests

### Dependency Injection
- [ ] Update GraphQL schema builder to include new repositories
- [ ] Verify all subscriptions have access to required dependencies
- [ ] Test context data availability

### Testing
- [ ] All unit tests pass
- [ ] All integration tests pass
- [ ] Manual testing in GraphQL Playground
- [ ] Load testing for concurrent subscriptions (optional)

### Documentation
- [ ] Update API documentation with new subscriptions
- [ ] Create frontend migration guide
- [ ] Update DASHBOARD_IMPLEMENTATION.md
- [ ] Update BACKEND_GRAPHQL_SUBSCRIPTIONS_ANALYSIS.md

### Frontend Migration (Separate Tasks)
- [ ] Replace `evaluations_by_feature` query with `top_features` subscription
- [ ] Replace `feature_growth` query with `feature_growth` subscription
- [ ] Replace `recent_activities` query with `activity_feed` subscription
- [ ] Test real-time updates in dashboard
- [ ] Performance testing with subscriptions

### Cleanup (Optional)
- [ ] Mark old queries as deprecated
- [ ] Set removal timeline
- [ ] Monitor usage of deprecated queries
- [ ] Remove deprecated queries after migration period

---

## Success Criteria

### Functional Requirements
- ✅ All 3 subscriptions implemented and tested
- ✅ Subscriptions update every 30-60 seconds
- ✅ Rolling time windows work correctly
- ✅ All input validation works
- ✅ Error handling is graceful

### Non-Functional Requirements
- ✅ Subscriptions don't cause memory leaks
- ✅ CPU usage is acceptable with multiple concurrent subscriptions
- ✅ Database queries are optimized (use existing indexes)
- ✅ WebSocket connections are stable

### Code Quality
- ✅ All tests pass
- ✅ Code follows existing patterns in `subscription.rs`
- ✅ Documentation is comprehensive
- ✅ No compiler warnings

---

## Related Documentation
- [BACKEND_GRAPHQL_SUBSCRIPTIONS_ANALYSIS.md](./BACKEND_GRAPHQL_SUBSCRIPTIONS_ANALYSIS.md) - Current subscription analysis
- [BACKEND_SUBSCRIPTION_TASKS.md](./BACKEND_SUBSCRIPTION_TASKS.md) - Optional enhancement tasks
- [DASHBOARD_IMPLEMENTATION.md](./feature-toggle/DASHBOARD_IMPLEMENTATION.md) - Frontend dashboard implementation
- [DASHBOARD_TASKS.md](./feature-toggle/DASHBOARD_TASKS.md) - Dashboard task tracking
- [FEATURE_GROWTH_QUERY_IMPLEMENTATION.md](./feature-toggle/FEATURE_GROWTH_QUERY_IMPLEMENTATION.md) - Feature growth query details

---

## Notes
- This migration ensures ALL dashboard data sources are available as real-time subscriptions
- Frontend can choose to use subscriptions (real-time) or queries (on-demand) based on use case
- Subscriptions provide better user experience but may increase server load
- Consider implementing connection pooling and rate limiting for production
