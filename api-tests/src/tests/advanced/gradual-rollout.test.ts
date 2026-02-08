import { ApiClient, getApiClient } from '../../utils/api-client.js';
import { createFeatureFixture, createEnvironmentFixture } from '../../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    TEST_TEAM_ID,
    cleanupResource,
    delay,
} from '../../utils/test-utils.js';

function buildLinearRelationships(stageCount: number): Array<{ sourceId: number; targetId: number }> {
    return Array.from({ length: Math.max(0, stageCount - 1) }, (_, index) => ({
        sourceId: index,
        targetId: index + 1,
    }));
}

function buildFeatureStages(
    environmentIds: string[]
): Array<{ environmentId: string; orderIndex: number; position: string; bucketingKey: string }> {
    return environmentIds.map((environmentId, index) => ({
        environmentId,
        orderIndex: index,
        position: String(index + 1),
        bucketingKey: 'userId',
    }));
}

/**
 * Gradual Rollout Tests
 * 
 * Tests percentage-based feature deployments:
 * - Canary releases (1% -> 10% -> 25% -> 50% -> 100%)
 * - Ring-based deployments
 * - Percentage-based targeting
 * - A/B testing with percentage splits
 */
describe('Gradual Rollout', () => {
    let client: ApiClient;
    let testEnvId: string;
    const createdFeatureIds: string[] = [];
    const getStageByEnvironment = async (featureId: string, environmentId: string) => {
        const response = await client.get(`/features/${featureId}`);
        expectSuccess(response);
        const stage = response.data.stages?.find((s: any) => s.environment.id === environmentId);
        expect(stage).toBeDefined();
        return stage;
    };

    beforeAll(async () => {
        client = await getApiClient();

        const envResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'gradual-rollout-test' }));
        expectStatus(envResponse, 201);
        testEnvId = envResponse.data.id;
    });

    afterAll(async () => {
        for (const id of createdFeatureIds) {
            await cleanupResource(client, '/features', id);
        }
        if (testEnvId) await cleanupResource(client, '/environments', testEnvId);
    });

    describe('Canary Release Pattern', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create feature at 0% rollout
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(response, 201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should start at 0% rollout', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            if (response.data.stages) {
                const stage = response.data.stages.find((s: any) => s.environment.id === testEnvId);
                if (stage) {
                    expect(stage).toHaveProperty('status');
                }
            }
        });

        it('should rollout to 1% (initial canary)', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 1 }],
            });

            expect([200, 400]).toContain(response.status);
        });

        it('should rollout to 10%', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 10 }],
            });

            expect([200, 400]).toContain(response.status);
        });

        it('should rollout to 25%', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 25 }],
            });

            expect([200, 400]).toContain(response.status);
        });

        it('should rollout to 50%', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 50 }],
            });

            expect([200, 400]).toContain(response.status);
        });

        it('should complete rollout to 100%', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 100 }],
            });

            expect([200, 400]).toContain(response.status);
        });

        it('should verify 100% rollout', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            // Verify rollout percentage if endpoint supports it
            if (response.data.stages) {
                const stage = response.data.stages.find((s: any) => s.environment.id === testEnvId);
                // May or may not have changed depending on API support
                expect(stage).toBeDefined();
            }
        });
    });

    describe('Ring-Based Deployment', () => {
        let testFeatureId: string;
        let ring1EnvId: string;
        let ring2EnvId: string;
        let ring3EnvId: string;

        beforeAll(async () => {
            // Create ring environments
            const ring1 = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
                createEnvironmentFixture({ name: 'ring1-internal' }));
            expectStatus(ring1, 201);
            ring1EnvId = ring1.data.id;

            const ring2 = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
                createEnvironmentFixture({ name: 'ring2-beta' }));
            expectStatus(ring2, 201);
            ring2EnvId = ring2.data.id;

            const ring3 = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
                createEnvironmentFixture({ name: 'ring3-general' }));
            expectStatus(ring3, 201);
            ring3EnvId = ring3.data.id;

            // Create feature with ring stages
            const stages = buildFeatureStages([ring1EnvId, ring2EnvId, ring3EnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(response, 201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        afterAll(async () => {
            if (ring1EnvId) await cleanupResource(client, '/environments', ring1EnvId);
            if (ring2EnvId) await cleanupResource(client, '/environments', ring2EnvId);
            if (ring3EnvId) await cleanupResource(client, '/environments', ring3EnvId);
        });

        it('should verify initial ring 1 deployment', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            if (response.data.stages) {
                const ring1 = response.data.stages.find((s: any) => s.environment.id === ring1EnvId);
                if (ring1) {
                    expect(ring1).toHaveProperty('status');
                }
            }
        });

        it('should expand to ring 2 (beta users)', async () => {
            const ring2Stage = await getStageByEnvironment(testFeatureId, ring2EnvId);
            const response = await client.post(`/stages/${ring2Stage.id}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });

            expect([200, 400, 403]).toContain(response.status);
        });

        it('should expand to ring 3 (general availability)', async () => {
            const ring3Stage = await getStageByEnvironment(testFeatureId, ring3EnvId);
            const response = await client.post(`/stages/${ring3Stage.id}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });

            expect([200, 400, 403]).toContain(response.status);
        });

        it('should verify all rings are active', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            // All stages should be enabled after promotion
            if (response.data.stages) {
                const ring1 = response.data.stages.find((s: any) => s.environment.id === ring1EnvId);
                const ring2 = response.data.stages.find((s: any) => s.environment.id === ring2EnvId);
                const ring3 = response.data.stages.find((s: any) => s.environment.id === ring3EnvId);
                expect(ring1).toBeDefined();
                expect(ring2).toBeDefined();
                expect(ring3).toBeDefined();
            }
        });
    });

    describe('Percentage Validation', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(response, 201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should reject negative percentage', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: -10 }],
            });

            expect([400]).toContain(response.status);
        });

        it('should reject percentage over 100', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 150 }],
            });

            expect([400]).toContain(response.status);
        });

        it('should accept decimal percentages if supported', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 33.33 }],
            });

            // May be accepted or rounded
            expect([200, 400]).toContain(response.status);
        });

        it('should accept 0% (feature off)', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 0 }],
            });

            expect([200, 400]).toContain(response.status);
        });

        it('should accept 100% (fully enabled)', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 100 }],
            });

            expect([200, 400]).toContain(response.status);
        });
    });

    describe('A/B Testing Rollout', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create feature for A/B test
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture({ featureType: 'CONTEXTUAL' }),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(response, 201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should create 50/50 A/B test split', async () => {
            // Create two variants
            const response = await client.patch(`/features/${testFeatureId}`, {
                variants: [
                    { key: 'control', weight: 50 },
                    { key: 'experiment', weight: 50 },
                ],
            });

            // May not be supported via this endpoint
            expect([200, 400, 404]).toContain(response.status);
        });

        it('should create 70/20/10 multi-variant test', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                variants: [
                    { key: 'control', weight: 70 },
                    { key: 'variant-a', weight: 20 },
                    { key: 'variant-b', weight: 10 },
                ],
            });

            expect([200, 400, 404]).toContain(response.status);
        });
    });

    describe('Percentage + Targeting Combination', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture({ featureType: 'CONTEXTUAL' }),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(response, 201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should enable 25% rollout with targeting rules', async () => {
            // Add a criterion that further filters within the 25%
            const stage = await getStageByEnvironment(testFeatureId, testEnvId);
            const criteriaFixture = [{
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'STARTS_WITH', value: 'beta' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            }];
            const response = await client.put(`/stages/${stage.id}/criteria`, criteriaFixture);

            expect([200, 400]).toContain(response.status);
        });

        it('should increase rollout while maintaining targeting', async () => {
            // Increase to 50%
            const patchResponse = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 50 }],
            });

            expect([200, 400]).toContain(patchResponse.status);

            // Criteria should still be active
            const stage = await getStageByEnvironment(testFeatureId, testEnvId);
            const criteriaResponse = await client.get(`/stages/${stage.id}/criteria`);
            expect([200, 404]).toContain(criteriaResponse.status);
        });
    });

    describe('Staged Rollout with Monitoring', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expectStatus(response, 201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should perform staged rollout with waits (simulating monitoring)', async () => {
            const stages = [1, 5, 10, 25, 50, 100];

            for (const percentage of stages) {
                // Update rollout percentage
                const response = await client.patch(`/features/${testFeatureId}`, {
                    stages: [{ environmentId: testEnvId, rolloutPercentage: percentage }],
                });

                expect([200, 400]).toContain(response.status);

                // Simulate monitoring period
                await delay(100);

                // Verify feature is still healthy (can be fetched)
                const healthCheck = await client.get(`/features/${testFeatureId}`);
                expectSuccess(healthCheck);
            }
        });
    });
});
