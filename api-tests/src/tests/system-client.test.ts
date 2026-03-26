import axios, { AxiosInstance } from 'axios';
import { ApiClient, getApiClient } from '../utils/api-client.js';
import {
  createApprovalPolicyFixture,
  createEnvironmentFixture,
  createFeatureFixture,
  createTeamFixture,
} from '../utils/test-fixtures.js';
import { cleanupResource, expectStatus, expectSuccess } from '../utils/test-utils.js';

const APPROVER_ROLE_ID = '00000000-0000-0000-0000-000000000001';

const BASE_URL = process.env.API_BASE_URL || 'http://127.0.0.1:18080/api/v1';

function createTokenClient(token: string): AxiosInstance {
  return axios.create({
    baseURL: BASE_URL,
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
    validateStatus: () => true,
  });
}

describe('System Client API', () => {
  let adminClient: ApiClient;
  let teamId: string;
  let otherTeamId: string;
  let environmentId: string;
  let featureId: string;
  let stageId: string;
  let policyId: string;
  let systemClientId: string;
  let systemToken = '';
  let tokenClient: AxiosInstance;

  beforeAll(async () => {
    adminClient = await getApiClient();

    const teamResponse = await adminClient.post('/teams', createTeamFixture());
    expectStatus(teamResponse, 201);
    teamId = teamResponse.data.id;

    const otherTeamResponse = await adminClient.post('/teams', createTeamFixture());
    expectStatus(otherTeamResponse, 201);
    otherTeamId = otherTeamResponse.data.id;

    const envResponse = await adminClient.post(
      `/teams/${teamId}/environments`,
      createEnvironmentFixture({ environmentType: 'Production' })
    );
    expectStatus(envResponse, 201);
    environmentId = envResponse.data.id;

    const policyResponse = await adminClient.post(
      `/teams/${teamId}/approval-policies`,
      createApprovalPolicyFixture({
        appliesTo: 'production_only',
        requiredApprovers: 1,
        approverRoleIds: [APPROVER_ROLE_ID],
      })
    );
    expectStatus(policyResponse, 201);
    policyId = policyResponse.data.id;

    const featureResponse = await adminClient.post(
      `/teams/${teamId}/features`,
      {
        ...createFeatureFixture({ environmentId }),
        relationships: [],
      }
    );
    expectStatus(featureResponse, 201);
    featureId = featureResponse.data.id;
    stageId = featureResponse.data.stages[0].id;

    const expiresAt = new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString();
    const systemClientResponse = await adminClient.post(
      `/teams/${teamId}/system-clients`,
      {
        name: `automation-${Date.now()}`,
        description: 'Automation token for integration tests',
        enabled: true,
        expiresAt,
      }
    );
    expectStatus(systemClientResponse, 201);
    systemClientId = systemClientResponse.data.systemClient.id;
    systemToken = systemClientResponse.data.token;
    tokenClient = createTokenClient(systemToken);
  });

  afterAll(async () => {
    if (featureId) {
      await cleanupResource(adminClient, '/features', featureId);
    }
    if (policyId) {
      await cleanupResource(adminClient, '/approval-policies', policyId);
    }
    if (environmentId) {
      await cleanupResource(adminClient, '/environments', environmentId);
    }
    if (teamId) {
      await cleanupResource(adminClient, '/teams', teamId);
    }
    if (otherTeamId) {
      await cleanupResource(adminClient, '/teams', otherTeamId);
    }
  });

  it('creates a system client and returns a JWT token', async () => {
    expect(systemClientId).toBeTruthy();
    expect(systemToken).toBeTruthy();
    expect(systemToken.split('.')).toHaveLength(3);
  });

  it('allows team-scoped API access', async () => {
    const response = await tokenClient.get(`/teams/${teamId}/clients`);
    expectStatus(response, 200);
    expect(response.data).toHaveProperty('items');
  });

  it('blocks cross-team access for system client token', async () => {
    const response = await tokenClient.get(`/teams/${otherTeamId}/clients`);
    expectStatus(response, 403);
  });

  it('can request and approve a stage change using system client token', async () => {
    const requestResponse = await tokenClient.post(`/stages/${stageId}/request-change`, {
      request: 'DEPLOYMENT_REQUESTED',
    });
    expectStatus(requestResponse, 200);

    const requestId = requestResponse.data.pendingApprovalRequestId;
    expect(requestId).toBeTruthy();

    const approveResponse = await tokenClient.post(`/approval-requests/${requestId}/approve`, {
      comment: 'automation approval',
    });
    expectStatus(approveResponse, 200);

    const verification = await adminClient.get(`/teams/${teamId}/approval-requests`, {
      statuses: 'approved,auto_approved,pending',
      offset: 0,
      limit: 100,
    });
    expectSuccess(verification);
    expect((verification.data.items || []).some((item: any) => item.id === requestId)).toBe(true);
  });

  it('can regenerate token and invalidates previous token', async () => {
    const regenerateResponse = await adminClient.post(
      `/system-clients/${systemClientId}/regenerate-token`,
      {}
    );
    expectStatus(regenerateResponse, 200);

    const newToken = regenerateResponse.data.token;
    expect(newToken).toBeTruthy();
    expect(newToken).not.toBe(systemToken);

    const oldTokenClient = createTokenClient(systemToken);
    const oldTokenResponse = await oldTokenClient.get(`/teams/${teamId}/clients`);
    expectStatus(oldTokenResponse, 401);

    tokenClient = createTokenClient(newToken);
    const newTokenResponse = await tokenClient.get(`/teams/${teamId}/clients`);
    expectStatus(newTokenResponse, 200);
  });
});
