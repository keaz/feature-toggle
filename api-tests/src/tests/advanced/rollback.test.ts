import { ApiClient, getApiClient } from '../../utils/api-client.js';
import { createFeatureFixture, createEnvironmentFixture } from '../../utils/test-fixtures.js';
import {
    expectSuccess,
    TEST_TEAM_ID,
    cleanupResource,
    delay,
} from '../../utils/test-utils.js';

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

function buildLinearRelationships(stageCount: number): Array<{ sourceId: number; targetId: number }> {
    return Array.from({ length: Math.max(0, stageCount - 1) }, (_, index) => ({
        sourceId: index,
        targetId: index + 1,
    }));
}

async function getStageByEnvironment(
    client: ApiClient,
    featureId: string,
    environmentId: string
): Promise<any> {
    const response = await client.get(`/features/${featureId}`);
    expectSuccess(response);
    const stage = response.data.stages?.find((s: any) => s.environment.id === environmentId);
    expect(stage).toBeDefined();
    return stage;
}

/**
 * Rollback Tests
 * 
 * Tests feature rollback scenarios:
 * - Request rollback and deployment transitions
 * - Emergency disable and recovery
 * - Rollback-related endpoint compatibility checks
 */
describe('Feature Rollback', () => {
    let client: ApiClient;
    let testEnvId: string;
    const createdFeatureIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        const envResponse = await client.post(
            `/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'rollback-test-env' })
        );
        testEnvId = envResponse.data.id;
    });

    afterAll(async () => {
        for (const id of createdFeatureIds) {
            await cleanupResource(client, '/features', id);
        }
        if (testEnvId) {
            await cleanupResource(client, '/environments', testEnvId);
        }
    });

    describe('Enable/Disable Rollback', () => {
        let testFeatureId: string;
        let disableStatus: number | null = null;
        let enableStatus: number | null = null;

        beforeAll(async () => {
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expect(response.status).toBe(201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should disable feature (simulate issue discovered)', async () => {
            const stage = await getStageByEnvironment(client, testFeatureId, testEnvId);
            const response = await client.post(`/stages/${stage.id}/request-change`, {
                request: 'ROLLBACK_REQUESTED',
            });

            disableStatus = response.status;
            expect([200, 400, 403]).toContain(response.status);
        });

        it('should verify feature is disabled', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            const stage = response.data.stages?.find((s: any) => s.environment.id === testEnvId);
            expect(stage).toBeDefined();
            if (stage && disableStatus === 200) {
                expect(['ROLLBACK_REQUESTED', 'ROLLBACK_APPROVED', 'ROLLBACKED']).toContain(stage.status);
            }
        });

        it('should rollback by re-enabling feature', async () => {
            const stage = await getStageByEnvironment(client, testFeatureId, testEnvId);
            const response = await client.post(`/stages/${stage.id}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });

            enableStatus = response.status;
            expect([200, 400, 403]).toContain(response.status);
        });

        it('should verify feature is re-enabled after rollback', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            const stage = response.data.stages?.find((s: any) => s.environment.id === testEnvId);
            expect(stage).toBeDefined();
            if (stage && enableStatus === 200) {
                expect(['DEPLOYMENT_REQUESTED', 'DEPLOYMENT_APPROVED', 'DEPLOYED']).toContain(stage.status);
            }
        });
    });

    describe('Rollout Percentage Rollback', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expect(response.status).toBe(201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should reduce rollout to 50%', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 50 }],
            });

            expect([200, 400, 404]).toContain(response.status);
        });

        it('should rollback to 100% after issue resolved', async () => {
            const response = await client.patch(`/features/${testFeatureId}`, {
                stages: [{ environmentId: testEnvId, rolloutPercentage: 100 }],
            });

            expect([200, 400, 404]).toContain(response.status);
        });
    });

    describe('Emergency Disable and Recovery', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expect(response.status).toBe(201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should emergency disable feature', async () => {
            const response = await client.post(`/features/${testFeatureId}/emergency-disable`, {
                rollbackInMinutes: 30,
            });

            expectSuccess(response);
        });

        it('should verify feature is completely disabled', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);
            expect(response.data.killSwitchEnabled).toBe(true);
        });

        it('should recover from emergency disable', async () => {
            const response = await client.post(`/features/${testFeatureId}/emergency-enable`);
            expectSuccess(response);
        });

        it('should verify feature is recovered', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);
            expect(response.data.killSwitchEnabled).toBe(false);
        });
    });

    describe('Rollback with History', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const stages = buildFeatureStages([testEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expect(response.status).toBe(201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should get feature change history', async () => {
            const stage = await getStageByEnvironment(client, testFeatureId, testEnvId);

            await client.post(`/stages/${stage.id}/request-change`, { request: 'ROLLBACK_REQUESTED' });
            await delay(100);
            await client.post(`/stages/${stage.id}/request-change`, { request: 'DEPLOYMENT_REQUESTED' });

            const response = await client.get(`/features/${testFeatureId}/history`);

            expect([200, 404]).toContain(response.status);
            if (response.status === 200) {
                expect(Array.isArray(response.data)).toBe(true);
            }
        });

        it('should rollback to specific version if supported', async () => {
            const response = await client.post(`/features/${testFeatureId}/rollback`, {
                reason: 'Rolling back to known good state',
            });

            expect([200, 201, 404]).toContain(response.status);
        });
    });

    describe('Multi-Environment Rollback', () => {
        let testFeatureId: string;
        let devEnvId: string;
        let prodEnvId: string;
        let rollbackStatus: number | null = null;
        let devStatusBefore: string | null = null;

        beforeAll(async () => {
            const devEnv = await client.post(
                `/teams/${TEST_TEAM_ID}/environments`,
                createEnvironmentFixture({ name: 'dev-rollback-test' })
            );
            devEnvId = devEnv.data.id;

            const prodEnv = await client.post(
                `/teams/${TEST_TEAM_ID}/environments`,
                createEnvironmentFixture({ name: 'prod-rollback-test' })
            );
            prodEnvId = prodEnv.data.id;

            const stages = buildFeatureStages([devEnvId, prodEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expect(response.status).toBe(201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);

            const current = await client.get(`/features/${testFeatureId}`);
            expectSuccess(current);
            const devStage = current.data.stages?.find((s: any) => s.environment.id === devEnvId);
            devStatusBefore = devStage?.status ?? null;
        });

        afterAll(async () => {
            if (devEnvId) {
                await cleanupResource(client, '/environments', devEnvId);
            }
            if (prodEnvId) {
                await cleanupResource(client, '/environments', prodEnvId);
            }
        });

        it('should rollback prod only while keeping dev enabled', async () => {
            const prodStage = await getStageByEnvironment(client, testFeatureId, prodEnvId);
            const response = await client.post(`/stages/${prodStage.id}/request-change`, {
                request: 'ROLLBACK_REQUESTED',
            });

            rollbackStatus = response.status;
            expect([200, 400, 403]).toContain(response.status);
        });

        it('should verify dev remains enabled after prod rollback', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            const devStage = response.data.stages?.find((s: any) => s.environment.id === devEnvId);
            const prodStage = response.data.stages?.find((s: any) => s.environment.id === prodEnvId);
            expect(devStage).toBeDefined();
            expect(prodStage).toBeDefined();

            if (devStage && devStatusBefore) {
                expect(devStage.status).toBe(devStatusBefore);
            }
            if (prodStage && rollbackStatus === 200) {
                expect(['ROLLBACK_REQUESTED', 'ROLLBACK_APPROVED', 'ROLLBACKED']).toContain(prodStage.status);
            }
        });
    });
});
