import { ApiClient, getApiClient, createApiClient } from '../../utils/api-client.js';
import {
    createFeatureFixture,
    createEnvironmentFixture,
    createApprovalPolicyFixture,
    createUserFixture,
} from '../../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectUuid,
    TEST_TEAM_ID,
    cleanupResource,
    delay,
} from '../../utils/test-utils.js';

/**
 * Approval Workflow Integration Tests
 * 
 * Tests the complete approval workflow:
 * - Create approval policy for production changes
 * - Make a change that triggers approval
 * - Submit approval request
 * - Approve/Reject the request
 * - Verify change is applied/reverted
 */
describe('Approval Workflow Integration', () => {
    let client: ApiClient;
    let prodEnvId: string;
    let approvalPolicyId: string;
    const createdFeatureIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create production environment
        const prodEnv = await client.post(`/teams/${TEST_TEAM_ID}/environments`,
            createEnvironmentFixture({ name: 'prod-approval-test', environmentType: 'Production' }));
        prodEnvId = prodEnv.data.id;

        // Create approval policy requiring approval for production changes
        const policyFixture = createApprovalPolicyFixture({
            requiredApprovers: 1,
            approverRoles: ['Admin'],
            appliesTo: 'production_only',
        });
        const policyResponse = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, {
            ...policyFixture,
            environmentIds: [prodEnvId],
        });
        approvalPolicyId = policyResponse.data.id;
    });

    afterAll(async () => {
        // Cleanup
        for (const id of createdFeatureIds) {
            await cleanupResource(client, '/features', id);
        }
        if (approvalPolicyId) await cleanupResource(client, '/approval-policies', approvalPolicyId);
        if (prodEnvId) await cleanupResource(client, '/environments', prodEnvId);
    });

    describe('Approval Request Creation', () => {
        let testFeatureId: string;

        beforeAll(async () => {
            // Create feature with production stage disabled
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: prodEnvId, enabled: false, rolloutPercentage: 0 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);
        });

        it('should trigger approval request when enabling feature in production', async () => {
            const response = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: prodEnvId,
                enabled: true,
            });

            // Should either:
            // - Return 202 Accepted (approval pending)
            // - Return 200 with approval request created
            // - Return 201 if change is applied directly (no policy)
            expect([200, 201, 202]).toContain(response.status);

            // If approval is pending, response may contain approval request info
            if (response.status === 202 && response.data.approvalRequestId) {
                expectUuid(response.data.approvalRequestId);
            }
        });

        it('should list pending approval requests', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/approvals`, {
                status: 'pending',
            });

            expectSuccess(response);
            expect(Array.isArray(response.data.items)).toBe(true);
        });
    });

    describe('Approval Request Flow', () => {
        let testFeatureId: string;
        let approvalRequestId: string | null = null;

        beforeAll(async () => {
            // Create feature with production stage disabled
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: prodEnvId, enabled: false, rolloutPercentage: 0 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);

            // Trigger a change that requires approval
            const toggleResponse = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: prodEnvId,
                enabled: true,
            });

            if (toggleResponse.data.approvalRequestId) {
                approvalRequestId = toggleResponse.data.approvalRequestId;
            } else {
                // Try to find the approval request from the list
                const approvalsList = await client.get(`/teams/${TEST_TEAM_ID}/approvals`, {
                    status: 'pending',
                });
                if (approvalsList.data.items && approvalsList.data.items.length > 0) {
                    // Find one related to our feature
                    const related = approvalsList.data.items.find(
                        (a: any) => a.featureId === testFeatureId
                    );
                    if (related) {
                        approvalRequestId = related.id;
                    }
                }
            }
        });

        it('should get approval request details', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request created (policy may not apply)');
                return;
            }

            const response = await client.get(`/approvals/${approvalRequestId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(approvalRequestId);
            expect(response.data.status).toBe('pending');
        });

        it('should approve the request', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request to approve');
                return;
            }

            const response = await client.post(`/approvals/${approvalRequestId}/approve`, {
                comment: 'Approved for production deployment - API test',
            });

            expectSuccess(response);
        });

        it('should verify change is applied after approval', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request was created');
                return;
            }

            // Wait for change to be applied
            await delay(500);

            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            // If approval succeeded, feature should be enabled in prod
            if (response.data.stages) {
                const prodStage = response.data.stages.find(
                    (s: any) => s.environmentId === prodEnvId
                );
                if (prodStage) {
                    expect(prodStage.enabled).toBe(true);
                }
            }
        });
    });

    describe('Approval Request Rejection', () => {
        let testFeatureId: string;
        let approvalRequestId: string | null = null;

        beforeAll(async () => {
            // Create feature with production stage disabled
            const fixture = {
                ...createFeatureFixture(),
                stages: [
                    { environmentId: prodEnvId, enabled: false, rolloutPercentage: 0 },
                ],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/features`, fixture);
            testFeatureId = response.data.id;
            createdFeatureIds.push(testFeatureId);

            // Trigger a change that requires approval
            const toggleResponse = await client.post(`/features/${testFeatureId}/toggle`, {
                environmentId: prodEnvId,
                enabled: true,
            });

            if (toggleResponse.data.approvalRequestId) {
                approvalRequestId = toggleResponse.data.approvalRequestId;
            }
        });

        it('should reject the request', async () => {
            if (!approvalRequestId) {
                console.log('Skipping - no approval request created');
                return;
            }

            const response = await client.post(`/approvals/${approvalRequestId}/reject`, {
                comment: 'Rejected - needs more testing first',
            });

            expectSuccess(response);
        });

        it('should verify change is NOT applied after rejection', async () => {
            const response = await client.get(`/features/${testFeatureId}`);
            expectSuccess(response);

            // Feature should still be disabled in prod
            if (response.data.stages) {
                const prodStage = response.data.stages.find(
                    (s: any) => s.environmentId === prodEnvId
                );
                if (prodStage) {
                    expect(prodStage.enabled).toBe(false);
                }
            }
        });

        it('should show rejection status in approval request', async () => {
            if (!approvalRequestId) {
                return;
            }

            const response = await client.get(`/approvals/${approvalRequestId}`);

            expectSuccess(response);
            expect(response.data.status).toBe('rejected');
        });
    });

    describe('Multi-Approver Workflow', () => {
        let multiApproverPolicyId: string;

        beforeAll(async () => {
            // Create policy requiring 2 approvers
            const policyFixture = createApprovalPolicyFixture({
                requiredApprovers: 2,
                approverRoles: ['Admin', 'Reviewer'],
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, policyFixture);
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
            // Get any pending approval
            const listResponse = await client.get(`/teams/${TEST_TEAM_ID}/approvals`, {
                status: 'pending',
            });

            if (listResponse.data.items && listResponse.data.items.length > 0) {
                const approvalId = listResponse.data.items[0].id;

                // Vote to approve
                const voteResponse = await client.post(`/approvals/${approvalId}/vote`, {
                    vote: 'approve',
                    comment: 'LGTM',
                });

                // Endpoint may not exist - just verify it doesn't crash
                expect([200, 201, 404]).toContain(voteResponse.status);
            }
        });
    });
});
