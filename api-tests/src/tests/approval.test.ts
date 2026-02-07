import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createApprovalPolicyFixture, createEnvironmentFixture } from '../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    expectPaginatedResponse,
    expectUuid,
    TEST_TEAM_ID,
    cleanupResource,
} from '../utils/test-utils.js';

/**
 * Approval API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams/{teamId}/approvals - List approval requests
 * - GET /api/v1/approvals/{id} - Get approval request by ID
 * - POST /api/v1/approvals/{id}/approve - Approve request
 * - POST /api/v1/approvals/{id}/reject - Reject request
 * - GET /api/v1/teams/{teamId}/approval-policies - List approval policies
 * - GET /api/v1/approval-policies/{id} - Get approval policy
 * - POST /api/v1/teams/{teamId}/approval-policies - Create approval policy
 * - PATCH /api/v1/approval-policies/{id} - Update approval policy
 * - DELETE /api/v1/approval-policies/{id} - Delete approval policy
 */
describe('Approval API', () => {
    let client: ApiClient;
    let testEnvironmentId: string;
    const createdPolicyIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();

        // Create test environment for policies
        const envFixture = createEnvironmentFixture();
        const envResponse = await client.post(`/teams/${TEST_TEAM_ID}/environments`, envFixture);
        testEnvironmentId = envResponse.data.id;
    });

    afterAll(async () => {
        // Cleanup policies
        for (const id of createdPolicyIds) {
            await cleanupResource(client, '/approval-policies', id);
        }
        // Cleanup environment
        if (testEnvironmentId) {
            await cleanupResource(client, '/environments', testEnvironmentId);
        }
    });

    describe('GET /teams/{teamId}/approvals', () => {
        it('should list approval requests for a team', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/approvals`);

            expectSuccess(response);
            expectPaginatedResponse(response);
        });

        it('should support pagination', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/approvals`, {
                page: 1,
                limit: 5,
            });

            expectSuccess(response);
            expect(response.data.items.length).toBeLessThanOrEqual(5);
        });

        it('should filter by status', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/approvals`, {
                status: 'pending',
            });

            expectSuccess(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${TEST_TEAM_ID}/approvals`);

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken(`/teams/${TEST_TEAM_ID}/approvals`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/approvals');

            expectClientError(response);
        });
    });

    describe('GET /teams/{teamId}/approval-policies', () => {
        it('should list approval policies for a team', async () => {
            const response = await client.get(`/teams/${TEST_TEAM_ID}/approval-policies`);

            expectSuccess(response);
            expect(Array.isArray(response.data)).toBe(true);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/teams/${TEST_TEAM_ID}/approval-policies`);

            expectStatus(response, 401);
        });

        it('should return 400 for invalid team ID', async () => {
            const response = await client.get('/teams/invalid-uuid/approval-policies');

            expectClientError(response);
        });
    });

    describe('POST /teams/{teamId}/approval-policies', () => {
        it('should create an approval policy', async () => {
            const fixture = createApprovalPolicyFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);

            createdPolicyIds.push(response.data.id);
        });

        it('should create policy with specific environments', async () => {
            const fixture = createApprovalPolicyFixture({
                appliesTo: 'specific_environments',
            });
            const requestBody = {
                ...fixture,
                environmentIds: [testEnvironmentId],
            };
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, requestBody);

            expectStatus(response, 201);
            createdPolicyIds.push(response.data.id);
        });

        it('should create policy requiring multiple approvers', async () => {
            const fixture = createApprovalPolicyFixture({
                requiredApprovers: 2,
                approverRoles: ['Admin', 'Reviewer'],
            });
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, fixture);

            expectStatus(response, 201);
            expect(response.data.requiredApprovers).toBe(2);

            createdPolicyIds.push(response.data.id);
        });

        it('should reject empty name', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, {
                name: '',
                requiredApprovers: 1,
                approverRoles: ['Admin'],
                appliesTo: 'all',
            });

            expectClientError(response);
        });

        it('should reject negative required approvers', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, {
                name: createApprovalPolicyFixture().name,
                requiredApprovers: -1,
                approverRoles: ['Admin'],
                appliesTo: 'all',
            });

            expectClientError(response);
        });

        it('should reject policy without approver roles', async () => {
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, {
                name: createApprovalPolicyFixture().name,
                requiredApprovers: 1,
                approverRoles: [],
                appliesTo: 'all',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createApprovalPolicyFixture();
            const response = await unauthClient.post(`/teams/${TEST_TEAM_ID}/approval-policies`, fixture);

            expectStatus(response, 401);
        });
    });

    describe('GET /approval-policies/{id}', () => {
        let testPolicyId: string;

        beforeAll(async () => {
            // Create a test policy
            const fixture = createApprovalPolicyFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, fixture);
            testPolicyId = response.data.id;
            createdPolicyIds.push(testPolicyId);
        });

        it('should get approval policy by ID', async () => {
            const response = await client.get(`/approval-policies/${testPolicyId}`);

            expectSuccess(response);
            expect(response.data.id).toBe(testPolicyId);
            expectUuid(response.data.id);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.get(`/approval-policies/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.get('/approval-policies/not-a-uuid');

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated(`/approval-policies/${testPolicyId}`);

            expectStatus(response, 401);
        });
    });

    describe('PATCH /approval-policies/{id}', () => {
        let testPolicyId: string;

        beforeAll(async () => {
            // Create a test policy
            const fixture = createApprovalPolicyFixture();
            const response = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, fixture);
            testPolicyId = response.data.id;
            createdPolicyIds.push(testPolicyId);
        });

        it('should update policy name', async () => {
            const newName = createApprovalPolicyFixture().name;
            const response = await client.patch(`/approval-policies/${testPolicyId}`, {
                name: newName,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
        });

        it('should update required approvers', async () => {
            const response = await client.patch(`/approval-policies/${testPolicyId}`, {
                requiredApprovers: 3,
            });

            expectSuccess(response);
            expect(response.data.requiredApprovers).toBe(3);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/approval-policies/${fakeId}`, {
                name: 'New Name',
            });

            expectStatus(response, 404);
        });

        it('should reject empty name update', async () => {
            const response = await client.patch(`/approval-policies/${testPolicyId}`, {
                name: '',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/approval-policies/${testPolicyId}`, {
                name: 'Unauthorized Update',
            });

            expectStatus(response, 401);
        });
    });

    describe('DELETE /approval-policies/{id}', () => {
        it('should delete an approval policy', async () => {
            // Create a disposable policy
            const fixture = createApprovalPolicyFixture();
            const createResponse = await client.post(`/teams/${TEST_TEAM_ID}/approval-policies`, fixture);
            expectStatus(createResponse, 201);
            const policyId = createResponse.data.id;

            // Delete it
            const deleteResponse = await client.delete(`/approval-policies/${policyId}`);
            expectStatus(deleteResponse, 204);

            // Verify it's gone
            const getResponse = await client.get(`/approval-policies/${policyId}`);
            expectStatus(getResponse, 404);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.delete(`/approval-policies/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.delete('/approval-policies/some-id');

            expectStatus(response, 401);
        });
    });

    // Approval Request Actions (approve/reject) - these require pending requests
    describe('Approval Request Actions', () => {
        // Note: These tests would require creating a feature change that triggers
        // an approval request. For now, we'll test the error cases.

        it('should return 404 when approving non-existent request', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.post(`/approvals/${fakeId}/approve`, {
                comment: 'Approved',
            });

            expectStatus(response, 404);
        });

        it('should return 404 when rejecting non-existent request', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.post(`/approvals/${fakeId}/reject`, {
                comment: 'Rejected',
            });

            expectStatus(response, 404);
        });

        it('should return 401 when approving without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.post('/approvals/some-id/approve', {
                comment: 'Approved',
            });

            expectStatus(response, 401);
        });
    });
});
