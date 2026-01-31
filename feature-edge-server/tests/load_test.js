import http from 'k6/http';
import {check, sleep} from 'k6';
import {Counter, Rate, Trend} from 'k6/metrics';

// ============================================================================
// FluxGate Edge Server OFREP Performance Test
// ============================================================================
// 
// Tests the OpenFeature Remote Evaluation Protocol (OFREP) endpoint:
//   POST /ofrep/v1/evaluate/flags/{flagKey}
//
// Industry Benchmarks:
//   - P50: ≤10ms (target: ≤5ms)
//   - P95: ≤30ms (target: ≤15ms)  
//   - P99: ≤50ms (target: ≤25ms)
//
// Usage:
//   k6 run --env LOAD_TIER=1x tests/load_test.js
//   k6 run --env LOAD_TIER=1x --env CLIENT_ID=your-client-id tests/load_test.js
// ============================================================================

// Custom metrics
const evaluationLatency = new Trend('evaluation_latency', true);
const evaluationErrors = new Counter('evaluation_errors');
const evaluationSuccess = new Rate('evaluation_success');

// Load tier configurations
const LOAD_TIERS = {
    '1x': { vus: 50, rps: 500, duration: '2m' },    // Baseline production
    '2x': { vus: 100, rps: 1000, duration: '2m' },  // Peak hours
    '5x': { vus: 250, rps: 2500, duration: '2m' },  // High-traffic events
    '10x': { vus: 500, rps: 5000, duration: '2m' }, // Stress test
};

// Get configuration from environment
const loadTier = __ENV.LOAD_TIER || '1x';
const config = LOAD_TIERS[loadTier] || LOAD_TIERS['1x'];
const baseUrl = __ENV.BASE_URL || 'http://localhost:8081';

// Client authentication (from environment or defaults)
// Get these from: node populate_test_data.js output or database
const clientId = __ENV.CLIENT_ID || '';
const clientSecret = __ENV.CLIENT_SECRET || '';

// Test options
export const options = {
    scenarios: {
        constant_load: {
            executor: 'constant-arrival-rate',
            rate: config.rps,
            timeUnit: '1s',
            duration: config.duration,
            preAllocatedVUs: config.vus,
            maxVUs: config.vus * 2,
        },
    },
    thresholds: {
        // Industry-aligned thresholds
        'evaluation_latency': [
            'p(50)<10',   // P50 under 10ms
            'p(95)<30',   // P95 under 30ms
            'p(99)<50',   // P99 under 50ms
        ],
        'evaluation_success': ['rate>0.99'],  // 99% success rate
        'http_req_failed': ['rate<0.01'],     // Less than 1% failures
    },
};

// Feature keys matching populate_test_data.js
const FEATURE_KEYS = [
    // E-Commerce Team (Simple)
    'NewCheckoutFlow',
    'PaymentMethodApplePay',
    'ProductRecommendations',
    'GiftWrapping',
    'ExpressShipping',
    // E-Commerce Team (Contextual)
    'PremiumCheckout',
    'BulkOrderDiscount',
    'RegionalPricing',
    'LoyaltyProgram',
    'SeasonalPromotions',
    // Analytics Team
    // 'RealtimeDashboard',
    // 'ExportToCSV',
    // 'AdvancedFilters',
];

// Sample user IDs for bucketing
const USER_IDS = [
    'user-001', 'user-002', 'user-003', 'user-004', 'user-005',
    'user-006', 'user-007', 'user-008', 'user-009', 'user-010',
    'user-100', 'user-200', 'user-300', 'user-400', 'user-500',
];

// Environment IDs (from populate_test_data.js)
// Use actual UUID after running populate script
const ENVIRONMENT_IDS = [
    'bf06820b-3ff6-4235-b7c6-91b27f5ef9a6',
    '9646bb30-6bbe-48d8-89eb-a0200d4c95ce',
    '7c7efb52-018f-4140-87ae-e42322ffa94d',
];

function randomElement(arr) {
    return arr[Math.floor(Math.random() * arr.length)];
}

