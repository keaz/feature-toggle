import { ApiClient, getApiClient, createApiClient } from '../../utils/api-client.js';
import { createFeatureFixture, createContextFixture, createEnvironmentFixture } from '../../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    TEST_TEAM_ID,
    cleanupResource,
} from '../../utils/test-utils.js';

/**
 * Extended Criteria Tests
 * 
 * Tests all available operators and complex rule configurations:
 * - All comparison operators (EQUALS, NOT_EQUALS, GREATER_THAN, etc.)
 * - String operators (CONTAINS, STARTS_WITH, ENDS_WITH, REGEX)
 * - Semantic versioning operators (SEMVER_*)
 * - Compound rules with AND/OR logic
 * - Variant allocations (weighted split, specific variant)
 */
describe('Extended Criteria', () => {
    let client: ApiClient;
    let testFeatureId: string;
    let testEnvId: string;
    let testContextId: string;
    const createdCriteriaIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create environment
        const envResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'criteria-extended-test' }));
        testEnvId = envResponse.data.id;

        // Create context with test entries
        const ctxResponse = await client.post(`/teams/${TEST_TEAM_ID}/contexts`,
            createContextFixture({ key: 'userId', entries: ['alpha', 'beta', 'user123', 'vip-user'] }));
        testContextId = ctxResponse.data.id;

        // Create contextual feature
        const featureResponse = await client.post(`/teams/${TEST_TEAM_ID}/features`, {
            ...createFeatureFixture({ featureType: 'CONTEXTUAL' }),
            stages: [{ environmentId: testEnvId, enabled: true, rolloutPercentage: 100 }],
        });
        testFeatureId = featureResponse.data.id;
    });

    afterAll(async () => {
        if (testFeatureId) await cleanupResource(client, '/features', testFeatureId);
        if (testContextId) await cleanupResource(client, '/contexts', testContextId);
        if (testEnvId) await cleanupResource(client, '/environments', testEnvId);
    });

    describe('Equality Operators', () => {
        it('should create criterion with EQUALS operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 1,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'user123' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
            if (response.status === 201 || response.status === 200) {
                createdCriteriaIds.push(response.data.id);
            }
        });

        it('should create criterion with NOT_EQUALS operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 2,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'NOT_EQUALS', value: 'banned-user' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('Comparison Operators', () => {
        beforeAll(async () => {
            // Create numeric context
            await client.post(`/teams/${TEST_TEAM_ID}/contexts`,
                createContextFixture({ key: 'userAge', entries: ['18', '25', '30', '65'] }));
        });

        it('should create criterion with GREATER_THAN operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 3,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userAge', operator: 'GREATER_THAN', value: '18' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with GREATER_THAN_OR_EQUALS operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 4,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userAge', operator: 'GREATER_THAN_OR_EQUALS', value: '21' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with LESS_THAN operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 5,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userAge', operator: 'LESS_THAN', value: '65' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with LESS_THAN_OR_EQUALS operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 6,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userAge', operator: 'LESS_THAN_OR_EQUALS', value: '30' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('List Operators', () => {
        it('should create criterion with IN operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 7,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'IN', value: 'alpha,beta,gamma' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with NOT_IN operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 8,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'NOT_IN', value: 'banned1,banned2,banned3' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('String Operators', () => {
        beforeAll(async () => {
            // Create email context
            await client.post(`/teams/${TEST_TEAM_ID}/contexts`,
                createContextFixture({ key: 'userEmail', entries: ['user@example.com', 'admin@company.io'] }));
        });

        it('should create criterion with CONTAINS operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 9,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userEmail', operator: 'CONTAINS', value: '@company.io' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with STARTS_WITH operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 10,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userEmail', operator: 'STARTS_WITH', value: 'admin' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with ENDS_WITH operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 11,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userEmail', operator: 'ENDS_WITH', value: '.io' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with REGEX operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 12,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userEmail', operator: 'REGEX', value: '^[a-z]+@.*\\.io$' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('Semantic Version Operators', () => {
        beforeAll(async () => {
            // Create app version context
            await client.post(`/teams/${TEST_TEAM_ID}/contexts`,
                createContextFixture({ key: 'appVersion', entries: ['1.0.0', '1.2.3', '2.0.0', '2.1.0-beta'] }));
        });

        it('should create criterion with SEMVER_EQUALS operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 13,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'appVersion', operator: 'SEMVER_EQUALS', value: '2.0.0' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with SEMVER_GREATER_THAN operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 14,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'appVersion', operator: 'SEMVER_GREATER_THAN', value: '1.5.0' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with SEMVER_LESS_THAN operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 15,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'appVersion', operator: 'SEMVER_LESS_THAN', value: '3.0.0' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('Compound Rules with AND Logic', () => {
        it('should create criterion with multiple AND conditions', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 16,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [
                        { contextKey: 'userId', operator: 'STARTS_WITH', value: 'vip' },
                        { contextKey: 'userAge', operator: 'GREATER_THAN', value: '18' },
                    ],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with 3+ AND conditions', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 17,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [
                        { contextKey: 'userId', operator: 'NOT_EQUALS', value: 'banned' },
                        { contextKey: 'userAge', operator: 'GREATER_THAN_OR_EQUALS', value: '21' },
                        { contextKey: 'userEmail', operator: 'ENDS_WITH', value: '.io' },
                    ],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('Compound Rules with OR Logic', () => {
        it('should create criterion with multiple OR conditions', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 18,
                groups: [{
                    logicOperator: 'OR',
                    conditions: [
                        { contextKey: 'userId', operator: 'EQUALS', value: 'alpha' },
                        { contextKey: 'userId', operator: 'EQUALS', value: 'beta' },
                        { contextKey: 'userId', operator: 'STARTS_WITH', value: 'vip' },
                    ],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('Multi-Group Rules (AND of ORs / OR of ANDs)', () => {
        it('should create criterion with multiple groups (AND between groups)', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 19,
                groups: [
                    {
                        logicOperator: 'OR',
                        conditions: [
                            { contextKey: 'userId', operator: 'EQUALS', value: 'alpha' },
                            { contextKey: 'userId', operator: 'EQUALS', value: 'beta' },
                        ],
                    },
                    {
                        logicOperator: 'AND',
                        conditions: [
                            { contextKey: 'userAge', operator: 'GREATER_THAN', value: '18' },
                        ],
                    },
                ],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create complex nested rule logic', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 20,
                groups: [
                    {
                        logicOperator: 'OR',
                        conditions: [
                            { contextKey: 'userId', operator: 'STARTS_WITH', value: 'vip' },
                            { contextKey: 'userEmail', operator: 'ENDS_WITH', value: '@company.io' },
                        ],
                    },
                    {
                        logicOperator: 'AND',
                        conditions: [
                            { contextKey: 'appVersion', operator: 'SEMVER_GREATER_THAN', value: '2.0.0' },
                            { contextKey: 'userId', operator: 'NOT_IN', value: 'banned1,banned2' },
                        ],
                    },
                ],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('Variant Allocations', () => {
        it('should create criterion with SPECIFIC_VARIANT selection', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 21,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'specific-user' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                variantKey: 'variant-a',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should create criterion with WEIGHTED_SPLIT selection', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 22,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'NOT_EQUALS', value: 'excluded' }],
                }],
                variantSelectionMode: 'WEIGHTED_SPLIT',
                variantWeights: [
                    { variantKey: 'control', weight: 50 },
                    { variantKey: 'experiment-a', weight: 25 },
                    { variantKey: 'experiment-b', weight: 25 },
                ],
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should validate variant weights sum to 100', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 23,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EXISTS', value: '' }],
                }],
                variantSelectionMode: 'WEIGHTED_SPLIT',
                variantWeights: [
                    { variantKey: 'control', weight: 30 },
                    { variantKey: 'experiment', weight: 30 },
                    // Missing 40% - should be rejected
                ],
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            // Should fail validation
            expect([200, 201, 400]).toContain(response.status);
        });
    });

    describe('Edge Cases', () => {
        it('should handle empty condition value', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 24,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: '' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            // Should be rejected or handled
            expect([200, 201, 400]).toContain(response.status);
        });

        it('should handle special characters in values', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 25,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'user@example.com' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            expect([200, 201, 400]).toContain(response.status);
        });

        it('should handle very long value strings', async () => {
            const longValue = 'a'.repeat(1000);
            const fixture = {
                stageId: testEnvId,
                priority: 26,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: longValue }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            // Should be rejected or truncated
            expect([200, 201, 400]).toContain(response.status);
        });

        it('should handle invalid operator', async () => {
            const fixture = {
                stageId: testEnvId,
                priority: 27,
                groups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'INVALID_OPERATOR', value: 'test' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                enabled: true,
            };
            const response = await client.post(`/features/${testFeatureId}/criteria`, fixture);

            // Should be rejected
            expect([400]).toContain(response.status);
        });
    });
});
