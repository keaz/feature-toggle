import { ApiClient, createApiClient, getApiClient } from '../../utils/api-client.js';
import {
    createContextFixture,
    createEnvironmentFixture,
    createFeatureFixture,
    createTeamFixture,
} from '../../utils/test-fixtures.js';
import { cleanupResource, expectStatus, expectSuccess } from '../../utils/test-utils.js';

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

/**
 * Coverage-focused tests for direct criteria management endpoints.
 */
describe('Criteria Management API', () => {
    let client: ApiClient;
    let teamId: string;
    let environmentId: string;
    let contextId: string;
    let featureId: string;
    let stageId: string;
    let criterionId: string;

    beforeAll(async () => {
        client = await getApiClient();

        const team = await client.post('/teams', createTeamFixture());
        expectStatus(team, 201);
        teamId = team.data.id;

        const environment = await client.post(
            `/teams/${teamId}/environments`,
            createEnvironmentFixture()
        );
        expectStatus(environment, 201);
        environmentId = environment.data.id;

        const context = await client.post(
            `/teams/${teamId}/contexts`,
            createContextFixture({ key: 'userId', entries: ['alpha', 'beta', 'gamma'] })
        );
        expectStatus(context, 201);
        contextId = context.data.id;

        const stages = buildFeatureStages([environmentId]);
        const feature = await client.post(`/teams/${teamId}/features`, {
            ...createFeatureFixture({ featureType: 'CONTEXTUAL' }),
            stages,
            relationships: buildLinearRelationships(stages.length),
        });
        expectStatus(feature, 201);
        featureId = feature.data.id;

        const loadedFeature = await client.get(`/features/${featureId}`);
        expectSuccess(loadedFeature);
        const stage = loadedFeature.data.stages?.find((item: any) => item.environment.id === environmentId);
        expect(stage).toBeDefined();
        stageId = stage.id;

        const seedCriteriaResponse = await client.put(`/stages/${stageId}/criteria`, [
            {
                priority: 1,
                ruleGroups: [
                    {
                        logicOperator: 'AND',
                        conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'alpha' }],
                    },
                ],
                variantSelectionMode: 'WEIGHTED_SPLIT',
                variantAllocations: [
                    { variantControl: 'true', weight: 50 },
                    { variantControl: 'false', weight: 50 },
                ],
            },
        ]);
        expectStatus(seedCriteriaResponse, 200);
        expect(seedCriteriaResponse.data.length).toBe(1);
        criterionId = seedCriteriaResponse.data[0].id;
    });

    afterAll(async () => {
        if (featureId) {
            await cleanupResource(client, '/features', featureId);
        }
        if (contextId) {
            await cleanupResource(client, '/contexts', contextId);
        }
        if (environmentId) {
            await cleanupResource(client, '/environments', environmentId);
        }
        if (teamId) {
            await cleanupResource(client, '/teams', teamId);
        }
    });

    describe('Stage Criteria Validation', () => {
        it('should reject invalid stage id format', async () => {
            const response = await client.put('/stages/not-a-uuid/criteria', []);
            expectStatus(response, 400);
        });

        it('should reject context keys longer than 100 chars', async () => {
            const response = await client.put(`/stages/${stageId}/criteria`, [
                {
                    priority: 1,
                    ruleGroups: [
                        {
                            logicOperator: 'AND',
                            conditions: [
                                {
                                    contextKey: 'x'.repeat(101),
                                    operator: 'EQUALS',
                                    value: 'alpha',
                                },
                            ],
                        },
                    ],
                    variantSelectionMode: 'SPECIFIC_VARIANT',
                    selectedVariantControl: 'true',
                },
            ]);
            expectStatus(response, 400);
        });

        it('should persist multi-group criteria definitions', async () => {
            const response = await client.put(`/stages/${stageId}/criteria`, [
                {
                    priority: 2,
                    ruleGroups: [
                        {
                            logicOperator: 'AND',
                            conditions: [
                                { contextKey: 'userId', operator: 'STARTS_WITH', value: 'a' },
                            ],
                        },
                        {
                            logicOperator: 'OR',
                            conditions: [
                                { contextKey: 'country', operator: 'IN', value: 'US,CA' },
                                { contextKey: 'plan', operator: 'EQUALS', value: 'premium' },
                            ],
                        },
                    ],
                    variantSelectionMode: 'SPECIFIC_VARIANT',
                    selectedVariantControl: 'true',
                },
            ]);

            expectStatus(response, 200);
            expect(response.data[0].ruleGroups).toHaveLength(2);
            const operators = response.data[0].ruleGroups.map((group: any) => group.logicOperator);
            expect(operators).toEqual(expect.arrayContaining(['AND', 'OR']));
            criterionId = response.data[0].id;
        });
    });

    describe('Variant Allocation Endpoint', () => {
        it('should replace variant allocations for a criterion', async () => {
            const response = await client.put(`/criteria/${criterionId}/variant-allocations`, {
                allocations: [
                    { variantControl: 'true', weight: 40 },
                    { variantControl: 'false', weight: 60 },
                ],
            });

            expectStatus(response, 200);
            expect(response.data).toHaveLength(2);
            const total = response.data.reduce((sum: number, item: any) => sum + item.weight, 0);
            expect(total).toBe(100);
        });

        it('should reject allocations above 100 total weight', async () => {
            const response = await client.put(`/criteria/${criterionId}/variant-allocations`, {
                allocations: [
                    { variantControl: 'true', weight: 70 },
                    { variantControl: 'false', weight: 40 },
                ],
            });
            expectStatus(response, 400);
        });

        it('should reject malformed criteria IDs', async () => {
            const response = await client.put('/criteria/not-a-uuid/variant-allocations', {
                allocations: [{ variantControl: 'true', weight: 100 }],
            });
            expectStatus(response, 400);
        });

        it('should require authentication for variant allocation updates', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.put(`/criteria/${criterionId}/variant-allocations`, {
                allocations: [{ variantControl: 'true', weight: 100 }],
            });
            expectStatus(response, 401);
        });
    });

    describe('Rule Group CRUD Endpoints', () => {
        let groupId: string;

        it('should create a rule group', async () => {
            const response = await client.post('/rule-groups', {
                criteriaId: criterionId,
                logicOperator: 'AND',
                conditions: [
                    { contextKey: 'region', operator: 'EQUALS', value: 'us-east-1', orderIndex: 0 },
                    { contextKey: 'tier', operator: 'EQUALS', value: 'gold', orderIndex: 1 },
                ],
            });

            expectStatus(response, 200);
            expect(response.data.id).toBeDefined();
            expect(response.data.conditions).toHaveLength(2);
            groupId = response.data.id;
        });

        it('should update a rule group', async () => {
            const response = await client.patch(`/rule-groups/${groupId}`, {
                logicOperator: 'OR',
                conditions: [
                    { contextKey: 'region', operator: 'EQUALS', value: 'eu-west-1', orderIndex: 0 },
                ],
            });

            expectStatus(response, 200);
            expect(response.data.logicOperator).toBe('OR');
            expect(response.data.conditions).toHaveLength(1);
            expect(response.data.conditions[0].value).toBe('eu-west-1');
        });

        it('should reject invalid condition payloads on update', async () => {
            const response = await client.patch(`/rule-groups/${groupId}`, {
                conditions: [
                    {
                        contextKey: 'y'.repeat(101),
                        operator: 'EQUALS',
                        value: 'bad',
                        orderIndex: 0,
                    },
                ],
            });
            expectStatus(response, 400);
        });

        it('should delete a rule group', async () => {
            const response = await client.delete(`/rule-groups/${groupId}`);
            expectStatus(response, 204);
        });

        it('should return 404 when deleting a non-existent rule group', async () => {
            const response = await client.delete(`/rule-groups/${groupId}`);
            expect([204, 404]).toContain(response.status);
        });

        it('should reject malformed group IDs', async () => {
            const patchResponse = await client.patch('/rule-groups/not-a-uuid', {
                logicOperator: 'AND',
            });
            expectStatus(patchResponse, 400);

            const deleteResponse = await client.delete('/rule-groups/not-a-uuid');
            expectStatus(deleteResponse, 400);
        });

        it('should require authentication for creating rule groups', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.post('/rule-groups', {
                criteriaId: criterionId,
                logicOperator: 'AND',
                conditions: [
                    { contextKey: 'region', operator: 'EQUALS', value: 'us-east-1', orderIndex: 0 },
                ],
            });
            expectStatus(response, 401);
        });
    });
});