const MS_TO_US = 1000;
const toMicroseconds = (ms) => ms * MS_TO_US;
const formatLatencyUs = (ms) => `${toMicroseconds(ms).toFixed(0)}us`;
const formatLatencyUsWithMs = (ms) => `${formatLatencyUs(ms)} (${ms.toFixed(2)}ms)`;

export function setup() {
    const authMethod = clientId ? `Bearer token (${clientId.substring(0, 8)}...)` : 'Default (edge server credentials)';

    console.log(`
╔══════════════════════════════════════════════════════════════╗
║       FluxGate OFREP Endpoint Load Test                      ║
╠══════════════════════════════════════════════════════════════╣
║  Endpoint: POST /ofrep/v1/evaluate/flags/{key}               ║
╠══════════════════════════════════════════════════════════════╣
║  Load Tier: ${loadTier.padEnd(47)}                           ║
║  Target RPS: ${String(config.rps).padEnd(45)}                ║
║  Virtual Users: ${String(config.vus).padEnd(42)}             ║
║  Duration: ${config.duration.padEnd(47)}                     ║
║  Base URL: ${baseUrl.padEnd(47)}                             ║
║  Auth: ${authMethod.padEnd(51)}                              ║
╚══════════════════════════════════════════════════════════════╝

SETUP:
1. Start services: make up
2. Populate test data: node populate_test_data.js
3. Get client credentials from populate script output

    `);

    // Warm-up request
    const headers = { 'Content-Type': 'application/json' };
    if (clientId) {
        headers['Authorization'] = `Bearer ${clientId}`;
    }

    const warmupRes = http.post(`${baseUrl}/ofrep/v1/evaluate/flags/NewCheckoutFlow`, JSON.stringify({
        context: {
            targetingKey: 'warmup-user',
            environment_id: 'E-Commerce-Dev',
        }
    }), { headers });

    console.log(`Warm-up response: ${warmupRes.status}`);

    return { baseUrl, clientId };
}

export default function (data) {
    const featureKey = randomElement(FEATURE_KEYS);
    const userId = randomElement(USER_IDS);
    const envId = randomElement(ENVIRONMENT_IDS);

    // OFREP request body format
    const payload = JSON.stringify({
        context: {
            targetingKey: userId,
            // Additional attributes are passed as custom properties
            environment_id: envId,
            platform: 'web',
            version: '2.1.0',
            userTier: 'premium',  // For contextual features
        }
    });

    const headers = {
        'Content-Type': 'application/json',
    };

    // Add authentication if client ID is provided
    if (data.clientId) {
        headers['Authorization'] = `Bearer ${data.clientId}`;
    }

    const params = {
        headers,
        tags: { endpoint: 'ofrep_evaluate', feature: featureKey },
    };

    // OFREP endpoint: POST /ofrep/v1/evaluate/flags/{flagKey}
    const url = `${data.baseUrl}/ofrep/v1/evaluate/flags/${featureKey}`;

    const startTime = Date.now();
    const res = http.post(url, payload, params);
    const latency = Date.now() - startTime;

    // Record custom metrics
    evaluationLatency.add(latency);

    const success = check(res, {
        'status is 200': (r) => r.status === 200,
        'has value': (r) => {
            try {
                const body = JSON.parse(r.body);
                return body.value !== undefined;
            } catch {
                return false;
            }
        },
        'has reason': (r) => {
            try {
                const body = JSON.parse(r.body);
                return body.reason !== undefined;
            } catch {
                return false;
            }
        },
        'latency under 50ms': () => latency < 50,
    });

    if (success) {
        evaluationSuccess.add(1);
    } else {
        evaluationErrors.add(1);
        evaluationSuccess.add(0);
    }

    // Small random sleep to simulate realistic traffic patterns
    sleep(Math.random() * 0.1);
}

