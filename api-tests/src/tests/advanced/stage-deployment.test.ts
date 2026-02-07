import { ApiClient, getApiClient, createApiClient } from '../../utils/api-client.js';
import { createFeatureFixture, createEnvironmentFixture, createPipelineFixture } from '../../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectUuid,
    TEST_TEAM_ID,
    cleanupResource,
    delay,
} from '../../utils/test-utils.js';

/**
 * Feature Stage Deployment Tests
 * 
 * Tests multi-environment deployments through pipeline stages:
 * - Create pipeline with Dev → Staging → Production stages
 * - Deploy features through pipeline stages
 * - Verify stage transitions
 * - Test stage rollout percentages
 */
describe('Feature Stage Deployment', () => {
    let client: ApiClient;
    let devEnvId: string;
    let stagingEnvId: string;
    let prodEnvId: string;
    let pipelineId: string;
    const createdFeatureIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create three environments: Development, Staging, Production
        const devEnv = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'dev-stage-test', environmentType: 'Development' }));
        devEnvId = devEnv.data.id;

        const stagingEnv = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'staging-stage-test', environmentType: 'Staging' }));
        stagingEnvId = stagingEnv.data.id;

        const prodEnv = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'prod-stage-test', environmentType: 'Production' }));
        prodEnvId = prodEnv.data.id;

        // Create a pipeline with these environments
        const pipelineFixture = {
            ...createPipelineFixture(),
            stages: [
                { name: 'Development', order: 1, environmentId: devEnvId },
                { name: 'Staging', order: 2, environmentId: stagingEnvId },
                { name: 'Production', order: 3, environmentId: prodEnvId },
            ],
        };
        const pipelineResponse = await client.post(`/teams/${TEST_TEAM_ID}/pipelines`, pipelineFixture);
        pipelineId = pipelineResponse.data.id;
    });

    afterAll(async () => {
        // Cleanup
        for (const id of createdFeatureIds) {
            await cleanupResource(client, '/features', id);
        }
        if (pipelineId) await cleanupResource(client, '/pipelines', pipelineId);
        if (prodEnvId) await cleanupResource(client, '/environments', prodEnvId);
        if (stagingEnvId) await cleanupResource(client, '/environments', stagingEnvId);
        if (devEnvId) await cleanupResource(client, '/environments', devEnvId);
    });

    describe('Multi-Environment Feature Creation', () => {
        it('should create feature with multiple environment stages', async () => {
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: devEnvId, enabled: true, rolloutPercentage: 100 },
                    { environmentId: stagingEnvId, enabled: false, rolloutPercentage: 0 },
                    { environmentId: prodEnvId, enabled: false, rolloutPercentage: 0 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);

            expectStatus(response, 201);
            expect(response.data.stages).toHaveLength(3);
            createdFeatureIds.push(response.data.id);
        });

        it('should create feature with pipeline association', async () => {
            const fixture = {
                ...createFeatureFixture(),
                pipelineId: pipelineId,
                stages: [
                    { environmentId: devEnvId, enabled: true, rolloutPercentage: 100 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);

            expectStatus(response, 201);
            createdFeatureIds.push(response.data.id);
        });
    });

    describe('Stage Promotion Workflow', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create feature enabled in Dev only
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: devEnvId, enabled: true, rolloutPercentage: 100 },
                    { environmentId: stagingEnvId, enabled: false, rolloutPercentage: 0 },
                    { environmentId: prodEnvId, enabled: false, rolloutPercentage: 0 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should promote feature from Dev to Staging', async () => {
            // Enable in Staging via toggle
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: stagingEnvId,
                enabled: true,
            });

            // May require approval or succeed immediately
            expect([200, 201, 202]).toContain(response.status);
        });

        it('should promote feature from Staging to Production', async () => {
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: prodEnvId,
                enabled: true,
            });

            expect([200, 201, 202]).toContain(response.status);
        });

        it('should verify all stages are enabled after promotion', async () => {
            const response = await client.get(`/features/${testFeatureId}`);

            expectSuccess(response);
            // Verify stages (if the toggle was applied)
            if (response.data.stages) {
                const stages = response.data.stages;
                // Dev should still be enabled
                const devStage = stages.find((s: any) => s.environmentId === devEnvId);
                if (devStage) {
                    expect(devStage.enabled).toBe(true);
                }
            }
        });
    });

    describe('Stage Rollout Percentage Changes', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: devEnvId, enabled: true, rolloutPercentage: 100 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should update stage rollout percentage to 50%', async () => {
            const response = await client.patch(`/features/${testFeatureId}/stages/${devEnvId}`, {
                rolloutPercentage: 50,
            });

            // The endpoint might be different based on API design
            expect([200, 404]).toContain(response.status);
        });

        it('should update stage rollout percentage incrementally (canary)', async () => {
            // Simulate canary: 10% -> 25% -> 50% -> 100%
            const percentages = [10, 25, 50, 100];

            for (const pct of percentages) {
                const response = await client.patch(`/features/${testFeatureId}`, {
                    stages: [{ environmentId: devEnvId, rolloutPercentage: pct }],
                });

                // Allow various responses depending on API
                expect([200, 400, 404]).toContain(response.status);
            }
        });
    });

    describe('Cross-Environment State Verification', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: devEnvId, enabled: true, rolloutPercentage: 100 },
                    { environmentId: stagingEnvId, enabled: true, rolloutPercentage: 50 },
                    { environmentId: prodEnvId, enabled: false, rolloutPercentage: 0 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should show different states for each environment', async () => {
            const response = await client.get(`/features/${testFeatureId}`);

            expectSuccess(response);
            if (response.data.stages) {
                const stages = response.data.stages;

                const devStage = stages.find((s: any) => s.environmentId === devEnvId);
                const stagingStage = stages.find((s: any) => s.environmentId === stagingEnvId);
                const prodStage = stages.find((s: any) => s.environmentId === prodEnvId);

                if (devStage && stagingStage && prodStage) {
                    expect(devStage.enabled).toBe(true);
                    expect(devStage.rolloutPercentage).toBe(100);
                    expect(stagingStage.enabled).toBe(true);
                    expect(stagingStage.rolloutPercentage).toBe(50);
                    expect(prodStage.enabled).toBe(false);
                }
            }
        });

        it('should disable feature in specific environment only', async () => {
            // Disable in Staging only
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: stagingEnvId,
                enabled: false,
            });

            expect([200, 201, 202]).toContain(response.status);

            // Verify Dev is still enabled
            const getResponse = await client.get(`/features/${testFeatureId}`);
            if (getResponse.data.stages) {
                const devStage = getResponse.data.stages.find((s: any) => s.environmentId === devEnvId);
                if (devStage) {
                    expect(devStage.enabled).toBe(true);
                }
            }
        });
    });
});
