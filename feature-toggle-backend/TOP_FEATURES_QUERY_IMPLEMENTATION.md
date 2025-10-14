# Top Features Query Implementation

## Overview

This document describes the implementation of the `evaluationsByFeature` GraphQL query, which provides aggregated evaluation data grouped by feature key for dashboard analytics.

## Implementation Details

### Backend Components

#### 1. Database Layer (`feature_evaluation.rs`)

**Struct: `EvaluationByFeature`**
```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EvaluationByFeature {
    pub feature_key: String,
    pub total_evaluations: i64,
    pub successful_evaluations: i64,
    pub cached_evaluations: i64,
    pub unique_users: i64,
    pub last_evaluated_at: chrono::DateTime<chrono::Utc>,
}
```

**Repository Method: `get_evaluations_by_feature`**
- Aggregates evaluation data by `feature_key`
- Groups evaluations using SQL GROUP BY
- Filters by time range (required)
- Optional filters: `environment_id`, `client_id`
- Supports pagination with `limit` and `offset`
- Orders results by `total_evaluations DESC` (most evaluated first)
- Uses PostgreSQL aggregate functions:
  - `COUNT(*)` for total evaluations
  - `COUNT(*) FILTER (WHERE evaluation_result = true)` for successful evaluations
  - `COUNT(*) FILTER (WHERE prior_assignment = true)` for cached evaluations
  - `COUNT(DISTINCT user_context)` for unique users
  - `MAX(evaluated_at)` for last evaluation timestamp

**SQL Query Structure:**
```sql
SELECT 
    feature_key,
    COUNT(*) as total_evaluations,
    COUNT(*) FILTER (WHERE evaluation_result = true) as successful_evaluations,
    COUNT(*) FILTER (WHERE prior_assignment = true) as cached_evaluations,
    COUNT(DISTINCT user_context) FILTER (WHERE user_context IS NOT NULL) as unique_users,
    MAX(evaluated_at) as last_evaluated_at
FROM feature_evaluations 
WHERE evaluated_at >= $1 AND evaluated_at <= $2
    [AND environment_id = $n]
    [AND client_id = $n]
GROUP BY feature_key 
ORDER BY total_evaluations DESC
[LIMIT $n]
[OFFSET $n]
```

#### 2. GraphQL Schema (`schema.rs`)

**Type: `EvaluationByFeature`**
```rust
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct EvaluationByFeature {
    pub feature_key: String,
    pub total_evaluations: i64,
    pub successful_evaluations: i64,
    pub cached_evaluations: i64,
    pub unique_users: i64,
    pub last_evaluated_at: chrono::DateTime<chrono::Utc>,
}
```

#### 3. GraphQL Query (`query.rs`)

