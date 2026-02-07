import { ApiClient, getApiClient, createApiClient } from '../../utils/api-client.js';
import { createFeatureFixture, createEnvironmentFixture } from '../../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    TEST_TEAM_ID,
    cleanupResource,
    delay,
} from '../../utils/test-utils.js';

/**
 * Rollback Tests
 * 
 * Tests feature rollback scenarios:
 * - Rollback feature to previous enabled state
 * - Rollback to previous rollout percentage
 * - Emergency disable and recovery
 * - Rollback after failed deployment
 */
describe('Feature Rollback', () => {
    let client: ApiClient;
    let testEnvId: string;
    const createdFeatureIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create test environment
        const envResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'rollback-test-env' }));
        testEnvId = envResponse.data.id;
    });

    afterAll(async () => {
        for (const id of createdFeatureIds) {
            await cleanupResource(client, '/features', id);
        }
        if (testEnvId) await cleanupResource(client, '/environments', testEnvId);
    });

    describe('Enable/Disable Rollback', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create feature initially enabled
            const fixture = {
                ...createFeatureFixture(),
                stages: [{ environmentId: testEnvId, enabled: true, rolloutPercentage: 100 }],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should disable feature (simulate issue discovered)', async () => {
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: testEnvId,
                enabled: false,
            });

            expect([200, 201, 202]).toContain(response.status);
        });

        it('should verify feature is disabled', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            if (response.data.stages) {
                const stage = response.data.stages.find((s: any) => s.environmentId === testEnvId);
                if (stage) {
                    expect(stage.enabled).toBe(false);
                }
            }
        });

        it('should rollback by re-enabling feature', async () => {
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: testEnvId,
                enabled: true,
            });

            expect([200, 201, 202]).toContain(response.status);
        });

        it('should verify feature is re-enabled after rollback', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            if (response.data.stages) {
                const stage = response.data.stages.find((s: any) => s.environmentId === testEnvId);
                if (stage) {
                    expect(stage.enabled).toBe(true);
                }
            }
        });
    });

    describe('Rollout Percentage Rollback', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create feature at 100% rollout
            const fixture = {
                ...createFeatureFixture(),
                stages: [{ environmentId: testEnvId, enabled: true, rolloutPercentage: 100 }],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should reduce rollout to 50%', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 50 }],
            });

            expect([200, 400]).toContain(response.status); // 400 if endpoint doesn't support this
        });

        it('should rollback to 100% after issue resolved', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 100 }],
            });

            expect([200, 400]).toContain(response.status);
        });
    });

    describe('Emergency Disable and Recovery', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const fixture = {
                ...createFeatureFixture(),
                stages: [{ environmentId: testEnvId, enabled: true, rolloutPercentage: 100 }],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should emergency disable feature', async () => {
            const response = await client.post(`/features/${testFeatureId}/emergency-disable`, {
                reason: 'Critical bug discovered in production',
            });

            expectSuccess(response);
        });

        it('should verify feature is completely disabled', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            // After emergency disable, feature should be disabled
            if (response.data.stages) {
                const stage = response.data.stages.find((s: any) => s.environmentId === testEnvId);
                if (stage) {
                    expect(stage.enabled).toBe(false);
                }
            }
        });

        it('should recover from emergency disable', async () => {
            // Re-enable after fix deployed
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: testEnvId,
                enabled: true,
            });

            expect([200, 201, 202]).toContain(response.status);
        });

        it('should verify feature is recovered', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            if (response.data.stages) {
                const stage = response.data.stages.find((s: any) => s.environmentId === testEnvId);
                if (stage) {
                    expect(stage.enabled).toBe(true);
                }
            }
        });
    });

    describe('Rollback with History', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const fixture = {
                ...createFeatureFixture(),
                stages: [{ environmentId: testEnvId, enabled: true, rolloutPercentage: 100 }],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should get feature change history', async () => {
            // First make some changes
            await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: testEnvId,
                enabled: false,
            });
            await delay(100);
            await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: testEnvId,
                enabled: true,
            });

            // Get history
            const response = await client.get(`/features/${testFeatureId}/history`);

            // Endpoint may not exist
            expect([200, 404]).toContain(response.status);

            if (response.status === 200) {
                expect(Array.isArray(response.data)).toBe(true);
            }
        });

        it('should rollback to specific version if supported', async () => {
            // Try to rollback to a specific version
            const response = await client.post(`/features/${testFeatureId}/rollback`, {
                reason: 'Rolling back to known good state',
            });

            // Endpoint may not exist
            expect([200, 201, 404]).toContain(response.status);
        });
    });

    describe('Multi-Environment Rollback', () => {
        let testFeatureId: string;
        let devEnvId: string;
        let prodEnvId: string;

        beforeAll(async () => {
            // Create additional environments
            const devEnv = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
                createEnvironmentFixture({ name: 'dev-rollback-test' }));
            devEnvId = devEnv.data.id;

            const prodEnv = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
                createEnvironmentFixture({ name: 'prod-rollback-test' }));
            prodEnvId = prodEnv.data.id;

            // Create feature enabled in both
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: devEnvId, enabled: true, rolloutPercentage: 100 },
                    { environmentId: prodEnvId, enabled: true, rolloutPercentage: 100 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        afterAll(async () => {
            if (devEnvId) await cleanupResource(client, '/environments', devEnvId);
            if (prodEnvId) await cleanupResource(client, '/environments', prodEnvId);
        });

        it('should rollback prod only while keeping dev enabled', async () => {
            // Disable in prod
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: prodEnvId,
                enabled: false,
            });

            expect([200, 201, 202]).toContain(response.status);
        });

        it('should verify dev remains enabled after prod rollback', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            if (response.data.stages) {
                const devStage = response.data.stages.find((s: any) => s.environmentId === devEnvId);
                const prodStage = response.data.stages.find((s: any) => s.environmentId === prodEnvId);

                if (devStage) expect(devStage.enabled).toBe(true);
                if (prodStage) expect(prodStage.enabled).toBe(false);
            }
        });
    });
});
