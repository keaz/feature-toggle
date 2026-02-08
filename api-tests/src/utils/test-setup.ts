/**
 * Test Setup - runs before all tests
 */
import { getApiClient, resetSharedClient } from './api-client.js';

// Increase Jest timeout for API tests
jest.setTimeout(30000);

// Suppress console output during tests (optional, remove if debugging)
// global.console = {
//   ...console,
//   log: jest.fn(),
//   debug: jest.fn(),
//   info: jest.fn(),
// };

beforeAll(async () => {
    console.log('🚀 Starting FluxGate API Tests');

    try {
        // Ensure each test file gets a fresh authenticated client.
        resetSharedClient();
        await getApiClient();
        console.log('🔐 API auth bootstrap complete');
    } catch (error) {
        console.error('❌ API auth bootstrap failed:', error);
        throw error;
    }
});

afterAll(() => {
    resetSharedClient();
    console.log('✅ FluxGate API Tests Complete');
});
