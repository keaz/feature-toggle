import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createFeatureFixture, createEnvironmentFixture, uniqueName } from '../utils/test-fixtures.js';
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
 * Feature API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams/{teamId}/features - List features
 * - GET /api/v1/features/{id} - Get feature by ID
 * - POST /api/v1/teams/{teamId}/features - Create feature
 * - PATCH /api/v1/features/{id} - Update feature
 * - POST /api/v1/features/{id}/toggle - Toggle feature
 * - POST /api/v1/features/{id}/emergency-disable - Emergency disable
 */
describe('Feature API', () => {
    let client: ApiClient;
    let testEnvironmentId: string;
    const createdIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create a test environment for feature stages
        const envFixture = createEnvironmentFixture();
        const envResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`, envFixture);
        testEnvironmentId = envResponse.data.id;
    });

    afterAll(async () => {
        // Cleanup created features
        for (const id of createdIds) {
            await cleanupResource(client, '/features', id);
        }
        // Cleanup test environment
        if (testEnvironmentId) {
            await cleanupResource(client, '/environments', testEnvironmentId);
        }
    });

    describe('GET /teams/{teamId}/features', () => {
        it('should list features for a team', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/features`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/features`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should filter by feature type', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/features`, {
                featureType: 'SIMPLE',
            });

            expectSuccess(response);
            if (response.data.items.length > 0) {
                response.data.items.forEach((feature: { featureType: string }) => {
                    expect(feature.featureType).toBe('SIMPLE');
                });
            }
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${TEST_TEAM_ID}/features`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${TEST_TEAM_ID}/features`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/features');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/features', () => {
        it('should create a SIMPLE feature', async () => {
            const fixture = createFeatureFixture({ featureType: 'SIMPLE' });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);
            expect(response.data.featureType).toBe('SIMPLE');

            createdIds.push(response.data.id);
        });

        it('should create a CONTEXTUAL feature', async () => {
            const fixture = createFeatureFixture({ featureType: 'CONTEXTUAL' });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);

            expectStatus(response, 201);
            expect(response.data.featureType).toBe('CONTEXTUAL');

            createdIds.push(response.data.id);
        });

        it('should create feature with default value true', async () => {
            const fixture = createFeatureFixture({ defaultValue: true });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);

            expectStatus(response, 201);
            expect(response.data.defaultValue).toBe(true);

            createdIds.push(response.data.id);
        });

        it('should create feature with stages', async () => {
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    {
                        environmentId: testEnvironmentId,
                        enabled: true,
                        rolloutPercentage: 100,
                    },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);

            expectStatus(response, 201);
            expect(response.data.stages).toBeDefined();

            createdIds.push(response.data.id);
        });

        it('should reject duplicate feature names', async () => {
            const fixture = createFeatureFixture();

            // Create first feature
            const first = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(first, 201);
            createdIds.push(first.data.id);

            // Try to create duplicate
            const duplicate = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(duplicate, 409); // Conflict
        });

        it('should reject empty name', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, {
                name: '',
                featureType: 'SIMPLE',
            });

            expectClientError(response);
        });

        it('should reject invalid feature type', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, {
                name: uniqueName('invalid-type'),
                featureType: 'INVALID_TYPE',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createFeatureFixture();
            const response = await unauthClient.post(`/teams/${TEST_TEAM_ID}/features`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /features/{id}', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create a test feature
            const fixture = createFeatureFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should get feature by ID', async () => {
            const response = await client.get(`/features/${testFeatureId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(testFeatureId);
            expectUuid(response.data.id);
            expectIsoDate(response.data.createdAt);
        });

        it('should include feature metadata', async () => {
            const response = await client.get(`/features/${testFeatureId}`);

            expectSuccess(response);
            expect(response.data).toHaveProperty('name');
            expect(response.data).toHaveProperty('featureType');
            expect(response.data).toHaveProperty('defaultValue');
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.get(`/features/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.get('/features/not-a-uuid');

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/features/${testFeatureId}`);

            expectStatus(response, 401);
        });
    });

    describe('PATCH /features/{id}', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create a test feature
            const fixture = createFeatureFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should update feature name', async () => {
            const newName = createFeatureFixture().name;
            const response = await client.patch(`/features/${testFeatureId}`, {
                name: newName,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
        });

        it('should update feature description', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                description: 'Updated feature description',
            });

            expectSuccess(response);
            expect(response.data.description).toBe('Updated feature description');
        });

        it('should update default value', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                defaultValue: true,
            });

            expectSuccess(response);
            expect(response.data.defaultValue).toBe(true);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/features/${fakeId}`, {
                name: 'New Name',
            });

            expectStatus(response, 404);
        });

        it('should reject empty name update', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                name: '',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/features/${testFeatureId}`, {
                name: 'Unauthorized Update',
            });

            expectStatus(response, 401);
        });
    });

    describe('POST /features/{id}/toggle', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create a test feature with stages
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    {
                        environmentId: testEnvironmentId,
                        enabled: false,
                        rolloutPercentage: 100,
                    },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should toggle feature enabled state', async () => {
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: testEnvironmentId,
                enabled: true,
            });

            // Toggle may require approval or succeed immediately
            expect([200, 201, 202]).toContain(response.status);
        });

        it('should return 404 for non-existent feature', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.post(`/features/${fakeId}/toggle`, {
                environmentId: testEnvironmentId,
                enabled: true,
            });

            expectStatus(response, 404);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.post(`/features/${testFeatureId}/toggle`, {
                environmentId: testEnvironmentId,
                enabled: true,
            });

            expectStatus(response, 401);
        });
    });

    describe('POST /features/{id}/emergency-disable', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create a test feature
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    {
                        environmentId: testEnvironmentId,
                        enabled: true,
                        rolloutPercentage: 100,
                    },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should emergency disable a feature', async () => {
            const response = await client.post(`/features/${testFeatureId}/emergency-disable`, {
                reason: 'Test emergency disable',
            });

            expectSuccess(response);
        });

        it('should return 404 for non-existent feature', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.post(`/features/${fakeId}/emergency-disable`, {
                reason: 'Test',
            });

            expectStatus(response, 404);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.post(`/features/${testFeatureId}/emergency-disable`, {
                reason: 'Unauthorized',
            });

            expectStatus(response, 401);
        });
    });
});
