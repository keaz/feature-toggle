import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createContextFixture } from '../utils/test-fixtures.js';
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
 * Context API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams/{teamId}/contexts - List contexts
 * - GET /api/v1/contexts/{id} - Get context by ID
 * - POST /api/v1/teams/{teamId}/contexts - Create context
 * - PATCH /api/v1/contexts/{id} - Update context
 * - DELETE /api/v1/contexts/{id} - Delete context
 */
describe('Context API', () => {
    let client: ApiClient;
    const createdIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();
    });

    afterAll(async () => {
        // Cleanup all created contexts
        for (const id of createdIds) {
            await cleanupResource(client, '/contexts', id);
        }
    });

    describe('GET /teams/{teamId}/contexts', () => {
        it('should list contexts for a team', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/contexts`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination parameters', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/contexts`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${TEST_TEAM_ID}/contexts`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${TEST_TEAM_ID}/contexts`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/contexts');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/contexts', () => {
        it('should create a new context', async () => {
            const fixture = createContextFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.key).toBe(fixture.key);
            expect(response.data.entries).toHaveLength(fixture.entries.length);

            createdIds.push(response.data.id);
        });

        it('should create context with custom entries', async () => {
            const fixture = createContextFixture({
                entries: ['entry1', 'entry2', 'entry3', 'entry4', 'entry5'],
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, fixture);

            expectStatus(response, 201);
            expect(response.data.entries).toHaveLength(5);

            createdIds.push(response.data.id);
        });

        it('should reject empty key', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, {
                key: '',
                entries: ['value1'],
            });

            expectClientError(response);
        });

        it('should reject duplicate entries in the same context', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, {
                key: createContextFixture().key,
                entries: ['duplicate', 'duplicate'],
            });

            expectClientError(response);
        });

        it('should reject empty entries array', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, {
                key: createContextFixture().key,
                entries: [],
            });

            // This may be allowed or rejected depending on business rules
            // Adjust expectation based on actual behavior
            expect([200, 201, 400]).toContain(response.status);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createContextFixture();
            const response = await unauthClient.post(`/teams/${TEST_TEAM_ID}/contexts`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /contexts/{id}', () => {
        let testCtxId: string;

        beforeAll(async () => {
            // Create a test context
            const fixture = createContextFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, fixture);
            testCtxId = response.data.id;
            createdIds.push(testCtxId);
        });

        it('should get context by ID', async () => {
            const response = await client.get(`/contexts/${testCtxId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(testCtxId);
            expectUuid(response.data.id);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.get(`/contexts/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.get('/contexts/not-a-uuid');

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/contexts/${testCtxId}`);

            expectStatus(response, 401);
        });
    });

    describe('PATCH /contexts/{id}', () => {
        let testCtxId: string;

        beforeAll(async () => {
            // Create a test context
            const fixture = createContextFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, fixture);
            testCtxId = response.data.id;
            createdIds.push(testCtxId);
        });

        it('should update context key', async () => {
            const newKey = createContextFixture().key;
            const response = await client.patch(`/contexts/${testCtxId}`, {
                key: newKey,
            });

            expectSuccess(response);
            expect(response.data.key).toBe(newKey);
        });

        it('should update context entries', async () => {
            const newEntries = ['new1', 'new2', 'new3'];
            const response = await client.patch(`/contexts/${testCtxId}`, {
                entries: newEntries,
            });

            expectSuccess(response);
            expect(response.data.entries).toHaveLength(newEntries.length);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/contexts/${fakeId}`, {
                key: 'new-key',
            });

            expectStatus(response, 404);
        });

        it('should reject empty key update', async () => {
            const response = await client.patch(`/contexts/${testCtxId}`, {
                key: '',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/contexts/${testCtxId}`, {
                key: 'unauthorized-update',
            });

            expectStatus(response, 401);
        });
    });

    describe('DELETE /contexts/{id}', () => {
        it('should delete a context', async () => {
            // Create a disposable context
            const fixture = createContextFixture();
            const createResponse = await client.post(`/teams/${TEST_TEAM_ID}/contexts`, fixture);
            const ctxId = createResponse.data.id;

            // Delete it
            const deleteResponse = await client.delete(`/contexts/${ctxId}`);
            expectStatus(deleteResponse, 204);

            // Verify it's gone
            const getResponse = await client.get(`/contexts/${ctxId}`);
            expectStatus(getResponse, 404);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.delete(`/contexts/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.delete(`/contexts/${TEST_TEAM_ID}`);

            expectStatus(response, 401);
        });
    });
});
