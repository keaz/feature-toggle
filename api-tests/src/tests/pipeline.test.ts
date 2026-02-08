import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createPipelineFixture, createEnvironmentFixture, createTeamFixture } from '../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    expectPaginatedResponse,
    expectUuid,
    cleanupResource,
} from '../utils/test-utils.js';

function buildLinearRelationships(stageCount: number): Array<{ sourceId: number; targetId: number }> {
    return Array.from({ length: Math.max(0, stageCount - 1) }, (_, index) => ({
        sourceId: index,
        targetId: index + 1,
    }));
}

function buildPipelineStages(
    environmentIds: string[]
): Array<{ environmentId: string; orderIndex: number; position: string }> {
    return environmentIds.map((environmentId, index) => ({
        environmentId,
        orderIndex: index,
        position: String(index + 1),
    }));
}

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
    let testTeamId: string;
    let testEnvironmentIds: string[] = [];
    const createdIds: string[] = [];
    const defaultStageEnvIds = () => testEnvironmentIds.slice(0, 3);
    const buildValidPipelinePayload = (name?: string) => {
        const stages = buildPipelineStages(defaultStageEnvIds());
        return {
            name: name ?? createPipelineFixture().name,
            stages,
            relationships: buildLinearRelationships(stages.length),
        };
    };

    const updatePipeline = async (
        pipelineId: string,
        updates: Partial<{
            name: string;
            active: boolean;
            stages: Array<{ environmentId: string; orderIndex: number; position: string }>;
            relationships: Array<{ sourceId: number; targetId: number }>;
        }>
    ) => {
        const currentResponse = await client.get(`/pipelines/${pipelineId}`);
        expectSuccess(currentResponse);

        const current = currentResponse.data;
        const payload = {
            name: current.name,
            active: current.active,
            stages: current.stages.map((stage: any) => ({
                environmentId: stage.environment.id,
                orderIndex: stage.orderIndex,
                position: stage.position,
            })),
            relationships: (current.relationships || []).map((rel: any) => ({
                sourceId: rel.sourceId,
                targetId: rel.targetId,
            })),
            ...updates,
        };

        return client.patch(`/pipelines/${pipelineId}`, payload);
    };

    beforeAll(async () => {
        client = await getApiClient();

        const teamResponse = await client.post('/teams', createTeamFixture());
        expectStatus(teamResponse, 201);
        testTeamId = teamResponse.data.id;

        // Create test environments for pipeline stages
        for (let i = 0; i < 3; i++) {
            const envFixture = createEnvironmentFixture();
            const envResponse = await client.post(`/teams/${testTeamId}/environments`, envFixture);
            expectStatus(envResponse, 201);
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
            const response = await client.get(`/teams/${testTeamId}/pipelines`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination', async () => {
            const response = await client.get(`/teams/${testTeamId}/pipelines`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should filter by active status', async () => {
            const response = await client.get(`/teams/${testTeamId}/pipelines`, {
                active: true,
            });

            expectSuccess(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${testTeamId}/pipelines`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${testTeamId}/pipelines`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/pipelines');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/pipelines', () => {
        it('should create a pipeline', async () => {
            const fixture = buildValidPipelinePayload();
            const response = await client.post(`/teams/${testTeamId}/pipelines`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);

            createdIds.push(response.data.id);
        });

        it('should create pipeline with environment stages', async () => {
            const fixture = {
                ...buildValidPipelinePayload(),
            };
            const response = await client.post(`/teams/${testTeamId}/pipelines`, fixture);

            expectStatus(response, 201);
            expect(response.data.stages).toHaveLength(testEnvironmentIds.length);

            createdIds.push(response.data.id);
        });

        it('should reject duplicate pipeline names', async () => {
            const fixture = buildValidPipelinePayload();

            // Create first pipeline
            const first = await client.post(`/teams/${testTeamId}/pipelines`, fixture);
            expectStatus(first, 201);
            createdIds.push(first.data.id);

            // Try to create duplicate
            const duplicate = await client.post(`/teams/${testTeamId}/pipelines`, fixture);
            expectStatus(duplicate, 409); // Conflict
        });

        it('should reject empty name', async () => {
            const fixture = buildValidPipelinePayload('');
            const response = await client.post(`/teams/${testTeamId}/pipelines`, {
                ...fixture,
            });

            expectClientError(response);
        });

        it('should reject pipeline without stages', async () => {
            const response = await client.post(`/teams/${testTeamId}/pipelines`, {
                name: createPipelineFixture().name,
                stages: [],
                relationships: [],
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = buildValidPipelinePayload();
            const response = await unauthClient.post(`/teams/${testTeamId}/pipelines`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /pipelines/{id}', () => {
        let testPipelineId: string;

        beforeAll(async () => {
            // Create a test pipeline
            const fixture = buildValidPipelinePayload();
            const response = await client.post(`/teams/${testTeamId}/pipelines`, fixture);
            expectStatus(response, 201);
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
            const fixture = buildValidPipelinePayload();
            const response = await client.post(`/teams/${testTeamId}/pipelines`, fixture);
            expectStatus(response, 201);
            testPipelineId = response.data.id;
            createdIds.push(testPipelineId);
        });

        it('should update pipeline name', async () => {
            const newName = createPipelineFixture().name;
            const response = await updatePipeline(testPipelineId, {
                name: newName,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
        });

        it('should update pipeline active status', async () => {
            const response = await updatePipeline(testPipelineId, {
                active: false,
            });

            expectSuccess(response);
            expect(response.data.active).toBe(false);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const fixture = buildValidPipelinePayload('new-pipeline-name');
            const response = await client.patch(`/pipelines/${fakeId}`, {
                name: fixture.name,
                active: true,
                stages: fixture.stages,
                relationships: fixture.relationships,
            });

            expectStatus(response, 404);
        });

        it('should reject empty name update', async () => {
            const response = await updatePipeline(testPipelineId, {
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
