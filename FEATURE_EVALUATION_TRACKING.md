# Feature Evaluation Tracking Implementation

This document describes the implementation of feature evaluation tracking in the feature toggle system. The system now captures detailed analytics about every feature evaluation that occurs in the edge server and stores them in the backend database.

## Overview

The feature evaluation tracking system consists of:

1. **Database Schema**: A new `feature_evaluations` table to store evaluation events
2. **Backend Components**: Repository, logic, and gRPC service extensions to handle evaluation data
3. **Edge Server Extensions**: Event collection and periodic streaming to backend
4. **gRPC Protocol**: New message types for evaluation event streaming

## Database Schema

### feature_evaluations Table

```sql
CREATE TABLE feature_evaluations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    feature_key VARCHAR NOT NULL,
    environment_id VARCHAR NOT NULL, 
    client_id UUID NOT NULL,
    evaluated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    evaluation_result BOOLEAN NOT NULL,
    evaluation_context JSONB,
    user_context VARCHAR,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);
```

**Fields:**
- `id`: Unique identifier for the evaluation record
- `feature_key`: The feature flag key that was evaluated
- `environment_id`: Environment where evaluation occurred (e.g., "prod", "staging")
- `client_id`: ID of the client making the evaluation request
- `evaluated_at`: Timestamp when the evaluation occurred 
- `evaluation_result`: Boolean result of the evaluation (true/false)
- `evaluation_context`: JSON containing the evaluation context (user attributes, etc.)
- `user_context`: Extracted user identifier for easier querying
- `created_at`: Timestamp when record was stored in database

**Indexes:**
- Individual indexes on `feature_key`, `environment_id`, `client_id`, `evaluated_at`, `user_context`
- Composite index on `(feature_key, environment_id, evaluated_at DESC)` for common queries
- Foreign key constraint to `clients` table

## Backend Implementation

### Repository Layer (`database/feature_evaluation.rs`)

The repository provides methods for:
- `create_evaluation()`: Store a single evaluation event
- `bulk_create_evaluations()`: Store multiple evaluation events efficiently 
- `get_evaluations()`: Query evaluation events with flexible filtering
- `get_evaluation_count()`: Count evaluations matching filter criteria

**Filter Options:**
- Feature key
- Environment ID
- Client ID  
- User context
- Date range (from/to)
- Pagination (limit/offset)

### Logic Layer (`logic/feature_evaluation.rs`)

The logic layer provides:
- Input validation for evaluation data
- Business logic for evaluation recording and querying
- Error handling and type conversions

### gRPC Service Extension (`grpc/mod.rs`)

New gRPC endpoint:
- `PushEvaluationEvents`: Accepts batches of evaluation events from edge servers

The service:
1. Authenticates the client using client_id/client_secret
2. Validates evaluation event data
3. Converts protobuf messages to database format
4. Stores events using bulk insert for efficiency

## Edge Server Implementation

### Event Collection

The edge server now:
1. **Captures evaluation events** in `evaluate_handler()` for every feature evaluation
2. **Stores events** in memory queue (`pending_evaluation_events`)
3. **Includes context** such as:
   - Feature key and environment
   - Evaluation result (true/false)
   - Full evaluation context (user attributes, etc.)
   - Extracted user identifier
   - Timestamp

### Periodic Streaming

New background task `run_evaluation_flush_task()`:
1. **Runs periodically** (configurable via `EDGE_EVALUATION_FLUSH_SECS`, default: 30s)
2. **Batches events** from memory queue
3. **Converts to gRPC format** and sends to backend
4. **Handles failures** by requeueing events for retry

## Protocol Definitions

### New Protobuf Messages

```protobuf
message FeatureEvaluationEvent {
  string feature_key = 1;
  string environment_id = 2;
  string client_id = 3;
  string client_secret = 4;
  bool evaluation_result = 5;
  repeated Context evaluation_context = 6;
  string user_context = 7;
  int64 evaluated_at_unix_ms = 8;
}

message PushEvaluationEventsRequest {
  repeated FeatureEvaluationEvent events = 1;
}

message PushEvaluationEventsResponse {
  string message_id = 1;
  int32 processed_count = 2;
}
```

### New gRPC Service Method

