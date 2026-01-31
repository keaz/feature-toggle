import http from 'k6/http';
import {check, sleep} from 'k6';
import {Counter, Rate, Trend} from 'k6/metrics';
import {randomString} from 'https://jslib.k6.io/k6-utils/1.2.0/index.js';

// ============================================================================
// FluxGate Edge Server OFREP - Unique Users Test
// ============================================================================
// 
// Tests with UNIQUE USER IDs on every request to avoid caching effects.
// Use this to measure worst-case performance (no assignment cache hits).
//
// Endpoint: POST /ofrep/v1/evaluate/flags/{flagKey}
//
// Usage:
//   k6 run --env LOAD_TIER=1x tests/load_test_unique_users.js
// ============================================================================

// Custom metrics
const evaluationLatency = new Trend('evaluation_latency', true);
const evaluationErrors = new Counter('evaluation_errors');
const evaluationSuccess = new Rate('evaluation_success');
const uniqueUsersGenerated = new Counter('unique_users_generated');

// Load tier configurations
const LOAD_TIERS = {
    '1x': { vus: 50, rps: 500, duration: '2m' },
    '2x': { vus: 100, rps: 1000, duration: '2m' },
    '5x': { vus: 250, rps: 2500, duration: '2m' },
    '10x': { vus: 500, rps: 5000, duration: '2m' },
};

// Get configuration from environment
const loadTier = __ENV.LOAD_TIER || '1x';
const config = LOAD_TIERS[loadTier] || LOAD_TIERS['1x'];
const baseUrl = __ENV.BASE_URL || 'http://localhost:8081';
const clientId = __ENV.CLIENT_ID || '';

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
        'evaluation_latency': [
            'p(50)<10',
            'p(95)<30',
            'p(99)<50',
        ],
        'evaluation_success': ['rate>0.99'],
        'http_req_failed': ['rate<0.01'],
    },
};

// Feature keys from populate_test_data.js
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

// Environment IDs
const ENVIRONMENT_IDS = [
    'bf06820b-3ff6-4235-b7c6-91b27f5ef9a6',
    '9646bb30-6bbe-48d8-89eb-a0200d4c95ce',
    '7c7efb52-018f-4140-87ae-e42322ffa94d',
];

// User tier options for contextual features
const USER_TIERS = ['free', 'basic', 'premium', 'enterprise'];

// Request counter for unique ID generation
let requestCounter = 0;

function randomElement(arr) {
    return arr[Math.floor(Math.random() * arr.length)];
}

// Generate a unique user ID using VU ID, iteration count, and timestamp
function generateUniqueUserId() {
    const vuId = __VU;
    const iter = __ITER;
    const timestamp = Date.now();
    const random = randomString(8);
    uniqueUsersGenerated.add(1);
    return `user-${vuId}-${iter}-${timestamp}-${random}`;
}

export function setup() {
    const authMethod = clientId ? `Bearer token (${clientId.substring(0, 8)}...)` : 'Default (edge server credentials)';

    console.log(`
╔══════════════════════════════════════════════════════════════╗
║       FluxGate OFREP - Unique Users Test                     ║
╠══════════════════════════════════════════════════════════════╣
║  Endpoint: POST /ofrep/v1/evaluate/flags/{key}               ║
║  User Strategy: UNIQUE on every request (no cache hits)      ║
╠══════════════════════════════════════════════════════════════╣
║  Load Tier: ${loadTier.padEnd(47)}║
║  Target RPS: ${String(config.rps).padEnd(45)}║
║  Virtual Users: ${String(config.vus).padEnd(42)}║
║  Duration: ${config.duration.padEnd(47)}║
║  Base URL: ${baseUrl.padEnd(47)}║
║  Auth: ${authMethod.padEnd(51)}║
╚══════════════════════════════════════════════════════════════╝

NOTE: This test generates unique user IDs for every request.
This measures WORST-CASE performance with no assignment caching.
    `);

    // Warm-up request
    const headers = { 'Content-Type': 'application/json' };
    if (clientId) {
        headers['Authorization'] = `Bearer ${clientId}`;
    }

    const warmupRes = http.post(`${baseUrl}/ofrep/v1/evaluate/flags/NewCheckoutFlow`, JSON.stringify({
        context: {
            targetingKey: 'warmup-unique-user',
            environment_id: ENVIRONMENT_IDS[0],
        }
    }), { headers });

    console.log(`Warm-up response: ${warmupRes.status}`);

    return { baseUrl, clientId };
}

