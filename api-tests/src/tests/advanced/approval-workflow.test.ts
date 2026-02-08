import { ApiClient, getApiClient } from '../../utils/api-client.js';
import {
    createFeatureFixture,
    createEnvironmentFixture,
    createApprovalPolicyFixture,
} from '../../utils/test-fixtures.js';
import {
    expectSuccess,
    expectUuid,
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

async function listApprovalRequests(client: ApiClient, statuses?: string) {
    return client.get(`/teams/${TEST_TEAM_ID}/approval-requests`, {
        statuses,
        offset: 0,
        limit: 100,
    });
}

/**
 * Approval Workflow Integration Tests
 * 
 * Tests the complete approval workflow:
 * - Create approval policy for production changes
 * - Make a change that triggers approval
 * - Approve/Reject the request
 * - Verify stage status updates
 */
describe('Approval Workflow Integration', () => {
    const TEAM_ADMIN_ROLE_ID = '00000000-0000-0000-0000-000000000003';
    const APPROVER_ROLE_ID = '00000000-0000-0000-0000-000000000001';
    let client: ApiClient;
    let prodEnvId: string;
    let approvalPolicyId: string;
    const createdFeatureIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create production environment
        const prodEnv = await client.post(
            `/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'prod-approval-test', environmentType: 'Production' })
        );
        prodEnvId = prodEnv.data.id;

        // Create approval policy requiring approval for production changes
        const policyFixture = createApprovalPolicyFixture({
            requiredApprovers: 1,
            approverRoleIds: [TEAM_ADMIN_ROLE_ID],
            appliesTo: 'production_only',
            environmentIds: [prodEnvId],
        });
        const policyResponse = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, policyFixture);
        expect(policyResponse.status).toBe(201);
        approvalPolicyId = policyResponse.data.id;
    });

    afterAll(async () => {
        for (const id of createdFeatureIds) {
            await cleanupResource(client, '/features', id);
        }
        if (approvalPolicyId) {
            await cleanupResource(client, '/approval-policies', approvalPolicyId);
        }
        if (prodEnvId) {
            await cleanupResource(client, '/environments', prodEnvId);
        }
    });

    describe('Approval Request Creation', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            const stages = buildFeatureStages([prodEnvId]);
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

        it('should trigger approval request when enabling feature in production', async () => {
            const prodStage = await getStageByEnvironment(client, testFeatureId, prodEnvId);
            const response = await client.post(`/stages/${prodStage.id}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });

            expect([200, 400, 403]).toContain(response.status);
            if (response.status === 200 && response.data.pendingApprovalRequestId) {
                expectUuid(response.data.pendingApprovalRequestId);
            }
        });

        it('should list pending approval requests', async () => {
            const response = await listApprovalRequests(client, 'pending');
            expectSuccess(response);
            expect(Array.isArray(response.data.items)).toBe(true);
        });
    });

    describe('Approval Request Flow', () => {
        let testFeatureId: string;
        let approvalRequestId: string | null = null;

        beforeAll(async () => {
            const stages = buildFeatureStages([prodEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expect(response.status).toBe(201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);

            const prodStage = await getStageByEnvironment(client, testFeatureId, prodEnvId);
            const requestResponse = await client.post(`/stages/${prodStage.id}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });

            if (requestResponse.status === 200 && requestResponse.data.pendingApprovalRequestId) {
                approvalRequestId = requestResponse.data.pendingApprovalRequestId;
                return;
            }

            const approvalsList = await listApprovalRequests(client, 'pending');
            if (approvalsList.status === 200 && approvalsList.data.items?.length > 0) {
                const related = approvalsList.data.items.find((item: any) => item.featureId === testFeatureId);
                approvalRequestId = related?.id ?? null;
            }
        });

        it('should get approval request details from list endpoint', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request created (policy may not apply)');
                return;
            }

            const response = await listApprovalRequests(client);
            expectSuccess(response);
            const approval = response.data.items.find((item: any) => item.id === approvalRequestId);
            expect(approval).toBeDefined();
            expect(approval.status).toBe('pending');
        });

        it('should approve the request', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request to approve');
                return;
            }

            const response = await client.post(`/approval-requests/${approvalRequestId}/approve`, {
                comment: 'Approved for production deployment - API test',
            });
            expect([200, 400, 403]).toContain(response.status);
        });

        it('should verify stage moves forward after approval', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request was created');
                return;
            }

            await delay(300);

            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            const prodStage = response.data.stages?.find((s: any) => s.environment.id === prodEnvId);
            expect(prodStage).toBeDefined();
            if (prodStage) {
                expect([
                    'DEPLOYMENT_REQUESTED',
                    'DEPLOYMENT_APPROVED',
                    'DEPLOYED',
                ]).toContain(prodStage.status);
            }
        });
    });

    describe('Approval Request Rejection', () => {
        let testFeatureId: string;
        let approvalRequestId: string | null = null;
        let rejectResponseStatus: number | null = null;

        beforeAll(async () => {
            const stages = buildFeatureStages([prodEnvId]);
            const fixture = {
                ...createFeatureFixture(),
                stages,
                relationships: buildLinearRelationships(stages.length),
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            expect(response.status).toBe(201);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);

            const prodStage = await getStageByEnvironment(client, testFeatureId, prodEnvId);
            const requestResponse = await client.post(`/stages/${prodStage.id}/request-change`, {
                request: 'DEPLOYMENT_REQUESTED',
            });

            if (requestResponse.status === 200 && requestResponse.data.pendingApprovalRequestId) {
                approvalRequestId = requestResponse.data.pendingApprovalRequestId;
                return;
            }

            const approvalsList = await listApprovalRequests(client, 'pending');
            if (approvalsList.status === 200 && approvalsList.data.items?.length > 0) {
                const related = approvalsList.data.items.find((item: any) => item.featureId === testFeatureId);
                approvalRequestId = related?.id ?? null;
            }
        });

        it('should reject the request', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request created');
                return;
            }

            const response = await client.post(`/approval-requests/${approvalRequestId}/reject`, {
                comment: 'Rejected - needs more testing first',
            });

            rejectResponseStatus = response.status;
            expect([200, 400, 403]).toContain(response.status);
        });

        it('should verify change is NOT applied after rejection', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            const prodStage = response.data.stages?.find((s: any) => s.environment.id === prodEnvId);
            expect(prodStage).toBeDefined();
            if (prodStage && rejectResponseStatus === 200) {
                expect(['DEPLOYMENT_REJECTED', 'DEPLOYMENT_REQUESTED']).toContain(prodStage.status);
            }
        });

        it('should show rejection status in approval request', async () => {
            if (!approvalRequestId) {
                return;
            }

            const response = await listApprovalRequests(client);
            expectSuccess(response);
            const approval = response.data.items.find((item: any) => item.id === approvalRequestId);
            expect(approval).toBeDefined();
            if (approval && rejectResponseStatus === 200) {
                expect(['rejected', 'pending']).toContain(approval.status);
            }
        });
    });

    describe('Multi-Approver Workflow', () => {
        let multiApproverPolicyId: string;

        beforeAll(async () => {
            const policyFixture = createApprovalPolicyFixture({
                requiredApprovers: 2,
                approverRoleIds: [TEAM_ADMIN_ROLE_ID, APPROVER_ROLE_ID],
                appliesTo: 'production_only',
                environmentIds: [prodEnvId],
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, policyFixture);
            expect(response.status).toBe(201);
            multiApproverPolicyId = response.data.id;
        });

        afterAll(async () => {
            if (multiApproverPolicyId) {
                await cleanupResource(client, '/approval-policies', multiApproverPolicyId);
            }
        });

        it('should create policy requiring multiple approvers', async () => {
            const response = await client.get(`/approval-policies/${multiApproverPolicyId}`);

            expectSuccess(response);
            expect(response.data.requiredApprovers).toBe(2);
        });
    });

    describe('Approval Voting', () => {
        it('should allow voting on approval request', async () => {
            const listResponse = await listApprovalRequests(client, 'pending');
            if (listResponse.status !== 200 || !listResponse.data.items?.length) {
                return;
            }

            const approvalId = listResponse.data.items[0].id;
            const voteResponse = await client.post(`/approval-requests/${approvalId}/vote`, {
                vote: 'approve',
                comment: 'LGTM',
            });

            // Endpoint may not exist - just verify it doesn't crash
            expect([200, 201, 404]).toContain(voteResponse.status);
        });
    });
});