```protobuf
service FeatureEvaluation {
  // ... existing methods ...
  rpc PushEvaluationEvents(PushEvaluationEventsRequest) returns (PushEvaluationEventsResponse);
}
```

## Configuration

### Environment Variables

**Edge Server:**
- `EDGE_EVALUATION_FLUSH_SECS`: How often to flush evaluation events (default: 30 seconds)

**Existing:**
- `EDGE_CLIENT_ID`: Client ID for authentication
- `EDGE_CLIENT_SECRET`: Client secret for authentication
- `EDGE_BACKEND_GRPC`: Backend gRPC endpoint URL

## Usage and Benefits

### Analytics and Monitoring

With evaluation tracking, you can now:

1. **Monitor feature usage** - See which features are being evaluated most frequently
2. **Analyze adoption** - Track feature flag rollout success across environments
3. **Debug issues** - Investigate evaluation patterns when features behave unexpectedly
4. **Performance insights** - Understand evaluation frequency and timing patterns
5. **User behavior** - Analyze how different user segments interact with features

### Example Queries

```sql
-- Most evaluated features in last 24 hours
SELECT feature_key, COUNT(*) as evaluations
FROM feature_evaluations  
WHERE evaluated_at > NOW() - INTERVAL '24 hours'
GROUP BY feature_key
ORDER BY evaluations DESC;

-- Feature adoption rate over time
SELECT 
  feature_key,
  DATE(evaluated_at) as date,
  COUNT(*) as total_evaluations,
  SUM(CASE WHEN evaluation_result THEN 1 ELSE 0 END) as enabled_evaluations,
  ROUND(100.0 * SUM(CASE WHEN evaluation_result THEN 1 ELSE 0 END) / COUNT(*), 2) as adoption_rate
FROM feature_evaluations
GROUP BY feature_key, DATE(evaluated_at)
ORDER BY date DESC, feature_key;

-- User-specific feature evaluations
SELECT 
  user_context,
  feature_key,
  evaluation_result,
  evaluated_at
FROM feature_evaluations
WHERE user_context = 'user123'
ORDER BY evaluated_at DESC;
```

## Performance Considerations

### Edge Server

- **Memory usage**: Events are batched in memory before flushing, configurable flush interval
- **Network efficiency**: Bulk gRPC calls reduce network overhead
- **Fault tolerance**: Events are requeued on failure to prevent data loss

### Backend

- **Bulk inserts**: Multiple evaluation events stored in single database transaction
- **Indexing**: Optimized indexes for common query patterns
- **Partitioning**: Consider partitioning by date for high-volume deployments

### Database Storage

- **JSONB context**: Efficient storage and querying of evaluation context data
- **Retention policy**: Consider implementing automatic cleanup of old evaluation data
- **Compression**: Historical evaluation data compresses well

## Future Enhancements

Potential improvements:

1. **Streaming analytics**: Real-time evaluation event processing
2. **Aggregated metrics**: Pre-computed rollup tables for faster queries
3. **Data retention**: Automatic cleanup of old evaluation events
4. **Export capabilities**: Integration with external analytics systems
5. **Dashboard views**: Built-in analytics dashboard for evaluation insights
6. **Alerting**: Notifications for anomalous evaluation patterns

## Testing the Implementation

To test the feature evaluation tracking:

1. **Start the backend server**:
   ```bash
   cargo run --bin feature-toggle-backend
   ```

2. **Start the edge server**:
   ```bash  
   cargo run --bin feature-edge-server
   ```

3. **Make feature evaluation requests** through the edge server HTTP API:
   ```bash
   curl -X POST http://localhost:8081/evaluate \
     -H "Content-Type: application/json" \
     -d '{
       "feature_key": "my-feature",
       "environment_id": "prod", 
       "context": [
         {"key": "user.id", "value": "user123"},
         {"key": "country", "value": "US"}
       ]
     }'
   ```

4. **Check the database** to see evaluation events being stored:
   ```sql
   SELECT * FROM feature_evaluations ORDER BY evaluated_at DESC LIMIT 10;
   ```

The edge server will collect evaluation events and periodically (every 30 seconds by default) push them to the backend via gRPC, where they'll be stored in the database for analytics and monitoring.
