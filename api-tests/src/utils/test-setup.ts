/**
 * Test Setup - runs before all tests
 */

// Increase Jest timeout for API tests
jest.setTimeout(30000);

// Suppress console output during tests (optional, remove if debugging)
// global.console = {
//   ...console,
//   log: jest.fn(),
//   debug: jest.fn(),
//   info: jest.fn(),
// };

beforeAll(() => {
    console.log('🚀 Starting FluxGate API Tests');
});

afterAll(() => {
    console.log('✅ FluxGate API Tests Complete');
});
