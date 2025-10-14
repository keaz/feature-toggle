# Feature Growth Query Implementation

## Overview

This document describes the implementation of the Feature Growth Query for tracking feature creation trends over time with optional team breakdown. This query powers the Feature Growth Chart in the System Overview Dashboard.

## Implementation Date
- **Created:** January 2025
- **Status:** Complete and Production-Ready

---

## Architecture

The Feature Growth Query consists of three layers:

1. **Database Layer** (`src/database/feature.rs`)
   - Time-bucketed aggregation using PostgreSQL's `date_trunc`
   - Cumulative count calculation with window functions
   - Optional team filtering

2. **GraphQL API Layer** (`src/graphql/schema.rs` & `src/graphql/query.rs`)
   - `FeatureGrowthPoint` type for time-series data
   - `featureGrowth` query with configurable intervals
   - Input validation and error handling

3. **Business Logic**
   - Interval validation (day, week, month)
   - Team-based filtering
   - Cumulative counting per team

---

## Database Layer

### Struct: FeatureGrowthPoint

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FeatureGrowthPoint {
    pub time_bucket: DateTime<Utc>,
    pub team_id: Option<Uuid>,
    pub team_name: Option<String>,
    pub feature_count: i64,
    pub cumulative_count: i64,
}
```

**Fields:**
- `time_bucket`: The time period for this data point (day, week, or month boundary)
- `team_id`: Team UUID (nullable for all-teams view)
- `team_name`: Team name for display purposes
- `feature_count`: Number of features created in this time bucket
- `cumulative_count`: Total features created up to and including this time bucket

### Repository Method

```rust
async fn get_feature_growth(
    &self,
    from_time: DateTime<Utc>,
    to_time: DateTime<Utc>,
    interval: String,
    team_id: Option<Uuid>,
) -> Result<Vec<FeatureGrowthPoint>, Error>
```

**Parameters:**
- `from_time`: Start of the time range
- `to_time`: End of the time range
- `interval`: Time bucket size - must be `"day"`, `"week"`, or `"month"`
- `team_id`: Optional team filter

**SQL Query Logic:**

The query uses Common Table Expressions (CTEs) to:

1. **Time Series CTE**: Groups features by time bucket and team
   - Uses `date_trunc(interval, created_at)` to bucket timestamps
   - Filters by date range and optional team
   - Counts features per bucket

2. **Cumulative CTE**: Calculates running totals
   - Uses window function `SUM(feature_count) OVER (PARTITION BY team_id ORDER BY time_bucket)`
   - Maintains separate cumulative counts per team

3. **Final SELECT**: Joins with teams table
   - Adds team names for display
   - Orders by time_bucket and team_id

**Example SQL (simplified):**

```sql
WITH time_series AS (
    SELECT 
        date_trunc('day', created_at) as time_bucket,
        team_id,
        COUNT(*) as feature_count
    FROM features
    WHERE created_at >= $1 AND created_at <= $2
    GROUP BY time_bucket, team_id
),
cumulative AS (
    SELECT 
        time_bucket,
        team_id,
        feature_count,
        SUM(feature_count) OVER (PARTITION BY team_id ORDER BY time_bucket) as cumulative_count
    FROM time_series
)
SELECT 
    c.time_bucket,
    c.team_id,
    t.name as team_name,
    c.feature_count,
    c.cumulative_count
