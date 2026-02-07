import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createEnvironmentFixture, invalidData } from '../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    expectPaginatedResponse,
    expectUuid,
    expectIsoDate,
    TEST_TEAM_ID,
    cleanupResource,
} from '../utils/test-utils.js';

/**
 * Environment API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams/{teamId}/environments - List environments
 * - GET /api/v1/environments/{id} - Get environment by ID
 * - POST /api/v1/teams/{teamId}/environments - Create environment
 * - PATCH /api/v1/environments/{id} - Update environment
 * - DELETE /api/v1/environments/{id} - Delete environment
 */
describe('Environment API', () => {
    let client: ApiClient;
    const createdIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();
    });

    afterAll(async () => {
        // Cleanup all created environments
        for (const id of createdIds) {
            await cleanupResource(client, '/environments', id);
        }
    });

    describe('GET /teams/{teamId}/environments', () => {
        it('should list environments for a team', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/environments`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination parameters', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/environments`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should filter by active status', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/environments`, {
                active: true,
            });

            expectSuccess(response);
            if (response.data.items.length > 0) {
                response.data.items.forEach((env: { active: boolean }) => {
                    expect(env.active).toBe(true);
                });
            }
        });

        it('should filter by name', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/environments`, {
                name: 'Test',
            });

            expectSuccess(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${TEST_TEAM_ID}/environments`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${TEST_TEAM_ID}/environments`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/environments');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/environments', () => {
        it('should create a new environment', async () => {
            const fixture = createEnvironmentFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);
            expect(response.data.active).toBe(fixture.active);

            createdIds.push(response.data.id);
        });

        it('should create an inactive environment', async () => {
            const fixture = createEnvironmentFixture({ active: false });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);

            expectStatus(response, 201);
            expect(response.data.active).toBe(false);

            createdIds.push(response.data.id);
        });

        it('should reject duplicate environment names', async () => {
            const fixture = createEnvironmentFixture();

            // Create first environment
            const first = await client.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);
            expectStatus(first, 201);
            createdIds.push(first.data.id);

            // Try to create duplicate
            const duplicate = await client.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);
            expectStatus(duplicate, 409); // Conflict
        });

        it('should reject empty name', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/environments`, {
                name: '',
                active: true,
            });

            expectClientError(response);
        });

        it('should reject request without name', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/environments`, {
                active: true,
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createEnvironmentFixture();
            const response = await unauthClient.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /environments/{id}', () => {
        let testEnvId: string;

        beforeAll(async () => {
            // Create a test environment
            const fixture = createEnvironmentFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);
            testEnvId = response.data.id;
            createdIds.push(testEnvId);
        });

        it('should get environment by ID', async () => {
            const response = await client.get(`/environments/${testEnvId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(testEnvId);
            expectUuid(response.data.id);
            expectIsoDate(response.data.createdAt);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.get(`/environments/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.get('/environments/not-a-uuid');

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/environments/${testEnvId}`);

            expectStatus(response, 401);
        });
    });

    describe('PATCH /environments/{id}', () => {
        let testEnvId: string;

        beforeAll(async () => {
            // Create a test environment
            const fixture = createEnvironmentFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);
            testEnvId = response.data.id;
            createdIds.push(testEnvId);
        });

        it('should update environment name', async () => {
            const newName = createEnvironmentFixture().name;
            const response = await client.patch(`/environments/${testEnvId}`, {
                name: newName,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
        });

        it('should update environment active status', async () => {
            const response = await client.patch(`/environments/${testEnvId}`, {
                active: false,
            });

            expectSuccess(response);
            expect(response.data.active).toBe(false);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/environments/${fakeId}`, {
                name: 'New Name',
            });

            expectStatus(response, 404);
        });

        it('should reject empty name update', async () => {
            const response = await client.patch(`/environments/${testEnvId}`, {
                name: '',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/environments/${testEnvId}`, {
                name: 'Unauthorized Update',
            });

            expectStatus(response, 401);
        });
    });

    describe('DELETE /environments/{id}', () => {
        it('should delete an environment', async () => {
            // Create a disposable environment
            const fixture = createEnvironmentFixture();
            const createResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`, fixture);
            const envId = createResponse.data.id;

            // Delete it
            const deleteResponse = await client.delete(`/environments/${envId}`);
            expectStatus(deleteResponse, 204);

            // Verify it's gone
            const getResponse = await client.get(`/environments/${envId}`);
            expectStatus(getResponse, 404);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.delete(`/environments/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.delete(`/environments/${TEST_TEAM_ID}`);

            expectStatus(response, 401);
        });
    });
});
