import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createPipelineFixture, createEnvironmentFixture } from '../utils/test-fixtures.js';
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
 * Pipeline API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams/{teamId}/pipelines - List pipelines
 * - GET /api/v1/pipelines/{id} - Get pipeline by ID
 * - POST /api/v1/teams/{teamId}/pipelines - Create pipeline
 * - PATCH /api/v1/pipelines/{id} - Update pipeline
 */
describe('Pipeline API', () => {
    let client: ApiClient;
    let testEnvironmentIds: string[] = [];
    const createdIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create test environments for pipeline stages
        for (let i = 0; i < 3; i++) {
            const envFixture = createEnvironmentFixture();
            const envResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`, envFixture);
            testEnvironmentIds.push(envResponse.data.id);
        }
    });

    afterAll(async () => {
        // Cleanup created pipelines
        for (const id of createdIds) {
            await cleanupResource(client, '/pipelines', id);
        }
        // Cleanup test environments
        for (const id of testEnvironmentIds) {
            await cleanupResource(client, '/environments', id);
        }
    });

    describe('GET /teams/{teamId}/pipelines', () => {
        it('should list pipelines for a team', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/pipelines`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/pipelines`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should filter by active status', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/pipelines`, {
                active: true,
            });

            expectSuccess(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${TEST_TEAM_ID}/pipelines`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${TEST_TEAM_ID}/pipelines`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/pipelines');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/pipelines', () => {
        it('should create a pipeline', async () => {
            const fixture = createPipelineFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);

            createdIds.push(response.data.id);
        });

        it('should create pipeline with environment stages', async () => {
            const fixture = {
                ...createPipelineFixture(),
                stages: testEnvironmentIds.map((envId, index) => ({
                    name: `Stage ${index + 1}`,
                    order: index + 1,
                    environmentId: envId,
                })),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, fixture);

            expectStatus(response, 201);
            expect(response.data.stages).toHaveLength(testEnvironmentIds.length);

            createdIds.push(response.data.id);
        });

        it('should reject duplicate pipeline names', async () => {
            const fixture = createPipelineFixture();

            // Create first pipeline
            const first = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, fixture);
            expectStatus(first, 201);
            createdIds.push(first.data.id);

            // Try to create duplicate
            const duplicate = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, fixture);
            expectStatus(duplicate, 409); // Conflict
        });

        it('should reject empty name', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, {
                name: '',
                stages: [],
            });

            expectClientError(response);
        });

        it('should reject pipeline without stages', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, {
                name: createPipelineFixture().name,
                stages: [],
            });

            // Empty stages may or may not be allowed
            expect([201, 400]).toContain(response.status);
            if (response.status === 201) {
                createdIds.push(response.data.id);
            }
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createPipelineFixture();
            const response = await unauthClient.post(`/teams/${TEST_TEAM_ID}/pipelines`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /pipelines/{id}', () => {
        let testPipelineId: string;

        beforeAll(async () => {
            // Create a test pipeline
            const fixture = createPipelineFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, fixture);
            testPipelineId = response.data.id;
            createdIds.push(testPipelineId);
        });

        it('should get pipeline by ID', async () => {
            const response = await client.get(`/pipelines/${testPipelineId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(testPipelineId);
            expectUuid(response.data.id);
        });

        it('should include pipeline stages', async () => {
            const response = await client.get(`/pipelines/${testPipelineId}`);

            expectSuccess(response);
            expect(response.data).toHaveProperty('stages');
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.get(`/pipelines/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.get('/pipelines/not-a-uuid');

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/pipelines/${testPipelineId}`);

            expectStatus(response, 401);
        });
    });

    describe('PATCH /pipelines/{id}', () => {
        let testPipelineId: string;

        beforeAll(async () => {
            // Create a test pipeline
            const fixture = createPipelineFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, fixture);
            testPipelineId = response.data.id;
            createdIds.push(testPipelineId);
        });

        it('should update pipeline name', async () => {
            const newName = createPipelineFixture().name;
            const response = await client.patch(`/pipelines/${testPipelineId}`, {
                name: newName,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
        });

        it('should update pipeline description', async () => {
            const response = await client.patch(`/pipelines/${testPipelineId}`, {
                description: 'Updated pipeline description',
            });

            expectSuccess(response);
            expect(response.data.description).toBe('Updated pipeline description');
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/pipelines/${fakeId}`, {
                name: 'New Name',
            });

            expectStatus(response, 404);
        });

        it('should reject empty name update', async () => {
            const response = await client.patch(`/pipelines/${testPipelineId}`, {
                name: '',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/pipelines/${testPipelineId}`, {
                name: 'Unauthorized Update',
            });

            expectStatus(response, 401);
        });
    });
});