FROM cumulative c
LEFT JOIN teams t ON c.team_id = t.id
ORDER BY c.time_bucket, c.team_id
```

---

## GraphQL API

### Type: FeatureGrowthPoint

```graphql
type FeatureGrowthPoint {
  """Time bucket for this data point (day, week, or month)"""
  timeBucket: DateTime!
  
  """Team ID (null if aggregated across all teams)"""
  teamId: ID
  
  """Team name for display purposes"""
  teamName: String
  
  """Number of features created in this time bucket"""
  featureCount: Int!
  
  """Cumulative count of features up to and including this time bucket"""
  cumulativeCount: Int!
}
```

### Query: featureGrowth

```graphql
type Query {
  featureGrowth(
    """Start time for feature growth data"""
    fromTime: DateTime!
    
    """End time for feature growth data"""
    toTime: DateTime!
    
    """Time interval: 'day', 'week', or 'month'"""
    interval: String!
    
    """Filter by team ID (optional)"""
    teamId: ID
  ): [FeatureGrowthPoint!]!
}
```

**Validation:**
- `interval` must be one of: `"day"`, `"week"`, `"month"`
- `fromTime` must be before `toTime`
- `teamId` must be a valid UUID if provided

---

## Usage Examples

### Example 1: Daily Feature Growth (Last 90 Days)

```graphql
query {
  featureGrowth(
    fromTime: "2025-10-15T00:00:00Z"
    toTime: "2025-01-13T23:59:59Z"
    interval: "day"
  ) {
    timeBucket
    teamId
    teamName
    featureCount
    cumulativeCount
  }
}
```

**Response:**
```json
{
  "data": {
    "featureGrowth": [
      {
        "timeBucket": "2024-10-15T00:00:00Z",
        "teamId": "123e4567-e89b-12d3-a456-426614174000",
        "teamName": "Platform Team",
        "featureCount": 3,
        "cumulativeCount": 3
      },
      {
        "timeBucket": "2024-10-16T00:00:00Z",
        "teamId": "123e4567-e89b-12d3-a456-426614174000",
        "teamName": "Platform Team",
        "featureCount": 2,
        "cumulativeCount": 5
      },
      {
        "timeBucket": "2024-10-15T00:00:00Z",
        "teamId": "223e4567-e89b-12d3-a456-426614174001",
        "teamName": "Mobile Team",
        "featureCount": 1,
        "cumulativeCount": 1
      }
    ]
  }
}
```

### Example 2: Weekly Growth for Specific Team

```graphql
query {
  featureGrowth(
    fromTime: "2024-01-01T00:00:00Z"
    toTime: "2025-01-13T23:59:59Z"
    interval: "week"
    teamId: "123e4567-e89b-12d3-a456-426614174000"
  ) {
    timeBucket
    teamName
    featureCount
    cumulativeCount
  }
}
```

**Use Case:** Track a specific team's feature creation velocity over the past year.

### Example 3: Monthly Growth (All Teams)

```graphql
query {
  featureGrowth(
    fromTime: "2024-01-01T00:00:00Z"
    toTime: "2024-12-31T23:59:59Z"
    interval: "month"
  ) {
    timeBucket
    teamId
    teamName
    featureCount
    cumulativeCount
  }
}
```

**Use Case:** Executive dashboard showing annual feature growth trends across all teams.

---

## Frontend Integration

### Using with Recharts

```typescript
import { useQuery } from '@apollo/client';
import { LineChart, Line, XAxis, YAxis, Tooltip, Legend } from 'recharts';
import { gql } from '@apollo/client';

const FEATURE_GROWTH_QUERY = gql`
  query FeatureGrowth($fromTime: DateTime!, $toTime: DateTime!, $interval: String!) {
    featureGrowth(fromTime: $fromTime, toTime: $toTime, interval: $interval) {
      timeBucket
      teamName
      cumulativeCount
    }
  }
`;

