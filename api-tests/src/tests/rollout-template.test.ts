import { ApiClient, createApiClient, getApiClient } from '../utils/api-client.js';
import {
    createEnvironmentFixture,
    createFeatureFixture,
    createTeamFixture,
    uniqueName,
} from '../utils/test-fixtures.js';
import { cleanupResource, expectClientError, expectStatus, expectSuccess } from '../utils/test-utils.js';

function linearRelationships(stageCount: number) {
    return Array.from({ length: Math.max(0, stageCount - 1) }, (_, index) => ({
        sourceId: index,
        targetId: index + 1,
    }));
}

describe('Rollout Template API', () => {
    let client: ApiClient;
    let teamId: string;
    let otherTeamId: string;
    let environmentIds: string[] = [];
    let createdFeatureIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        const teamResponse = await client.post('/teams', createTeamFixture());
        expectSuccess(teamResponse);
        teamId = teamResponse.data.id;

        const otherTeamResponse = await client.post('/teams', createTeamFixture());
        expectSuccess(otherTeamResponse);
        otherTeamId = otherTeamResponse.data.id;

        const envTypes = ['Development', 'Staging', 'Production'];
        for (const environmentType of envTypes) {
            const envResponse = await client.post(
                `/teams/${teamId}/environments`,
                createEnvironmentFixture({ environmentType })
            );
            expectSuccess(envResponse);
            environmentIds.push(envResponse.data.id);
        }
    });

    afterAll(async () => {
        for (const featureId of createdFeatureIds) {
            await cleanupResource(client, '/features', featureId);
        }
        for (const environmentId of environmentIds) {
            await cleanupResource(client, '/environments', environmentId);
        }
        if (teamId) {
            await cleanupResource(client, '/teams', teamId);
        }
        if (otherTeamId) {
            await cleanupResource(client, '/teams', otherTeamId);
        }
    });

    it('lists built-in system templates for a team', async () => {
        const response = await client.get(`/teams/${teamId}/rollout-templates`);
        expectSuccess(response);

        const ids = response.data.items.map((template: any) => template.id);
        expect(ids).toEqual(expect.arrayContaining([
            'simple_on_off',
            'canary_10_50_100',
            'approval_gated_production',
            'experiment_rollout',
            'kill_switch_guarded',
        ]));
    });

    it('previews a canary template and returns validation errors before save', async () => {
        const validPreview = await client.post(`/teams/${teamId}/rollout-templates/preview`, {
            templateId: 'canary_10_50_100',
            variables: {
                environmentIds,
            },
        });
        expectSuccess(validPreview);
        expect(validPreview.data.stages).toHaveLength(3);
        expect(validPreview.data.relationships).toEqual(linearRelationships(3));
        expect(validPreview.data.variables.percentages).toEqual([10, 50, 100]);
        expect(validPreview.data.validationErrors).toEqual([]);

        const invalidPreview = await client.post(`/teams/${teamId}/rollout-templates/preview`, {
            templateId: 'canary_10_50_100',
            variables: {
                environmentIds: [environmentIds[0], environmentIds[0], environmentIds[2]],
            },
        });
        expectSuccess(invalidPreview);
        expect(invalidPreview.data.validationErrors.length).toBeGreaterThan(0);
    });

    it('creates custom templates scoped to the owning team', async () => {
        const response = await client.post(`/teams/${teamId}/rollout-templates`, {
            name: uniqueName('release-template'),
            description: 'Custom release template from API test',
            config: {
                stages: environmentIds.slice(0, 2).map((environmentId, index) => ({
                    environmentId,
                    orderIndex: index,
                    position: JSON.stringify({ x: index * 220, y: 80 }),
                })),
                relationships: linearRelationships(2),
                variables: {
                    environmentIds: environmentIds.slice(0, 2),
                    approvalRequired: true,
                    schedule: 'manual',
                },
            },
        });
        expectStatus(response, 201);
        expect(response.data.isSystem).toBe(false);
        expect(response.data.teamId).toBe(teamId);

        const teamList = await client.get(`/teams/${teamId}/rollout-templates`);
        expectSuccess(teamList);
        expect(teamList.data.items.some((template: any) => template.id === response.data.id)).toBe(true);

        const otherTeamList = await client.get(`/teams/${otherTeamId}/rollout-templates`);
        expectSuccess(otherTeamList);
        expect(otherTeamList.data.items.some((template: any) => template.id === response.data.id)).toBe(false);

        const crossTeamPreview = await client.post(`/teams/${otherTeamId}/rollout-templates/preview`, {
            templateId: response.data.id,
            variables: { environmentIds: environmentIds.slice(0, 2) },
        });
        expectStatus(crossTeamPreview, 404);
    });

    it('allows preview output to be applied to feature creation', async () => {
        const preview = await client.post(`/teams/${teamId}/rollout-templates/preview`, {
            templateId: 'approval_gated_production',
            variables: {
                environmentIds,
            },
        });
        expectSuccess(preview);
        expect(preview.data.validationErrors).toEqual([]);

        const featureResponse = await client.post(`/teams/${teamId}/features`, {
            ...createFeatureFixture(),
            key: uniqueName('templated-feature'),
            stages: preview.data.stages,
            relationships: preview.data.relationships,
        });
        expectStatus(featureResponse, 201);
        createdFeatureIds.push(featureResponse.data.id);
        expect(featureResponse.data.stages).toHaveLength(3);
    });

    it('requires authentication for rollout template endpoints', async () => {
        const unauthClient = createApiClient();
        const response = await unauthClient.post(`/teams/${teamId}/rollout-templates`, {
            name: uniqueName('unauth-template'),
            config: {
                stages: [],
                relationships: [],
            },
        });
        expectClientError(response);
        expect(response.status).toBe(401);
    });
});