**Query Method: `evaluations_by_feature`**
```rust
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

**Parameters:**
- `from_time` (required): Start time for evaluation data (DateTime<Utc>)
- `to_time` (required): End time for evaluation data (DateTime<Utc>)
- `environment_id` (optional): Filter by environment ID (String)
- `client_id` (optional): Filter by client ID (GraphQL ID, converted to UUID)
- `limit` (optional): Maximum number of results to return (i32)
- `offset` (optional): Number of results to skip for pagination (i32)

**Returns:** `Vec<EvaluationByFeature>`

## GraphQL Usage

### Basic Query

Get top 10 features evaluated in the last 24 hours:

```graphql
query GetTopFeatures {
  evaluationsByFeature(
    fromTime: "2024-10-12T00:00:00Z"
    toTime: "2024-10-13T00:00:00Z"
    limit: 10
  ) {
    featureKey
    totalEvaluations
    successfulEvaluations
    cachedEvaluations
    uniqueUsers
    lastEvaluatedAt
  }
}
```

### Filtered by Environment

Get top features for production environment:

```graphql
query GetTopFeaturesInProduction {
  evaluationsByFeature(
    fromTime: "2024-10-01T00:00:00Z"
    toTime: "2024-10-13T00:00:00Z"
    environmentId: "prod"
    limit: 10
  ) {
    featureKey
    totalEvaluations
    successfulEvaluations
    cachedEvaluations
    uniqueUsers
    lastEvaluatedAt
  }
}
```

### Filtered by Client

Get top features for a specific client:

```graphql
query GetTopFeaturesForClient {
  evaluationsByFeature(
    fromTime: "2024-10-01T00:00:00Z"
    toTime: "2024-10-13T00:00:00Z"
    clientId: "550e8400-e29b-41d4-a716-446655440000"
    limit: 10
  ) {
    featureKey
    totalEvaluations
    successfulEvaluations
    cachedEvaluations
    uniqueUsers
    lastEvaluatedAt
  }
}
```

### Paginated Results

Get features with pagination (25 per page, skip first 25):

```graphql
query GetTopFeaturesPaginated {
  evaluationsByFeature(
    fromTime: "2024-10-01T00:00:00Z"
    toTime: "2024-10-13T00:00:00Z"
    limit: 25
    offset: 25
  ) {
    featureKey
    totalEvaluations
    successfulEvaluations
    cachedEvaluations
    uniqueUsers
    lastEvaluatedAt
  }
}
```

### Complete Example with All Filters

```graphql
query GetTopFeaturesComplete {
  evaluationsByFeature(
    fromTime: "2024-10-01T00:00:00Z"
    toTime: "2024-10-13T00:00:00Z"
    environmentId: "prod"
    clientId: "550e8400-e29b-41d4-a716-446655440000"
    limit: 25
    offset: 0
  ) {
    featureKey
    totalEvaluations
    successfulEvaluations
    cachedEvaluations
    uniqueUsers
    lastEvaluatedAt
  }
}
```

## Response Format

```json
{
  "data": {
    "evaluationsByFeature": [
      {
        "featureKey": "new-checkout-flow",
        "totalEvaluations": 15420,
        "successfulEvaluations": 12336,
        "cachedEvaluations": 7710,
        "uniqueUsers": 3205,
        "lastEvaluatedAt": "2024-10-13T15:30:42.123Z"
      },
      {
        "featureKey": "dark-mode",
        "totalEvaluations": 12890,
        "successfulEvaluations": 11601,
        "cachedEvaluations": 6445,
        "uniqueUsers": 2890,
        "lastEvaluatedAt": "2024-10-13T15:29:15.456Z"
      },
      {
        "featureKey": "recommendation-engine",
        "totalEvaluations": 9876,
        "successfulEvaluations": 4938,
        "cachedEvaluations": 4938,
        "uniqueUsers": 1234,
        "lastEvaluatedAt": "2024-10-13T15:28:03.789Z"
      }
    ]
  }
}
```

## Calculated Metrics

While the query returns raw counts, you can calculate these metrics in the frontend:

- **Success Rate**: `(successfulEvaluations / totalEvaluations) * 100`
- **Cache Hit Rate**: `(cachedEvaluations / totalEvaluations) * 100`
- **Average Users per Feature**: `uniqueUsers / number_of_features`

## Frontend Integration (React/TypeScript)

### GraphQL Query File

Create `src/graphql/queries/topFeatures.graphql`:

```graphql
query GetTopFeatures(
  $fromTime: DateTime!
  $toTime: DateTime!
  $environmentId: String
  $clientId: ID
  $limit: Int
  $offset: Int
) {
  evaluationsByFeature(
    fromTime: $fromTime
    toTime: $toTime
    environmentId: $environmentId
    clientId: $clientId
    limit: $limit
    offset: $offset
  ) {
    featureKey
    totalEvaluations
    successfulEvaluations
    cachedEvaluations
    uniqueUsers
    lastEvaluatedAt
  }
}
```

### TypeScript Hook

Create `src/hooks/useTopFeatures.ts`:

```typescript
import { useQuery } from '@apollo/client';
import { GET_TOP_FEATURES } from '@/graphql/queries/topFeatures';
import { useDashboard } from '@/contexts/DashboardContext';

interface TopFeaturesVariables {
  fromTime: string;
  toTime: string;
  environmentId?: string;
  clientId?: string;
  limit?: number;
  offset?: number;
}

export const useTopFeatures = () => {
  const { filters } = useDashboard();

  const variables: TopFeaturesVariables = {
    fromTime: filters.startDate?.toISOString() || new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString(),
    toTime: filters.endDate?.toISOString() || new Date().toISOString(),
    environmentId: filters.environmentId || undefined,
    clientId: filters.clientId || undefined,
    limit: 10,
    offset: 0,
  };

  const { data, loading, error, refetch } = useQuery(GET_TOP_FEATURES, {
    variables,
    pollInterval: 30000, // Refresh every 30 seconds
  });

  return {
    features: data?.evaluationsByFeature || [],
    loading,
    error,
    refetch,
  };
};
```

### React Component

Create `src/components/Dashboard/TopFeaturesTable.tsx`:

```typescript
import { useTopFeatures } from '@/hooks/useTopFeatures';

interface Feature {
  featureKey: string;
  totalEvaluations: number;
  successfulEvaluations: number;
  cachedEvaluations: number;
  uniqueUsers: number;
  lastEvaluatedAt: string;
}