function FeatureGrowthChart() {
  const { data, loading } = useQuery(FEATURE_GROWTH_QUERY, {
    variables: {
      fromTime: new Date(Date.now() - 90 * 24 * 60 * 60 * 1000).toISOString(),
      toTime: new Date().toISOString(),
      interval: 'day'
    }
  });

  if (loading) return <div>Loading...</div>;

  // Group by team for multiple lines
  const teamData = groupByTeam(data.featureGrowth);

  return (
    <LineChart width={800} height={400} data={teamData}>
      <XAxis dataKey="timeBucket" />
      <YAxis />
      <Tooltip />
      <Legend />
      {Object.keys(teamData).map((teamName, index) => (
        <Line
          key={teamName}
          type="monotone"
          dataKey="cumulativeCount"
          stroke={COLORS[index % COLORS.length]}
          name={teamName}
        />
      ))}
    </LineChart>
  );
}
```

### Data Transformation Helper

```typescript
function groupByTeam(growthData: FeatureGrowthPoint[]): any {
  const teamMap = new Map();
  
  growthData.forEach(point => {
    const teamName = point.teamName || 'Unknown Team';
    if (!teamMap.has(teamName)) {
      teamMap.set(teamName, []);
    }
    teamMap.get(teamName).push({
      timeBucket: new Date(point.timeBucket).toLocaleDateString(),
      cumulativeCount: point.cumulativeCount,
      featureCount: point.featureCount
    });
  });
  
  return Object.fromEntries(teamMap);
}
```

---

## Performance Considerations

### Database Performance

1. **Indexes**: The query benefits from existing indexes on:
   - `features.created_at` - Time range filtering
   - `features.team_id` - Team filtering
   - Consider composite index: `(team_id, created_at)` for optimal performance

2. **Date Range**: Recommended maximum ranges:
   - Daily: 180 days (6 months)
   - Weekly: 2 years
   - Monthly: 5 years

3. **Query Complexity**: Uses window functions efficiently
   - Single table scan with aggregation
   - Window function processes data in memory
   - JOIN with teams table is cheap (small table)

### Caching Strategies

1. **Client-side**: Cache results for 5-15 minutes
2. **Server-side**: Consider materialized views for very large datasets
3. **Frontend**: Use polling with `pollInterval` in Apollo Client

```typescript
const { data } = useQuery(FEATURE_GROWTH_QUERY, {
  variables: { /* ... */ },
  pollInterval: 300000, // Refresh every 5 minutes
  fetchPolicy: 'cache-and-network'
});
```

---

## Error Handling

### Invalid Interval Error

```json
{
  "errors": [
    {
      "message": "Invalid interval. Must be 'day', 'week', or 'month'",
      "path": ["featureGrowth"]
    }
  ]
}
```

**Solution:** Validate interval on frontend before sending request.

### Invalid Team ID Error

```json
{
  "errors": [
    {
      "message": "Invalid team ID format: invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `x` at 1",
      "path": ["featureGrowth"]
    }
  ]
}
```

**Solution:** Ensure team IDs are valid UUIDs.

### Database Error

```json
{
  "errors": [
    {
      "message": "Database error: error returned from database: relation \"features\" does not exist",
      "path": ["featureGrowth"]
    }
  ]
}
```

**Solution:** Check database migrations are applied.

---

## Use Cases

### 1. System Overview Dashboard - Feature Growth Chart

**Display:** Area chart showing cumulative feature count over 90 days

**Implementation:**
- Interval: `day`
- Time Range: Last 90 days
- Team Filter: All teams (stacked areas by team)

**Business Value:** 
- Track overall platform growth
- Identify growth acceleration/deceleration
- Compare team productivity

### 2. Team Dashboard - Feature Creation Velocity

**Display:** Bar chart showing weekly feature count for specific team

**Implementation:**
- Interval: `week`
- Time Range: Last 12 weeks
- Team Filter: Selected team

**Business Value:**
- Monitor team delivery velocity
- Sprint planning and capacity insights
- Identify productivity trends

### 3. Executive Dashboard - Annual Growth Report

**Display:** Monthly cumulative growth line chart

**Implementation:**
- Interval: `month`
- Time Range: Current year
- Team Filter: None (all teams aggregated)

**Business Value:**
- Year-over-year growth comparison
- Strategic planning insights
- Stakeholder reporting

### 4. Team Comparison View

**Display:** Multi-line chart comparing cumulative growth across teams

**Implementation:**
- Interval: `week`
- Time Range: Last 6 months
- Query multiple teams and overlay lines

**Business Value:**
- Identify high-performing teams
- Resource allocation decisions
- Team benchmarking

---

## Testing

### Manual Testing

1. **Query with Daily Interval:**
```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "query { featureGrowth(fromTime: \"2024-01-01T00:00:00Z\", toTime: \"2024-01-31T23:59:59Z\", interval: \"day\") { timeBucket featureCount cumulativeCount } }"
  }'
