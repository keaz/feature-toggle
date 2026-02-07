import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createClientFixture, createEnvironmentFixture } from '../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    expectPaginatedResponse,
    expectUuid,
    TEST_TEAM_ID,
    cleanupResource,
} from '../utils/test-utils.js';

/**
 * Client API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams/{teamId}/clients - List clients
 * - GET /api/v1/clients/{id} - Get client by ID
 * - POST /api/v1/teams/{teamId}/clients - Create client
 * - PATCH /api/v1/clients/{id} - Update client
 */
describe('Client API', () => {
    let client: ApiClient;
    let testEnvironmentId: string;
    const createdIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create a test environment for client creation
        const envFixture = createEnvironmentFixture();
        const envResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`, envFixture);
        testEnvironmentId = envResponse.data.id;
    });

    afterAll(async () => {
        // Cleanup created clients
        for (const id of createdIds) {
            await cleanupResource(client, '/clients', id);
        }
        // Cleanup test environment
        if (testEnvironmentId) {
            await cleanupResource(client, '/environments', testEnvironmentId);
        }
    });

    describe('GET /teams/{teamId}/clients', () => {
        it('should list clients for a team', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/clients`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/clients`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${TEST_TEAM_ID}/clients`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${TEST_TEAM_ID}/clients`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/clients');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/clients', () => {
        it('should create a BACKEND client', async () => {
            const fixture = createClientFixture({
                clientType: 'BACKEND',
                environmentId: testEnvironmentId,
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/clients`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);
            expect(response.data.clientType).toBe('BACKEND');

            createdIds.push(response.data.id);
        });

        it('should create a WEB client with web origins', async () => {
            const fixture = createClientFixture({
                clientType: 'WEB',
                environmentId: testEnvironmentId,
                webOrigins: ['http://localhost:3000', 'https://app.example.com'],
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/clients`, fixture);

            expectStatus(response, 201);
            expect(response.data.clientType).toBe('WEB');
            expect(response.data.webOrigins).toHaveLength(2);

            createdIds.push(response.data.id);
        });

        it('should reject WEB client without web origins', async () => {
            const fixture = createClientFixture({
                clientType: 'WEB',
                environmentId: testEnvironmentId,
                webOrigins: [],
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/clients`, fixture);

            // WEB clients require at least one web origin
            expectClientError(response);
        });

        it('should reject empty name', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/clients`, {
                name: '',
                clientType: 'BACKEND',
                environmentId: testEnvironmentId,
            });

            expectClientError(response);
        });

        it('should reject invalid environment ID', async () => {
            const fixture = createClientFixture({
                environmentId: '00000000-0000-0000-0000-000000000000',
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/clients`, fixture);

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createClientFixture({ environmentId: testEnvironmentId });
            const response = await unauthClient.post(`/teams/${TEST_TEAM_ID}/clients`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /clients/{id}', () => {
        let testClientId: string;

        beforeAll(async () => {
            // Create a test client
            const fixture = createClientFixture({
                clientType: 'BACKEND',
                environmentId: testEnvironmentId,
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/clients`, fixture);
            testClientId = response.data.id;
            createdIds.push(testClientId);
        });

        it('should get client by ID', async () => {
            const response = await client.get(`/clients/${testClientId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(testClientId);
            expectUuid(response.data.id);
        });

        it('should include client secret in response', async () => {
            const response = await client.get(`/clients/${testClientId}`);

            expectSuccess(response);
            // Client should have either secret or secretKey
            expect(response.data.secret || response.data.secretKey).toBeDefined();
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.get(`/clients/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.get('/clients/not-a-uuid');

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/clients/${testClientId}`);

            expectStatus(response, 401);
        });
    });

    describe('PATCH /clients/{id}', () => {
        let testClientId: string;

        beforeAll(async () => {
            // Create a test client
            const fixture = createClientFixture({
                clientType: 'BACKEND',
                environmentId: testEnvironmentId,
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/clients`, fixture);
            testClientId = response.data.id;
            createdIds.push(testClientId);
        });

        it('should update client name', async () => {
            const newName = createClientFixture().name;
            const response = await client.patch(`/clients/${testClientId}`, {
                name: newName,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
        });

        it('should update client description', async () => {
            const response = await client.patch(`/clients/${testClientId}`, {
                description: 'Updated client description',
            });

            expectSuccess(response);
            expect(response.data.description).toBe('Updated client description');
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/clients/${fakeId}`, {
                name: 'New Name',
            });

            expectStatus(response, 404);
        });

        it('should reject empty name update', async () => {
            const response = await client.patch(`/clients/${testClientId}`, {
                name: '',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/clients/${testClientId}`, {
                name: 'Unauthorized Update',
            });

            expectStatus(response, 401);
        });
    });
});
