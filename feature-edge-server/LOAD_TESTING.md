# OFREP Load Testing Guide

Performance testing for the FluxGate Edge Server OFREP endpoint using [k6](https://k6.io/).

## Endpoint Under Test

```
POST /ofrep/v1/evaluate/flags/{flagKey}
```

## Quick Start

```bash
# 1. Install k6
brew install k6

# 2. Start services
make up

# 3. Populate test data
node populate_test_data.js

# 4. Create results directory
mkdir -p feature-edge-server/tests/results

# 5. Run load test
cd feature-edge-server
k6 run --env LOAD_TIER=1x tests/load_test.js
```

## Client Authentication

```bash
# With client auth (recommended)
k6 run --env LOAD_TIER=1x --env CLIENT_ID=<client-id> tests/load_test.js

# Without (uses edge server defaults from config.toml)
k6 run --env LOAD_TIER=1x tests/load_test.js
```

Get client credentials:
```sql
SELECT name, client_id, client_secret FROM clients;
```

## Load Tiers

| Tier | RPS | VUs | Duration |
|------|-----|-----|----------|
| **1x** | 500 | 50 | 2m |
| **2x** | 1,000 | 100 | 2m |
| **5x** | 2,500 | 250 | 2m |
| **10x** | 5,000 | 500 | 2m |

## Output Files

After each test run, the following files are generated in `tests/results/`:

| File | Format | Use Case |
|------|--------|----------|
| `summary_{tier}.json` | JSON | Programmatic access, dashboards |
| `summary_{tier}.csv` | CSV | Excel, Google Sheets |
| `raw_{tier}.json` | JSON | Full k6 metrics, debugging |

## Generating Charts

### Option 1: Run All Tiers & Combine

```bash
# Run all load tiers
for tier in 1x 2x 5x 10x; do
  k6 run --env LOAD_TIER=$tier tests/load_test.js
done

# Combine CSVs for charting
cd tests/results
cat summary_1x.csv > combined.csv
tail -1 summary_2x.csv >> combined.csv
tail -1 summary_5x.csv >> combined.csv
tail -1 summary_10x.csv >> combined.csv
```

### Option 2: Real-Time Metrics with InfluxDB + Grafana

```bash
# Start InfluxDB (docker)
docker run -d -p 8086:8086 influxdb:1.8

# Run k6 with InfluxDB output
k6 run --out influxdb=http://localhost:8086/k6 \
       --env LOAD_TIER=1x tests/load_test.js
```

### Option 3: k6 Cloud (Built-in Charts)

```bash
# Login to k6 cloud
k6 login cloud

# Run with cloud output
k6 cloud --env LOAD_TIER=1x tests/load_test.js
```

## CSV Output Format

```csv
load_tier,target_rps,actual_rps,total_requests,p50_ms,p95_ms,p99_ms,avg_ms,min_ms,max_ms,p50_us,p95_us,p99_us,avg_us,min_us,max_us,success_rate,error_count,timestamp
1x,500,495.23,60000,3.21,8.45,15.67,4.12,0.89,45.23,3210,8450,15670,4120,890,45230,99.98,12,2026-01-28T19:30:00Z
```

## JSON Summary Format

```json
{
  "testInfo": {
    "loadTier": "1x",
    "targetRps": 500,
    "duration": "2m",
    "timestamp": "2026-01-28T19:30:00Z"
  },
  "latency": {
    "min": 0.89,
    "avg": 4.12,
    "p50": 3.21,
    "p95": 8.45,
    "p99": 15.67,
    "max": 45.23
  },
  "latencyUs": {
    "min": 890,
    "avg": 4120,
    "p50": 3210,
    "p95": 8450,
    "p99": 15670,
    "max": 45230
  },
  "throughput": {
    "totalRequests": 60000,
    "rps": 495.23,
    "successRate": 99.98,
    "errorCount": 12
  },
  "thresholds": {
    "p50Pass": true,
    "p95Pass": true,
    "p99Pass": true,
    "successPass": true
  }
}
```

## Thresholds

| Metric | Pass | Industry |
|--------|------|----------|
| P50 | < 10ms | ≤10ms |
| P95 | < 30ms | ≤30ms |
| P99 | < 50ms | ≤50ms |
| Success | > 99% | >99% |

## Test Variants

### Standard Test (Cached Users)
Uses a fixed pool of user IDs, simulating realistic cache hit rates:
```bash
k6 run --env LOAD_TIER=1x tests/load_test.js
```

### Unique Users Test (No Cache)
Generates unique user IDs for **every request**, measuring worst-case performance:
```bash
k6 run --env LOAD_TIER=1x tests/load_test_unique_users.js
```

**Use this to:**
- Measure worst-case latency (no assignment cache hits)
- Test evaluation engine performance under maximum load
- Benchmark database write performance (new assignments)

**Expected results:**
- Higher P95/P99 vs standard test
- More database writes
- Tests true evaluation throughput

## Troubleshooting

### Results Directory Error
```
could not open 'results/summary_1x.json': no such file or directory
```

**Fix:**
```bash
mkdir -p feature-edge-server/tests/results
```

### Threshold Failures

If you see:
```
ERRO thresholds on metrics 'evaluation_success, http_req_failed' have been crossed
```

This means tests failed. Common causes:

| Issue | Solution |
|-------|----------|
| Edge server not running | `make up` or `docker ps` |
| Features not deployed | Run `node populate_test_data.js` |
| Wrong environment IDs | Update `ENVIRONMENT_IDS` in test script |
| Wrong client credentials | Update `config.toml` with client from database |
| Backend not accessible | Check `backend_grpc` in `config.toml` |

**Check edge server logs:**
```bash
make logs-edge
# or
docker logs feature_toggle_edge_server
```

**Verify feature exists:**
```bash
curl -X POST http://localhost:8081/ofrep/v1/evaluate/flags/NewCheckoutFlow \
  -H "Content-Type: application/json" \
  -d '{
    "context": {
      "targetingKey": "test-user",
      "environment_id": "bf06820b-3ff6-4235-b7c6-91b27f5ef9a6"
    }
  }'
```
