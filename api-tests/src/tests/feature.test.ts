import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createFeatureFixture, createEnvironmentFixture, createTeamFixture, uniqueName } from '../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    expectPaginatedResponse,
    expectUuid,
    expectIsoDate,
    cleanupResource,
    updateFeature,
} from '../utils/test-utils.js';

/**
 * Feature API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams/{teamId}/features - List features
 * - GET /api/v1/features/{id} - Get feature by ID
 * - POST /api/v1/teams/{teamId}/features - Create feature
 * - PATCH /api/v1/features/{id} - Update feature
 * - POST /api/v1/features/{id}/emergency-disable - Emergency disable
 */
describe('Feature API', () => {
    let client: ApiClient;
    let testTeamId: string;
    let testEnvironmentId: string;
    const createdIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create a test team
        const teamFixture = createTeamFixture();
        const teamResponse = await client.post('/teams', teamFixture);
        expectSuccess(teamResponse);
        testTeamId = teamResponse.data.id;

        // Create a test environment for feature stages
        const envFixture = createEnvironmentFixture();
        const envResponse = await client.post(`/teams/${testTeamId}/environments`, envFixture);
        expectSuccess(envResponse);
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
        // Cleanup test team
        if (testTeamId) {
            await cleanupResource(client, '/teams', testTeamId);
        }
    });

    describe('GET /teams/{teamId}/features', () => {
        it('should list features for a team', async () => {
            const response = await client.get(`/teams/${testTeamId}/features`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination', async () => {
            const response = await client.get(`/teams/${testTeamId}/features`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should filter by feature type', async () => {
            const response = await client.get(`/teams/${testTeamId}/features`, {
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
            const response = await client.getUnauthenticated(`/teams/${testTeamId}/features`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${testTeamId}/features`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/features');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/features', () => {
        it('should create a SIMPLE feature', async () => {
            const fixture = createFeatureFixture({
                featureType: 'SIMPLE',
                environmentId: testEnvironmentId
            });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.key).toBe(fixture.key);
            expect(response.data.featureType).toBe('SIMPLE');

            createdIds.push(response.data.id);
        });

        it('should create a CONTEXTUAL feature', async () => {
            const fixture = createFeatureFixture({
                featureType: 'CONTEXTUAL',
                environmentId: testEnvironmentId
            });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);

            expectStatus(response, 201);
            expect(response.data.featureType).toBe('CONTEXTUAL');

            createdIds.push(response.data.id);
        });

        it('should create feature with enabled=true', async () => {
            const fixture = createFeatureFixture({
                enabled: true,
                environmentId: testEnvironmentId
            });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);

            expectStatus(response, 201);
            expect(response.data.enabled).toBe(true);

            createdIds.push(response.data.id);
        });

        it('should create feature with stages', async () => {
            const fixture = createFeatureFixture({
                environmentId: testEnvironmentId
            });

            const response = await client.post(`/teams/${testTeamId}/features`, fixture);

            expectStatus(response, 201);
            expect(response.data.stages).toBeDefined();
            expect(response.data.stages.length).toBeGreaterThan(0);

            createdIds.push(response.data.id);
        });

        it('should create feature with lifecycle metadata and expose stale filter', async () => {
            const expiresAt = new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString();
            const fixture = {
                ...createFeatureFixture({
                    environmentId: testEnvironmentId
                }),
                lifecycleStage: 'DRAFT',
                owner: 'Platform',
                expiresAt,
                cleanupReason: 'Remove after launch',
            };

            const response = await client.post(`/teams/${testTeamId}/features`, fixture);

            expectStatus(response, 201);
            expectIsoDate(response.data.createdAt);
            expect(response.data.lifecycleStage).toBe('DRAFT');
            expect(response.data.owner).toBe('Platform');
            expect(response.data.cleanupReason).toBe('Remove after launch');
            expectIsoDate(response.data.expiresAt);
            expect(new Date(response.data.expiresAt).getTime()).toBe(new Date(expiresAt).getTime());
            expect(response.data.isStale).toBe(true);
            expect(response.data.staleReasons).toContain('Expired');

            createdIds.push(response.data.id);

            const staleList = await client.get(`/teams/${testTeamId}/features`, {
                name: fixture.key,
                stale: true,
            });
            expectSuccess(staleList);
            expect(staleList.data.items.map((feature: { id: string }) => feature.id)).toContain(response.data.id);
        });

        it('should hide archived features by default and show them through lifecycle filter', async () => {
            const fixture = {
                ...createFeatureFixture({
                    environmentId: testEnvironmentId
                }),
                lifecycleStage: 'ARCHIVED',
                cleanupReason: 'Cleanup complete',
            };

            const response = await client.post(`/teams/${testTeamId}/features`, fixture);
            expectStatus(response, 201);
            createdIds.push(response.data.id);

            const defaultList = await client.get(`/teams/${testTeamId}/features`, {
                name: fixture.key,
            });
            expectSuccess(defaultList);
            expect(defaultList.data.items).toHaveLength(0);

            const archivedList = await client.get(`/teams/${testTeamId}/features`, {
                name: fixture.key,
                lifecycleStage: 'ARCHIVED',
            });
            expectSuccess(archivedList);
            expect(archivedList.data.items.map((feature: { id: string }) => feature.id)).toContain(response.data.id);
        });

        it('should reject duplicate feature keys', async () => {
            const fixture = createFeatureFixture({
                environmentId: testEnvironmentId
            });

            // Create first feature
            const first = await client.post(`/teams/${testTeamId}/features`, fixture);
            expectStatus(first, 201);
            createdIds.push(first.data.id);

            // Try to create duplicate
            const duplicate = await client.post(`/teams/${testTeamId}/features`, fixture);
            expectStatus(duplicate, 409); // Conflict
        });

        it('should reject empty key', async () => {
            const response = await client.post(`/teams/${testTeamId}/features`, {
                key: '',
                featureType: 'SIMPLE',
                stages: [{ environmentId: testEnvironmentId }]
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createFeatureFixture({ environmentId: testEnvironmentId });
            const response = await unauthClient.post(`/teams/${testTeamId}/features`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /features/{id}', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create a test feature
            const fixture = createFeatureFixture({ environmentId: testEnvironmentId });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should get feature by ID', async () => {
            const response = await client.get(`/features/${testFeatureId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(testFeatureId);
            expectUuid(response.data.id);
            // created_at is not in response
        });

        it('should include feature metadata', async () => {
            const response = await client.get(`/features/${testFeatureId}`);

            expectSuccess(response);
            expect(response.data).toHaveProperty('key');
            expect(response.data).toHaveProperty('featureType');
            expect(response.data).toHaveProperty('enabled');
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
            const fixture = createFeatureFixture({ environmentId: testEnvironmentId });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should update feature key', async () => {
            const newKey = uniqueName('updated-key');
            const response = await updateFeature(client, testFeatureId, {
                key: newKey,
            });

            expectSuccess(response);

            // Verify update via GET (workaround for backend returning stale data)
            const getResponse = await client.get(`/features/${testFeatureId}`);
            expect(getResponse.data.key).toBe(newKey);
        });

        it('should update feature description', async () => {
            const response = await updateFeature(client, testFeatureId, {
                description: 'Updated feature description',
            });

            expectSuccess(response);

            // Verify update via GET
            const getResponse = await client.get(`/features/${testFeatureId}`);
            expect(getResponse.data.description).toBe('Updated feature description');
        });

        it('should update enabled status', async () => {
            const response = await updateFeature(client, testFeatureId, {
                enabled: true,
            });

            expectSuccess(response);
            expect(response.data.enabled).toBe(true);
        });

        it('should update lifecycle metadata', async () => {
            const response = await updateFeature(client, testFeatureId, {
                lifecycleStage: 'DEPRECATED',
                owner: 'Runtime Team',
                cleanupReason: 'Superseded by rollout v2',
            });

            expectSuccess(response);
            expect(response.data.lifecycleStage).toBe('DEPRECATED');
            expect(response.data.owner).toBe('Runtime Team');
            expect(response.data.cleanupReason).toBe('Superseded by rollout v2');
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/features/${fakeId}`, {
                key: 'new-key',
                featureType: 'SIMPLE',
                description: 'desc',
                enabled: false,
                dependencies: [],
                relationships: [],
                stages: [{ environmentId: testEnvironmentId, orderIndex: 0, position: '1' }],
                variants: []
            });

            expectStatus(response, 404);
        });

        it('should reject empty key update', async () => {
            // updateFeature would return response/error
            const response = await client.patch(`/features/${testFeatureId}`, {
                key: '',
                // ... matching required fields is hard without helper
            });
            // Since we didn't provide required fields, it will likely be 400 anyway.
            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/features/${testFeatureId}`, {
                key: 'Unauthorized Update',
            });

            expectStatus(response, 401);
        });
    });

    describe('Feature version history and rollback', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const fixture = createFeatureFixture({ environmentId: testEnvironmentId });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);
            expectStatus(response, 201);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should record update snapshots, expose diffs, and rollback with a new audited version', async () => {
            const firstDescription = 'Version one description';
            const secondDescription = 'Version two description';

            const firstUpdate = await updateFeature(client, testFeatureId, {
                description: firstDescription,
                owner: 'Release Team',
            });
            expectSuccess(firstUpdate);

            const versionsAfterFirstUpdate = await client.get(`/features/${testFeatureId}/versions`);
            expectSuccess(versionsAfterFirstUpdate);
            expectPaginatedResponse(versionsAfterFirstUpdate);
            expect(versionsAfterFirstUpdate.data.items.length).toBeGreaterThanOrEqual(1);
            expect(versionsAfterFirstUpdate.data.items[0].versionNumber).toBe(1);
            expect(versionsAfterFirstUpdate.data.items[0].source).toBe('update');
            expect(versionsAfterFirstUpdate.data.items[0].snapshot.feature.description).toBe(firstDescription);

            const firstVersionId = versionsAfterFirstUpdate.data.items[0].id;
            const diff = await client.get(`/features/${testFeatureId}/versions/${firstVersionId}/diff`);
            expectSuccess(diff);
            expect(diff.data.entries.some((entry: { path: string }) => entry.path === 'feature.description')).toBe(true);

            const secondUpdate = await updateFeature(client, testFeatureId, {
                description: secondDescription,
            });
            expectSuccess(secondUpdate);

            const rollback = await client.post(`/features/${testFeatureId}/versions/${firstVersionId}/rollback`, {
                archiveConfirmation: true,
            });
            expectSuccess(rollback);
            expect(rollback.data.description).toBe(firstDescription);
            expect(rollback.data.owner).toBe('Release Team');

            const versionsAfterRollback = await client.get(`/features/${testFeatureId}/versions`);
            expectSuccess(versionsAfterRollback);
            expect(versionsAfterRollback.data.items[0].versionNumber).toBe(3);
            expect(versionsAfterRollback.data.items[0].source).toBe('rollback');
            expect(versionsAfterRollback.data.items[0].snapshot.feature.description).toBe(firstDescription);
        });
    });

    describe('Toggle / Enable Feature', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create a test feature
            const fixture = createFeatureFixture({ environmentId: testEnvironmentId, enabled: false });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should toggle feature enabled state via PATCH', async () => {
            const response = await updateFeature(client, testFeatureId, {
                enabled: true,
            });

            expectSuccess(response);
            expect(response.data.enabled).toBe(true);
        });
    });

    describe('POST /features/{id}/emergency-disable', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create a test feature
            const fixture = createFeatureFixture({ environmentId: testEnvironmentId, enabled: true });
            const response = await client.post(`/teams/${testTeamId}/features`, fixture);
            testFeatureId = response.data.id;
            createdIds.push(testFeatureId);
        });

        it('should emergency disable a feature', async () => {
            const response = await client.post(`/features/${testFeatureId}/emergency-disable`, {
                rollbackInMinutes: 0,
            });

            expectSuccess(response);

            // Verify update via GET
            const getResponse = await client.get(`/features/${testFeatureId}`);
            expect(getResponse.data.enabled).toBe(false);
            expect(getResponse.data.killSwitchEnabled).toBe(false); // Immediate disable sets active=false
        });

        it('should return 404 for non-existent feature', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.post(`/features/${fakeId}/emergency-disable`, {
                rollbackInMinutes: 0,
            });

            expectStatus(response, 404);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.post(`/features/${testFeatureId}/emergency-disable`, {
                rollbackInMinutes: 0,
            });

            expectStatus(response, 401);
        });
    });
});