```

2. **Query with Team Filter:**
```bash
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "query { featureGrowth(fromTime: \"2024-01-01T00:00:00Z\", toTime: \"2024-12-31T23:59:59Z\", interval: \"month\", teamId: \"51ecc366-f1cd-4d3d-ab73-fa60bad98f27\") { timeBucket teamName featureCount cumulativeCount } }"
  }'
```

3. **Test Invalid Interval:**
```bash
# Should return error
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "query { featureGrowth(fromTime: \"2024-01-01T00:00:00Z\", toTime: \"2024-12-31T23:59:59Z\", interval: \"hour\") { timeBucket } }"
  }'
```

### Expected Results

For a database with features created:
- Jan 1, 2024: 3 features (Team A: 2, Team B: 1)
- Jan 2, 2024: 2 features (Team A: 1, Team B: 1)
- Jan 3, 2024: 1 feature (Team A: 1)

**Daily Query Result:**
```json
[
  {
    "timeBucket": "2024-01-01T00:00:00Z",
    "teamId": "team-a-uuid",
    "teamName": "Team A",
    "featureCount": 2,
    "cumulativeCount": 2
  },
  {
    "timeBucket": "2024-01-01T00:00:00Z",
    "teamId": "team-b-uuid",
    "teamName": "Team B",
    "featureCount": 1,
    "cumulativeCount": 1
  },
  {
    "timeBucket": "2024-01-02T00:00:00Z",
    "teamId": "team-a-uuid",
    "teamName": "Team A",
    "featureCount": 1,
    "cumulativeCount": 3
  },
  {
    "timeBucket": "2024-01-02T00:00:00Z",
    "teamId": "team-b-uuid",
    "teamName": "Team B",
    "featureCount": 1,
    "cumulativeCount": 2
  },
  {
    "timeBucket": "2024-01-03T00:00:00Z",
    "teamId": "team-a-uuid",
    "teamName": "Team A",
    "featureCount": 1,
    "cumulativeCount": 4
  }
]
```

---

## Future Enhancements

### 1. Aggregated View (No Team Breakdown)

Add query parameter to return aggregated data across all teams without team grouping.

**GraphQL:**
```graphql
featureGrowth(
  fromTime: DateTime!
  toTime: DateTime!
  interval: String!
  aggregateTeams: Boolean
)
```

### 2. Feature Type Filtering

Filter growth by feature type (Simple vs Contextual).

**GraphQL:**
```graphql
featureGrowth(
  fromTime: DateTime!
  toTime: DateTime!
  interval: String!
  featureType: FeatureType
)
```

### 3. Percentage Growth Calculation

Include period-over-period growth percentages.

**FeatureGrowthPoint:**
```graphql
type FeatureGrowthPoint {
  # ... existing fields
  growthRate: Float  # Percentage change from previous period
}
```

### 4. Moving Average

Add moving average for trend smoothing.

**Implementation:**
```sql
AVG(feature_count) OVER (
  PARTITION BY team_id 
  ORDER BY time_bucket 
  ROWS BETWEEN 6 PRECEDING AND CURRENT ROW
) as moving_avg_7day
```

### 5. Materialized View

For very large datasets (millions of features), create a materialized view:

```sql
CREATE MATERIALIZED VIEW feature_growth_daily AS
WITH time_series AS (
  -- growth query here
)
SELECT * FROM time_series;

CREATE INDEX ON feature_growth_daily (time_bucket, team_id);
```

Refresh daily via cron job.

---

## Related Files

- **Repository**: `src/database/feature.rs`
- **GraphQL Schema**: `src/graphql/schema.rs`
- **GraphQL Query**: `src/graphql/query.rs`
- **Documentation**: This file

---

## Summary

The Feature Growth Query provides:

✅ **Time-Series Analytics**: Track feature creation trends over time  
✅ **Team Breakdown**: Compare productivity across teams  
✅ **Flexible Intervals**: Day, week, or month granularity  
✅ **Cumulative Counts**: See total growth trajectory  
✅ **GraphQL API**: Type-safe queries with validation  
✅ **Performance**: Optimized SQL with window functions  
✅ **Production Ready**: Compiled and tested  

This query is essential for the System Overview Dashboard (Task 2.6) and provides valuable insights for product management, team leads, and executives.
