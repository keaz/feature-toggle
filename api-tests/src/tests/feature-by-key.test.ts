import axios, { AxiosInstance } from 'axios';
import { ApiClient, getApiClient } from '../utils/api-client.js';
import {
  createEnvironmentFixture,
  createFeatureFixture,
  createTeamFixture,
} from '../utils/test-fixtures.js';
import { cleanupResource, expectStatus } from '../utils/test-utils.js';

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

async function createSystemClientToken(adminClient: ApiClient, teamId: string): Promise<string> {
  const response = await adminClient.post(`/teams/${teamId}/system-clients`, {
    name: `by-key-${Date.now()}-${Math.random().toString(16).slice(2, 6)}`,
    description: 'By-key endpoint tests',
    enabled: true,
    expiresAt: new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString(),
  });
  expectStatus(response, 201);
  return response.data.token;
}

describe('Feature by-key API', () => {
  let adminClient: ApiClient;
  let teamId: string;
  let otherTeamId: string;
  let environmentId: string;
  let environmentName: string;
  let featureId: string;
  let featureKey: string;
  let stageId: string;
  let tokenClient: AxiosInstance;
  let otherTeamClient: AxiosInstance;

  beforeAll(async () => {
    adminClient = await getApiClient();

    const teamResponse = await adminClient.post('/teams', createTeamFixture());
    expectStatus(teamResponse, 201);
    teamId = teamResponse.data.id;

    const otherTeamResponse = await adminClient.post('/teams', createTeamFixture());
    expectStatus(otherTeamResponse, 201);
    otherTeamId = otherTeamResponse.data.id;

    // A space in the name checks that the path segment is URL-decoded.
    environmentName = `Pre Prod ${Date.now()}`;
    const envResponse = await adminClient.post(
      `/teams/${teamId}/environments`,
      createEnvironmentFixture({ name: environmentName })
    );
    expectStatus(envResponse, 201);
    environmentId = envResponse.data.id;

    const featureResponse = await adminClient.post(`/teams/${teamId}/features`, {
      ...createFeatureFixture({ environmentId }),
      relationships: [],
    });
    expectStatus(featureResponse, 201);
    featureId = featureResponse.data.id;
    featureKey = featureResponse.data.key;
    stageId = featureResponse.data.stages[0].id;

    tokenClient = createTokenClient(await createSystemClientToken(adminClient, teamId));
    otherTeamClient = createTokenClient(await createSystemClientToken(adminClient, otherTeamId));
  });

  afterAll(async () => {
    if (featureId) {
      await cleanupResource(adminClient, '/features', featureId);
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

  const byKey = () => `/teams/${teamId}/features/by-key/${encodeURIComponent(featureKey)}`;
  const requestChange = (envName: string) =>
    `${byKey()}/environments/${encodeURIComponent(envName)}/request-change`;

  it('requests a stage change by key and reads the result by key', async () => {
    const requestResponse = await tokenClient.post(requestChange(environmentName.toLowerCase()), {
      request: 'DEPLOYMENT_REQUESTED',
      externalRef: 'PROJ-12',
    });
    expectStatus(requestResponse, 200);
    expect(requestResponse.data.id).toBe(featureId);

    const readResponse = await tokenClient.get(byKey());
    expectStatus(readResponse, 200);
    expect(readResponse.data.id).toBe(featureId);
    const stage = readResponse.data.stages.find((item: any) => item.id === stageId);
    expect(stage).toBeTruthy();
    expect(stage.status).not.toBe('NOT_DEPLOYED');
  });

  it('names what is missing in a 404', async () => {
    const unknownEnv = await tokenClient.post(requestChange('no-such-environment'), {
      request: 'DEPLOYMENT_REQUESTED',
    });
    expectStatus(unknownEnv, 404);
    expect(unknownEnv.data.code).toBe('environment_not_found');

    const unknownKey = await tokenClient.get(`/teams/${teamId}/features/by-key/no-such-feature`);
    expectStatus(unknownKey, 404);
    expect(unknownKey.data.code).toBe('feature_not_found');
  });

  it("denies another team's system client", async () => {
    const read = await otherTeamClient.get(byKey());
    expectStatus(read, 403);

    const request = await otherTeamClient.post(requestChange(environmentName), {
      request: 'DEPLOYMENT_REQUESTED',
    });
    expectStatus(request, 403);
  });
});
