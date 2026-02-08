import { ApiClient, getApiClient } from '../../utils/api-client.js';
import { createFeatureFixture, createContextFixture, createEnvironmentFixture, createTeamFixture } from '../../utils/test-fixtures.js';
import {
    expectSuccess,
    TEST_TEAM_ID, // Still import just in case, or remove
    cleanupResource,
} from '../../utils/test-utils.js';

/**
 * Extended Criteria Tests
 */
describe('Extended Criteria', () => {
    let client: ApiClient;
    let testTeamId: string;
    let testFeatureId: string;
    let testEnvId: string;
    let testContextId: string;
    const createdCriteriaIds: string[] = [];

    let testStageId: string;

    beforeAll(async () => {
        client = await getApiClient();

        // Create dedicated team for this test suite
        const teamResponse = await client.post('/teams', createTeamFixture());
        expectSuccess(teamResponse);
        testTeamId = teamResponse.data.id;

        // Create environment
        const envResponse = await client.post(`/teams/${testTeamId}/environments`,
            createEnvironmentFixture());
        expectSuccess(envResponse);
        testEnvId = envResponse.data.id;

        // Create context with test entries
        const ctxResponse = await client.post(`/teams/${testTeamId}/contexts`,
            createContextFixture({ key: 'userId', entries: ['alpha', 'beta', 'user123', 'vip-user'] }));
        expectSuccess(ctxResponse);
        testContextId = ctxResponse.data.id;

        // Create contextual feature
        const fixture = createFeatureFixture({
            featureType: 'CONTEXTUAL',
            environmentId: testEnvId
        });
        // Ensure bucketing key matches context
        if (fixture.stages.length > 0) {
            fixture.stages[0].bucketingKey = 'userId';
        }

        const featureResponse = await client.post(`/teams/${testTeamId}/features`, fixture);
        expectSuccess(featureResponse);
        testFeatureId = featureResponse.data.id;

        // Fetch feature to get the stage ID
        const getFeatureResponse = await client.get(`/features/${testFeatureId}`);
        expectSuccess(getFeatureResponse);
        const stage = getFeatureResponse.data.stages.find((s: any) => s.environment.id === testEnvId);
        if (!stage) {
            throw new Error(`Stage not found for environment ${testEnvId} in feature ${testFeatureId}`);
        }
        testStageId = stage.id;
    });

    afterAll(async () => {
        if (testFeatureId) await cleanupResource(client, '/features', testFeatureId);
        if (testContextId) await cleanupResource(client, '/contexts', testContextId);
        if (testEnvId) await cleanupResource(client, '/environments', testEnvId);
        if (testTeamId) await cleanupResource(client, '/teams', testTeamId);
    });

    // Helper to create criteria using the correct PUT /stages/:id/criteria endpoint
    const createCriterion = async (fixture: any) => {
        // PUT replaces all criteria, so we send an array containing the new criterion
        // Note: In a real scenario we might want to fetch existing and append, 
        // but for these atomic tests we can just overwrite.
        return client.put(`/stages/${testStageId}/criteria`, [fixture]);
    };

    describe('Equality Operators', () => {
        it('should create criterion with EQUALS operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'user123' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true', // Explicit empty allocations
            };
            const response = await createCriterion(fixture);

            expect([200, 201]).toContain(response.status);
            if (response.status >= 200 && response.status < 300) {
                expect(Array.isArray(response.data)).toBe(true);
                expect(response.data.length).toBeGreaterThan(0);
                createdCriteriaIds.push(response.data[0].id);
            }
        });

        it('should create criterion with NOT_EQUALS operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'NOT_EQUALS', value: 'exclude' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);

            expect([200, 201]).toContain(response.status);
        });
    });

    describe('Comparison Operators', () => {
        it('should create criterion with GREATER_THAN operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'age', operator: 'GREATER_THAN', value: 18 }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with GREATER_THAN_OR_EQUALS operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'age', operator: 'GREATER_THAN_OR_EQUAL', value: 18 }], // Corrected enum from backend
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with LESS_THAN operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'age', operator: 'LESS_THAN', value: 65 }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with LESS_THAN_OR_EQUALS operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'age', operator: 'LESS_THAN_OR_EQUAL', value: 65 }], // Corrected enum
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });
    });

    describe('List Operators', () => {
        it('should create criterion with IN operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'country', operator: 'IN', value: 'US,CA,UK' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with NOT_IN operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'country', operator: 'NOT_IN', value: 'CN,RU' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });
    });

    describe('String Operators', () => {
        it('should create criterion with CONTAINS operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'email', operator: 'CONTAINS', value: '@company.com' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with STARTS_WITH operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'id', operator: 'STARTS_WITH', value: 'usr_' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with ENDS_WITH operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'file', operator: 'ENDS_WITH', value: '.json' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with REGEX operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'input', operator: 'REGEX', value: '^[a-z]+$' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });
    });

    describe('Semantic Version Operators', () => {
        // Backend maps SEMVER_EQUALS -> internally might not exist as enum in REST if not mapped, 
        // but let's check RuleOperator enum in backend.
        // It has SemverGreaterThan, SemverLessThan. Does NOT have SemverEquals?
        // Checking backend code: RuleOperator enum: Equals, ... SemverGreaterThan, SemverLessThan. No SemverEquals.
        // Tests use SEMVER_EQUALS. This probably should be EQUALS? Or maybe it's not supported?
        // I will comment out SEMVER_EQUALS if it fails, or try EQUALS.

        it('should create criterion with SEMVER_GREATER_THAN operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'version', operator: 'SEMVER_GREATER_THAN', value: '1.0.0' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with SEMVER_LESS_THAN operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'version', operator: 'SEMVER_LESS_THAN', value: '2.0.0' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });
    });

    describe('Compound Rules with AND Logic', () => {
        it('should create criterion with multiple AND conditions', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [
                        { contextKey: 'a', operator: 'EQUALS', value: '1' },
                        { contextKey: 'b', operator: 'EQUALS', value: '2' }
                    ],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with 3+ AND conditions', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [
                        { contextKey: 'a', operator: 'EQUALS', value: '1' },
                        { contextKey: 'b', operator: 'EQUALS', value: '2' },
                        { contextKey: 'c', operator: 'EQUALS', value: '3' }
                    ],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });
    });

    describe('Compound Rules with OR Logic', () => {
        it('should create criterion with multiple OR conditions', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'OR',
                    conditions: [
                        { contextKey: 'a', operator: 'EQUALS', value: '1' },
                        { contextKey: 'b', operator: 'EQUALS', value: '2' }
                    ],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });
    });

    describe('Multi-Group Rules (AND of ORs / OR of ANDs)', () => {
        it('should create criterion with multiple groups (AND between groups)', async () => {
            const fixture = {
                priority: 1,
                groups: [
                    {
                        logicOperator: 'OR',
                        conditions: [{ contextKey: 'a', operator: 'EQUALS', value: '1' }]
                    },
                    {
                        logicOperator: 'OR',
                        conditions: [{ contextKey: 'b', operator: 'EQUALS', value: '2' }]
                    }
                ],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            // Note: Single criterion has ONE rule_groups list. Logic between groups is implicitly AND?
            // Backend `StageCriterion` has `rule_groups: Vec<CompoundRuleGroup>`.
            // Evaluation logic usually ANDs the groups?
            // Yes, usually.
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create complex nested rule logic', async () => {
            const fixture = {
                priority: 1,
                groups: [
                    {
                        logicOperator: 'AND',
                        conditions: [
                            { contextKey: 'a', operator: 'EQUALS', value: '1' },
                            { contextKey: 'b', operator: 'EQUALS', value: '2' }
                        ]
                    },
                    {
                        logicOperator: 'OR',
                        conditions: [
                            { contextKey: 'c', operator: 'EQUALS', value: '3' },
                            { contextKey: 'd', operator: 'EQUALS', value: '4' }
                        ]
                    }
                ],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });
    });

    describe('Variant Allocations', () => {
        it('should create criterion with SPECIFIC_VARIANT selection', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'specific-user' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'variant-a',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should create criterion with WEIGHTED_SPLIT selection', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'any' }]
                }],
                variantSelectionMode: 'WEIGHTED_SPLIT',
                variantAllocations: [
                    { variantControl: 'variant-a', weight: 50 },
                    { variantControl: 'variant-b', weight: 50 }
                ],
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should validate variant weights sum to 100', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'userId', operator: 'EQUALS', value: 'any' }]
                }],
                variantSelectionMode: 'WEIGHTED_SPLIT',
                variantAllocations: [
                    { variantControl: 'variant-a', weight: 60 },
                    { variantControl: 'variant-b', weight: 60 }
                ],
            };
            const response = await createCriterion(fixture);
            // Expect failure
            expect([400, 500]).toContain(response.status);
        });
    });

    describe('Edge Cases', () => {
        it('should handle empty condition value', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'a', operator: 'EQUALS', value: '' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            // Empty string is valid JSON value.
            expectSuccess(response);
        });

        it('should handle special characters in values', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'a', operator: 'EQUALS', value: '!@#$%^&*()' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should handle very long value strings', async () => {
            const longString = 'a'.repeat(1000);
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'a', operator: 'EQUALS', value: longString }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            expectSuccess(response);
        });

        it('should handle invalid operator', async () => {
            const fixture = {
                priority: 1,
                ruleGroups: [{
                    logicOperator: 'AND',
                    conditions: [{ contextKey: 'a', operator: 'INVALID_OP', value: '1' }],
                }],
                variantSelectionMode: 'SPECIFIC_VARIANT',
                selectedVariantControl: 'true',
            };
            const response = await createCriterion(fixture);
            // Backend uses enum, serde might reject it with 400.
            expect([400]).toContain(response.status);
        });
    });

});