export default function (data) {
    const featureKey = randomElement(FEATURE_KEYS);
    const uniqueUserId = generateUniqueUserId();
    const envId = randomElement(ENVIRONMENT_IDS);
    const userTier = randomElement(USER_TIERS);

    // OFREP request body with unique user
    const payload = JSON.stringify({
        context: {
            targetingKey: uniqueUserId, // UNIQUE on every request
            environment_id: envId,
            platform: 'web',
            version: '2.1.0',
            userTier: userTier,
        }
    });

    const headers = {
        'Content-Type': 'application/json',
    };

    if (data.clientId) {
        headers['Authorization'] = `Bearer ${data.clientId}`;
    }

    const params = {
        headers,
        tags: {
            endpoint: 'ofrep_evaluate',
            feature: featureKey,
            test_type: 'unique_users'
        },
    };

    const url = `${data.baseUrl}/ofrep/v1/evaluate/flags/${featureKey}`;

    const startTime = Date.now();
    const res = http.post(url, payload, params);
    const latency = Date.now() - startTime;

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
    const uniqueUsers = data.metrics.unique_users_generated?.values?.count || 0;

    // Summary for charts
    const chartData = {
        testInfo: {
            testType: 'unique_users',
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
        throughput: {
            totalRequests: totalRequests,
            uniqueUsersGenerated: uniqueUsers,
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

    // CSV format
    const csvHeader = 'test_type,load_tier,target_rps,actual_rps,total_requests,unique_users,p50_ms,p95_ms,p99_ms,avg_ms,min_ms,max_ms,success_rate,error_count,timestamp';
    const csvRow = `unique_users,${loadTier},${config.rps},${rps.toFixed(2)},${totalRequests},${uniqueUsers},${p50.toFixed(2)},${p95.toFixed(2)},${p99.toFixed(2)},${avg.toFixed(2)},${min.toFixed(2)},${max.toFixed(2)},${(successRate * 100).toFixed(2)},${errorCount},${new Date().toISOString()}`;
    const csvContent = `${csvHeader}\n${csvRow}`;

    console.log(`
╔══════════════════════════════════════════════════════════════╗
║          UNIQUE USERS LOAD TEST RESULTS                      ║
╠══════════════════════════════════════════════════════════════╣
║  Load Tier: ${loadTier.padEnd(47)}║
║  Test Type: Unique Users (No Cache)                         ║
╠══════════════════════════════════════════════════════════════╣
║  LATENCY                                                     ║
║    Min: ${String(min.toFixed(2) + 'ms').padEnd(51)}║
║    P50: ${String(p50.toFixed(2) + 'ms').padEnd(51)}║
║    P95: ${String(p95.toFixed(2) + 'ms').padEnd(51)}║
║    P99: ${String(p99.toFixed(2) + 'ms').padEnd(51)}║
║    Max: ${String(max.toFixed(2) + 'ms').padEnd(51)}║
╠══════════════════════════════════════════════════════════════╣
║  THROUGHPUT                                                  ║
║    Total Requests: ${String(totalRequests).padEnd(40)}║
║    Unique Users: ${String(uniqueUsers).padEnd(42)}║
║    Requests/sec: ${String(rps.toFixed(2)).padEnd(42)}║
║    Success Rate: ${String((successRate * 100).toFixed(2) + '%').padEnd(42)}║
║    Errors: ${String(errorCount).padEnd(48)}║
╚══════════════════════════════════════════════════════════════╝

Output files:
  - results/unique_users_${loadTier}.json  (for charts)
  - results/unique_users_${loadTier}.csv   (for spreadsheets)
  - results/unique_users_raw_${loadTier}.json (full metrics)
    `);

    return {
        [`results/unique_users_${loadTier}.json`]: JSON.stringify(chartData, null, 2),
        [`results/unique_users_${loadTier}.csv`]: csvContent,
        [`results/unique_users_raw_${loadTier}.json`]: JSON.stringify(data, null, 2),
    };
}
