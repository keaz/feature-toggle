# Feature Evaluation Dashboard - GraphQL Subscription Implementation

## Overview
This implementation provides real-time GraphQL subscriptions for feature evaluation analytics, designed to power a dashboard that displays feature evaluation rates and metrics over time periods up to 24 hours.

## Architecture

### 1. Database Layer (`feature_evaluation.rs`)
Enhanced the repository with time-series aggregation capabilities:

- **EvaluationRatePoint**: Time-bucketed aggregated metrics
  - `time_bucket`: Time period for the aggregation
  - `evaluation_count`: Total evaluations in the time bucket
  - `success_count`: Successful evaluations (result = true)
  - `prior_assignment_count`: Cached evaluations from prior assignments

- **EvaluationSummary**: Overall summary statistics
  - `total_evaluations`: Total count across time period
  - `successful_evaluations`: Count of successful evaluations
  - `cached_evaluations`: Count of prior assignment evaluations
  - `unique_users`: Number of unique users (when available)
  - `top_feature_key`: Most frequently evaluated feature
  - `success_rate` & `cache_hit_rate`: Pre-calculated percentages

#### Key Methods:
- `get_evaluation_rates()`: Returns time-bucketed rates using PostgreSQL's date_trunc
- `get_evaluation_summary()`: Returns aggregated summary statistics
- Uses concurrent queries for efficient data retrieval

### 2. Logic Layer (`feature_evaluation.rs`)
Business logic validation and orchestration:

- **Input Validation**:
  - Duration: 1-24 hours maximum
  - Interval: 1-60 minutes for rate aggregation
  - Time range: Prevents future dates
  - UUID validation for client IDs

- **Business Rules**:
  - Automatic percentage calculations (success rate, cache hit rate)
  - Time range boundary validation
  - Error handling and logging

### 3. GraphQL Subscription Layer (`subscription.rs`)
Real-time subscription endpoints with 30-second update intervals:

#### Input Types:
- **EvaluationRatesInput**: For rate-based subscriptions
  - Optional filtering by feature_key, environment_id, client_id
  - Time interval configuration (1-60 minutes)
  - Duration configuration (1-24 hours)

- **EvaluationSummaryInput**: For summary subscriptions
  - Same filtering options as rates
  - Duration-only configuration (no interval needed)

#### Output Types:
- **GqlEvaluationRatePoint**: Time-bucketed metrics with calculated rates
- **GqlEvaluationSummary**: Aggregated summary with percentages
- **GqlEvaluationDashboardData**: Combined rates and summary data

#### Subscription Methods:
1. **`evaluation_rates`**: Time-series data for charts and graphs
2. **`evaluation_summary`**: Summary statistics for dashboard overview
3. **`evaluation_dashboard`**: Combined data for complete dashboard view

### 4. Schema Integration (`lib.rs`)
- Full integration with existing GraphQL schema
- Dependency injection for logic layer
- Type-safe subscription registration

## Key Features

### Real-time Updates
- 30-second update intervals using tokio IntervalStream
- Non-blocking concurrent data fetching
- Efficient stream-based data delivery

### Flexible Filtering
- Filter by feature key, environment, or specific client
- Time range selection (1-24 hours)
- Configurable aggregation intervals (1-60 minutes)

### Performance Optimizations
- PostgreSQL time-series aggregation using date_trunc
- Concurrent query execution with tokio::join!
- Efficient streaming with minimal memory footprint
- Pre-calculated percentages to reduce client-side computation

### Error Handling
- Input validation with descriptive error messages
- Graceful error handling in streams
- Type-safe error propagation through GraphQL results

### Dashboard-Ready Data
- Pre-calculated success rates and cache hit rates
- ISO 8601 timestamps for easy frontend parsing
- Structured data suitable for charting libraries
- Consistent data format across all subscription types

## Usage Examples

### Basic Rate Subscription
```graphql
subscription {
  evaluationRates(input: {
    featureKey: "new_checkout_flow"
    environmentId: "production" 
    intervalMinutes: 5
    durationHours: 2
  }) {
    timeBucket
    evaluationCount
    successCount
    successRate
    cacheHitRate
  }
}
```

### Summary Statistics
```graphql
subscription {
  evaluationSummary(input: {
    environmentId: "production"
    durationHours: 24
  }) {
    totalEvaluations
    successfulEvaluations
    successRate
    cacheHitRate
    generatedAt
  }
}
```

### Complete Dashboard Data
```graphql
subscription {
  evaluationDashboard(input: {
    intervalMinutes: 15
    durationHours: 6
  }) {
    rates {
      timeBucket
      evaluationCount
      successRate
    }
    summary {
      totalEvaluations
      successRate
      cacheHitRate
    }
    generatedAt
  }
}
```

## Technical Implementation Details

### Stream Management
- Uses `Box<Pin<dyn Stream>>` for unified return types
- IntervalStream for consistent 30-second updates
- Futures-based async stream processing

### Database Queries
- Time-bucketed aggregation: `date_trunc('minute', created_at)`
- Efficient filtering with indexed columns
- Concurrent query execution for dashboard endpoint

### Type Safety
- Strong typing throughout the stack
- GraphQL schema validation
- Compile-time error checking for all data transformations

## Benefits for Dashboard Implementation

1. **Real-time Analytics**: Live updates every 30 seconds
2. **Historical Context**: Up to 24 hours of historical data
3. **Flexible Granularity**: 1-60 minute time buckets
4. **Performance**: Optimized queries and concurrent data fetching
5. **Reliability**: Comprehensive error handling and validation
6. **Developer Experience**: Type-safe GraphQL subscriptions with clear documentation

## Testing
Comprehensive test suite covering:
- Input validation logic
- UUID parsing and validation  
- Data structure creation and validation
- Edge cases for time boundaries
- Error conditions and responses

The implementation is production-ready and provides a solid foundation for building feature flag analytics dashboards with real-time data visualization capabilities.