export const TopFeaturesTable: React.FC = () => {
  const { features, loading, error } = useTopFeatures();

  const calculateSuccessRate = (feature: Feature) => {
    return ((feature.successfulEvaluations / feature.totalEvaluations) * 100).toFixed(1);
  };

  const calculateCacheHitRate = (feature: Feature) => {
    return ((feature.cachedEvaluations / feature.totalEvaluations) * 100).toFixed(1);
  };

  if (loading) return <div>Loading...</div>;
  if (error) return <div>Error: {error.message}</div>;

  return (
    <table>
      <thead>
        <tr>
          <th>Feature Key</th>
          <th>Total Evaluations</th>
          <th>Success Rate</th>
          <th>Cache Hit Rate</th>
          <th>Unique Users</th>
          <th>Last Evaluated</th>
        </tr>
      </thead>
      <tbody>
        {features.map((feature: Feature) => (
          <tr key={feature.featureKey}>
            <td>{feature.featureKey}</td>
            <td>{feature.totalEvaluations.toLocaleString()}</td>
            <td>{calculateSuccessRate(feature)}%</td>
            <td>{calculateCacheHitRate(feature)}%</td>
            <td>{feature.uniqueUsers.toLocaleString()}</td>
            <td>{new Date(feature.lastEvaluatedAt).toLocaleString()}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
};
```

## Performance Considerations

1. **Indexing**: Ensure indexes exist on:
   - `feature_evaluations(feature_key, evaluated_at)`
   - `feature_evaluations(environment_id, evaluated_at)`
   - `feature_evaluations(client_id, evaluated_at)`

2. **Query Optimization**:
   - Always specify reasonable time ranges (avoid open-ended queries)
   - Use pagination for large result sets
   - Consider using `limit` to restrict result size

3. **Caching**:
   - Results are relatively stable over short periods
   - Consider caching in frontend for 30-60 seconds
   - Use GraphQL query deduplication

4. **Database Load**:
   - Query performs aggregations which can be CPU-intensive
   - For large datasets, consider creating materialized views
   - Monitor query performance and add indexes as needed

## Testing

### Manual Testing with GraphQL Playground

1. Start the backend server
2. Navigate to GraphQL playground (usually `http://localhost:8080/graphql`)
3. Run the query:

```graphql
query TestTopFeatures {
  evaluationsByFeature(
    fromTime: "2024-10-12T00:00:00Z"
    toTime: "2024-10-13T00:00:00Z"
    limit: 5
  ) {
    featureKey
    totalEvaluations
    successfulEvaluations
    cachedEvaluations
    uniqueUsers
    lastEvaluatedAt
  }
}
```

### Unit Testing

Consider adding integration tests in `feature-toggle-backend/tests/`:

```rust
#[tokio::test]
async fn test_get_evaluations_by_feature() {
    let pool = setup_test_db().await;
    let repo = PgFeatureEvaluationRepository::new(pool);

    // Create test data
    // ...

    let from_time = Utc::now() - Duration::hours(24);
    let to_time = Utc::now();
    
    let results = repo
        .get_evaluations_by_feature(from_time, to_time, None, None, Some(10), None)
        .await
        .unwrap();

    assert!(!results.is_empty());
    assert_eq!(results[0].feature_key, "expected-feature");
}
```

## Error Handling

The query handles these error cases:

1. **Invalid Client ID**: Returns GraphQL error if client_id is not a valid UUID
2. **Database Errors**: Returns GraphQL error with database error message
3. **Invalid Date Range**: SQL will handle invalid ranges gracefully
4. **Missing Required Parameters**: GraphQL type system enforces required parameters

## Related Documentation

- [Dashboard Implementation Tasks](../../../../DASHBOARD_TASKS.md) - Backend Task 1
- [Feature Evaluation Analytics Dashboard](../../../../DASHBOARD_TASKS.md#phase-1-feature-evaluation-analytics-dashboard) - Task 1.7
- [GraphQL Subscriptions](../../DASHBOARD_IMPLEMENTATION.md) - Real-time subscription implementation

## Files Modified

1. `/feature-toggle/feature-toggle-backend/src/database/feature_evaluation.rs`
   - Added `EvaluationByFeature` struct
   - Added `get_evaluations_by_feature()` method to trait
   - Implemented query in `PgFeatureEvaluationRepository`

2. `/feature-toggle/feature-toggle-backend/src/graphql/schema.rs`
   - Added `EvaluationByFeature` GraphQL type

3. `/feature-toggle/feature-toggle-backend/src/graphql/query.rs`
   - Added `evaluations_by_feature()` query method
   - Added imports for `EvaluationByFeature` and `chrono` types

## Status

✅ **COMPLETED** - Backend implementation ready for frontend integration

## Next Steps

1. Create frontend GraphQL query file
2. Implement `useTopFeatures` hook
3. Create `TopFeaturesTable` component
4. Integrate with Dashboard State Management (Task INF-5)
5. Add to Evaluation Analytics Dashboard page (Task 1.7)
