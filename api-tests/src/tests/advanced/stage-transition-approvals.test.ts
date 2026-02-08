import { ApiClient, createApiClient, getApiClient } from '../../utils/api-client.js';
import {
    createApprovalPolicyFixture,
    createEnvironmentFixture,
    createFeatureFixture,
    createTeamFixture,
    createUserFixture,
} from '../../utils/test-fixtures.js';
import {
    TEST_TEAM_ID,
    cleanupResource,
    delay,
    expectStatus,
    expectSuccess,
    expectUuid,
} from '../../utils/test-utils.js';

const APPROVER_ROLE_ID = '00000000-0000-0000-0000-000000000001';
const REQUESTER_ROLE_ID = '00000000-0000-0000-0000-000000000002';

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

async function getStage(client: ApiClient, featureId: string, environmentId: string) {
    const feature = await client.get(`/features/${featureId}`);
    expectSuccess(feature);
    const stage = feature.data.stages?.find((item: any) => item.environment.id === environmentId);
    expect(stage).toBeDefined();
    return stage;
}

async function getStageStatus(client: ApiClient, featureId: string, environmentId: string): Promise<string> {
    const stage = await getStage(client, featureId, environmentId);
    return stage.status;
}

async function expectStageStatusEventually(
    client: ApiClient,
    featureId: string,
    environmentId: string,
    allowedStatuses: string[]
): Promise<void> {
    let lastStatus = '';
    for (let i = 0; i < 12; i++) {
        lastStatus = await getStageStatus(client, featureId, environmentId);
        if (allowedStatuses.includes(lastStatus)) {
            return;
        }
        await delay(80);
    }
    expect(allowedStatuses).toContain(lastStatus);
}

async function findLatestPendingRequestId(
    client: ApiClient,
    teamId: string,
    featureId: string
): Promise<string> {
    const response = await client.get(`/teams/${teamId}/approval-requests`, {
        statuses: 'pending',
        offset: 0,
        limit: 100,
    });
    expectSuccess(response);

    const related = (response.data.items || []).filter((item: any) => item.featureId === featureId);
    expect(related.length).toBeGreaterThan(0);
    related.sort(
        (a: any, b: any) =>
            new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
    );
    return related[0].id;
}

async function createAndLoginUserWithRoles(
    adminClient: ApiClient,
    teamId: string,
    roleIds: string[],
    usernamePrefix: string
): Promise<{ userId: string; username: string; password: string; client: ApiClient }> {
    const fixture = createUserFixture({
        username: `${usernamePrefix}-${Date.now()}-${Math.random().toString(16).slice(2, 6)}`,
    });

    const createResponse = await adminClient.post('/users', {
        ...fixture,
        isTemporaryPassword: false,
    });
    expectStatus(createResponse, 201);
    const userId = createResponse.data.id;

    const teamAssign = await adminClient.post(`/users/${userId}/teams`, {
        teamIds: [teamId],
    });
    expectSuccess(teamAssign);

    if (roleIds.length > 0) {
        const roleAssign = await adminClient.post(`/users/${userId}/roles`, {
            roleIds,
        });
        expectSuccess(roleAssign);
    }

    const userClient = createApiClient({
        username: fixture.username,
        password: fixture.password,
    });
    await userClient.authenticate();

    return {
        userId,
        username: fixture.username,
        password: fixture.password,
        client: userClient,
    };
}

/**
 * Coverage-focused tests for stage transitions with approval gates.
 */