export function handleSummary(data) {
    const p50 = data.metrics.evaluation_latency?.values?.['p(50)'] || 0;
    const p95 = data.metrics.evaluation_latency?.values?.['p(95)'] || 0;
    const p99 = data.metrics.evaluation_latency?.values?.['p(99)'] || 0;
    const avg = data.metrics.evaluation_latency?.values?.avg || 0;
    const min = data.metrics.evaluation_latency?.values?.min || 0;
    const max = data.metrics.evaluation_latency?.values?.max || 0;
    const successRate = data.metrics.evaluation_success?.values?.rate || 0;
    const totalRequests = data.metrics.http_reqs?.values?.count || 0;
    const rps = data.metrics.http_reqs?.values?.rate || 0;
    const errorCount = data.metrics.evaluation_errors?.values?.count || 0;

    const latencyUs = {
        min: toMicroseconds(min),
        avg: toMicroseconds(avg),
        p50: toMicroseconds(p50),
        p95: toMicroseconds(p95),
        p99: toMicroseconds(p99),
        max: toMicroseconds(max),
    };

    // Summary for charts
    const chartData = {
        testInfo: {
            loadTier,
            targetRps: config.rps,
            duration: config.duration,
            timestamp: new Date().toISOString(),
        },
        latency: {
            min: min,
            avg: avg,
            p50: p50,
            p95: p95,
            p99: p99,
            max: max,
        },
        latencyUs: latencyUs,
        throughput: {
            totalRequests: totalRequests,
            rps: rps,
            successRate: successRate * 100,
            errorCount: errorCount,
        },
        thresholds: {
            p50Pass: p50 < 10,
            p95Pass: p95 < 30,
            p99Pass: p99 < 50,
            successPass: successRate > 0.99,
        }
    };

    // CSV format for easy import into Excel/Google Sheets
    const csvHeader = 'load_tier,target_rps,actual_rps,total_requests,p50_ms,p95_ms,p99_ms,avg_ms,min_ms,max_ms,p50_us,p95_us,p99_us,avg_us,min_us,max_us,success_rate,error_count,timestamp';
    const csvRow = `${loadTier},${config.rps},${rps.toFixed(2)},${totalRequests},${p50.toFixed(2)},${p95.toFixed(2)},${p99.toFixed(2)},${avg.toFixed(2)},${min.toFixed(2)},${max.toFixed(2)},${latencyUs.p50.toFixed(0)},${latencyUs.p95.toFixed(0)},${latencyUs.p99.toFixed(0)},${latencyUs.avg.toFixed(0)},${latencyUs.min.toFixed(0)},${latencyUs.max.toFixed(0)},${(successRate * 100).toFixed(2)},${errorCount},${new Date().toISOString()}`;
    const csvContent = `${csvHeader}\n${csvRow}`;

    console.log(`
╔══════════════════════════════════════════════════════════════╗
║              OFREP LOAD TEST RESULTS                         ║
╠══════════════════════════════════════════════════════════════╣
║  Load Tier: ${loadTier.padEnd(47)}║
╠══════════════════════════════════════════════════════════════╣
║  LATENCY                                                     ║
║    Min: ${String(formatLatencyUsWithMs(min)).padEnd(51)}║
║    P50: ${String(formatLatencyUsWithMs(p50)).padEnd(51)}║
║    P95: ${String(formatLatencyUsWithMs(p95)).padEnd(51)}║
║    P99: ${String(formatLatencyUsWithMs(p99)).padEnd(51)}║
║    Max: ${String(formatLatencyUsWithMs(max)).padEnd(51)}║
╠══════════════════════════════════════════════════════════════╣
║  THROUGHPUT                                                  ║
║    Total Requests: ${String(totalRequests).padEnd(40)}║
║    Requests/sec: ${String(rps.toFixed(2)).padEnd(42)}║
║    Success Rate: ${String((successRate * 100).toFixed(2) + '%').padEnd(42)}║
║    Errors: ${String(errorCount).padEnd(48)}║
╚══════════════════════════════════════════════════════════════╝

Output files:
  - results/summary_${loadTier}.json  (for programmatic access)
  - results/summary_${loadTier}.csv   (for spreadsheets)
  - results/raw_${loadTier}.json      (full k6 metrics)
    `);

    return {
        [`results/summary_${loadTier}.json`]: JSON.stringify(chartData, null, 2),
        [`results/summary_${loadTier}.csv`]: csvContent,
        [`results/raw_${loadTier}.json`]: JSON.stringify(data, null, 2),
    };
}