describe('Stage Transitions With Approvals', () => {
    let adminClient: ApiClient;
    let teamId: string;
    let prodEnvId: string;
    let featureId: string;
    let stageId: string;
    let policyId: string;

    let requesterClient: ApiClient;
    let approverClient: ApiClient;
    let viewerClient: ApiClient;

    let deploymentRequestId: string;
    let rollbackRequestId: string;
    let secondRollbackRequestId: string;

    beforeAll(async () => {
        adminClient = await getApiClient();

        const team = await adminClient.post('/teams', createTeamFixture());
        expectStatus(team, 201);
        teamId = team.data.id;

        const environment = await adminClient.post(
            `/teams/${teamId}/environments`,
            createEnvironmentFixture({
                name: 'prod-stage-transition',
                environmentType: 'Production',
            })
        );
        expectStatus(environment, 201);
        prodEnvId = environment.data.id;

        const policyFixture = createApprovalPolicyFixture({
            appliesTo: 'production_only',
            environmentIds: [prodEnvId],
            requiredApprovers: 1,
            approverRoleIds: [APPROVER_ROLE_ID],
        });
        const policy = await adminClient.post(`/teams/${teamId}/approval-policies`, policyFixture);
        expectStatus(policy, 201);
        policyId = policy.data.id;

        const stages = buildFeatureStages([prodEnvId]);
        const featureFixture = {
            ...createFeatureFixture(),
            stages,
            relationships: buildLinearRelationships(stages.length),
        };
        const feature = await adminClient.post(`/teams/${teamId}/features`, featureFixture);
        expectStatus(feature, 201);
        featureId = feature.data.id;

        const stage = await getStage(adminClient, featureId, prodEnvId);
        stageId = stage.id;

        requesterClient = (
            await createAndLoginUserWithRoles(adminClient, teamId, [REQUESTER_ROLE_ID], 'requester')
        ).client;
        approverClient = (
            await createAndLoginUserWithRoles(adminClient, teamId, [APPROVER_ROLE_ID], 'approver')
        ).client;
        viewerClient = (
            await createAndLoginUserWithRoles(adminClient, teamId, [], 'viewer')
        ).client;
    });

    afterAll(async () => {
        if (featureId) {
            await cleanupResource(adminClient, '/features', featureId);
        }
        if (policyId) {
            await cleanupResource(adminClient, '/approval-policies', policyId);
        }
        if (prodEnvId) {
            await cleanupResource(adminClient, '/environments', prodEnvId);
        }
        if (teamId) {
            await cleanupResource(adminClient, '/teams', teamId);
        }
    });

    describe('Authorization Guards', () => {
        it('should reject unauthenticated stage change requests', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.post(`/stages/${stageId}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });
            expectStatus(response, 401);
        });

        it('should reject malformed stage IDs', async () => {
            const response = await requesterClient.post('/stages/not-a-uuid/request-change', {
                request: 'DEPLOYMENT_REQUESTED',
            });
            expectStatus(response, 400);
        });

        it('should reject stage change requests from users without Requester role', async () => {
            const response = await viewerClient.post(`/stages/${stageId}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });
            expectStatus(response, 403);
        });

        it('should reject reject-actions from users without Approver role', async () => {
            const response = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'DEPLOYMENT_REJECTED',
            });
            expectStatus(response, 403);
        });
    });

    describe('Deployment Approval State Machine', () => {
        it('should create a pending deployment request from NOT_DEPLOYED', async () => {
            const response = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });
            expectStatus(response, 200);

            if (response.data.pendingApprovalRequestId) {
                deploymentRequestId = response.data.pendingApprovalRequestId;
            } else {
                deploymentRequestId = await findLatestPendingRequestId(adminClient, teamId, featureId);
            }
            expectUuid(deploymentRequestId);

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'DEPLOYMENT_REQUESTED',
            ]);
        });

        it('should approve deployment request and move stage to DEPLOYMENT_APPROVED', async () => {
            const response = await approverClient.post(`/approval-requests/${deploymentRequestId}/approve`, {
                comment: 'approved for deployment',
            });
            expectStatus(response, 200);
            expect(response.data.status).toBe('approved');

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'DEPLOYMENT_APPROVED',
            ]);
        });

        it('should finalize deployment to DEPLOYED', async () => {
            const response = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'DEPLOYED',
            });
            expectStatus(response, 200);

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'DEPLOYED',
            ]);
        });
    });

    describe('Rollback Approval State Machine', () => {
        it('should request rollback and create approval request', async () => {
            const response = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'ROLLBACK_REQUESTED',
            });
            expectStatus(response, 200);

            if (response.data.pendingApprovalRequestId) {
                rollbackRequestId = response.data.pendingApprovalRequestId;
            } else {
                rollbackRequestId = await findLatestPendingRequestId(adminClient, teamId, featureId);
            }
            expectUuid(rollbackRequestId);

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'ROLLBACK_REQUESTED',
            ]);
        });

        it('should reject rollback request and move stage to ROLLBACK_REJECTED', async () => {
            const response = await approverClient.post(`/approval-requests/${rollbackRequestId}/reject`, {
                comment: 'rollback rejected',
            });
            expectStatus(response, 200);
            expect(response.data.status).toBe('rejected');

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'ROLLBACK_REJECTED',
            ]);
        });

        it('should allow re-requesting rollback from ROLLBACK_REJECTED', async () => {
            const response = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'ROLLBACK_REQUESTED',
            });
            expectStatus(response, 200);

            if (response.data.pendingApprovalRequestId) {
                secondRollbackRequestId = response.data.pendingApprovalRequestId;
            } else {
                secondRollbackRequestId = await findLatestPendingRequestId(
                    adminClient,
                    teamId,
                    featureId
                );
            }
            expectUuid(secondRollbackRequestId);

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'ROLLBACK_REQUESTED',
            ]);
        });

        it('should approve rollback request and move stage to ROLLBACK_APPROVED', async () => {
            const response = await approverClient.post(
                `/approval-requests/${secondRollbackRequestId}/approve`,
                {
                    comment: 'rollback approved',
                }
            );
            expectStatus(response, 200);
            expect(response.data.status).toBe('approved');

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'ROLLBACK_APPROVED',
            ]);
        });

        it('should finalize rollback to ROLLBACKED', async () => {
            const response = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'ROLLBACKED',
            });
            expectStatus(response, 200);

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'ROLLBACKED',
            ]);
        });
    });

    describe('Invalid Transition And Cancel Flow', () => {
        it('should reject invalid transition from ROLLBACKED to DEPLOYED', async () => {
            const response = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'DEPLOYED',
            });
            expectStatus(response, 400);
        });

        it('should cancel pending request and reset stage status', async () => {
            const createResponse = await requesterClient.post(`/stages/${stageId}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });
            expectStatus(createResponse, 200);

            const pendingRequestId =
                createResponse.data.pendingApprovalRequestId ??
                (await findLatestPendingRequestId(adminClient, teamId, featureId));

            expectUuid(pendingRequestId);
            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'DEPLOYMENT_REQUESTED',
            ]);

            const cancelResponse = await requesterClient.post(
                `/approval-requests/${pendingRequestId}/cancel`
            );
            expectStatus(cancelResponse, 200);
            expect(cancelResponse.data.status).toBe('cancelled');

            await expectStageStatusEventually(adminClient, featureId, prodEnvId, [
                'ROLLBACKED',
            ]);
        });

        it('should keep original seed team approval endpoint functional', async () => {
            // Sanity check against default seeded team to ensure no accidental regression.
            const response = await adminClient.get(`/teams/${TEST_TEAM_ID}/approval-requests`, {
                statuses: 'pending',
                offset: 0,
                limit: 5,
            });
            expect([200, 404]).toContain(response.status);
        });
    });
});
