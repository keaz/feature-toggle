import axios, { AxiosResponse } from 'axios';
import { ApiClient, createApiClient, getApiClient } from '../../utils/api-client.js';
import {
    createApprovalPolicyFixture,
    createEnvironmentFixture,
    createFeatureFixture,
    createTeamFixture,
    createUserFixture,
    uniqueName,
} from '../../utils/test-fixtures.js';
import { cleanupResource, expectStatus, expectSuccess, expectUuid } from '../../utils/test-utils.js';

/**
 * End-to-end Jira flow (JI-30). Jira is simulated: the test posts Jira-shaped
 * bodies to the public inbound endpoint, authenticated by the integration secret,
 * exactly as a Jira Automation "Send web request" action or a Jira webhook would.
 */

const APPROVER_ROLE_ID = '00000000-0000-0000-0000-000000000001';
const BASE_URL = process.env.API_BASE_URL || 'http://127.0.0.1:18080/api/v1';

const ENVIRONMENT_FIELD = 'customfield_10042';
const JIRA_ACTOR = { accountId: '5b10ac8d82e05b22cc7d4ef5', displayName: 'Jane Doe' };

let changelogCounter = 10_500;

/** A Jira Cloud `jira:issue_updated` webhook body for a status transition. */
function webhookBody(issueKey: string, status: string, environmentValue: string) {
    changelogCounter += 1;
    return {
        timestamp: Date.now(),
        webhookEvent: 'jira:issue_updated',
        issue_event_type_name: 'issue_generic',
        user: { ...JIRA_ACTOR, active: true, accountType: 'atlassian' },
        issue: {
            id: '10042',
            key: issueKey,
            fields: {
                summary: 'Ship the new checkout',
                status: { name: status, id: '10003' },
                labels: [],
                [ENVIRONMENT_FIELD]: { value: environmentValue, id: '10100' },
                updated: new Date().toISOString(),
            },
        },
        changelog: {
            id: String(changelogCounter),
            items: [
                {
                    field: 'status',
                    fieldtype: 'jira',
                    fieldId: 'status',
                    fromString: 'In Progress',
                    toString: status,
                },
            ],
        },
    };
}

/** A Jira Automation "Issue data (Jira format)" body, wrapped as `{issue, user}`. */
function automationBody(issueKey: string, status: string, environmentValue: string) {
    return {
        issue: {
            id: '10042',
            key: issueKey,
            fields: {
                status: { name: status, id: '10003' },
                [ENVIRONMENT_FIELD]: { value: environmentValue, id: '10100' },
                updated: new Date().toISOString(),
            },
        },
        user: JIRA_ACTOR,
    };
}

async function sendJiraEvent(
    integrationId: string,
    secret: string,
    body: unknown,
    header: 'authorization' | 'fluxgate' = 'authorization'
): Promise<AxiosResponse> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (header === 'authorization') {
        headers.Authorization = `Bearer ${secret}`;
    } else {
        headers['X-FluxGate-Jira-Secret'] = secret;
    }
    return axios.post(`${BASE_URL}/integrations/jira/${integrationId}/events`, body, {
        headers,
        maxRedirects: 0,
        validateStatus: () => true,
    });
}

async function getStage(client: ApiClient, featureId: string, environmentId: string) {
    const feature = await client.get(`/features/${featureId}`);
    expectSuccess(feature);
    const stage = feature.data.stages?.find((item: any) => item.environment.id === environmentId);
    expect(stage).toBeDefined();
    return stage;
}

async function getStageStatus(client: ApiClient, featureId: string, environmentId: string) {
    return (await getStage(client, featureId, environmentId)).status as string;
}

function resultFor(response: AxiosResponse, environmentId: string, action: string) {
    const result = (response.data.results || []).find(
        (item: any) => item.environmentId === environmentId && item.action === action
    );
    expect(result).toBeDefined();
    return result;
}

async function getApprovalRequest(client: ApiClient, teamId: string, requestId: string) {
    const response = await client.get(`/teams/${teamId}/approval-requests`, {
        statuses: 'pending,approved,auto_approved,rejected,cancelled',
        offset: 0,
        limit: 100,
    });
    expectSuccess(response);
    const request = (response.data.items || []).find((item: any) => item.id === requestId);
    expect(request).toBeDefined();
    return request;
}

async function featureActivityCount(client: ApiClient, featureId: string): Promise<number> {
    const response = await client.get('/activity/recent', {
        entityType: 'feature',
        entityId: featureId,
        offset: 0,
        limit: 100,
    });
    expectSuccess(response);
    return response.data.meta.total;
}

describe('Jira status rules flow', () => {
    let adminClient: ApiClient;
    let approverClient: ApiClient;
    let teamId: string;
    let qaEnvId: string;
    let prodEnvId: string;
    let policyId: string;
    let featureId: string;
    let integrationId: string;
    let secret = '';
    let prodPendingRequestId: string;
    const issueKey = `PROJ-${Math.floor(Math.random() * 90_000) + 10_000}`;

    beforeAll(async () => {
        adminClient = await getApiClient();

        // 1. Team, environments qa and prod, a feature with a stage in both,
        //    an approval policy for both, and a human approver.
        const team = await adminClient.post('/teams', createTeamFixture());
        expectStatus(team, 201);
        teamId = team.data.id;

        const qa = await adminClient.post(
            `/teams/${teamId}/environments`,
            createEnvironmentFixture({ name: uniqueName('qa'), environmentType: 'Staging' })
        );
        expectStatus(qa, 201);
        qaEnvId = qa.data.id;

        const prod = await adminClient.post(
            `/teams/${teamId}/environments`,
            createEnvironmentFixture({ name: uniqueName('prod'), environmentType: 'Production' })
        );
        expectStatus(prod, 201);
        prodEnvId = prod.data.id;

        const policy = await adminClient.post(
            `/teams/${teamId}/approval-policies`,
            createApprovalPolicyFixture({
                appliesTo: 'specific_environments',
                environmentIds: [qaEnvId, prodEnvId],
                requiredApprovers: 1,
                approverRoleIds: [APPROVER_ROLE_ID],
            })
        );
        expectStatus(policy, 201);
        policyId = policy.data.id;

        // Eligible approvers are resolved when a request is created, so the human
        // approver must exist before the first Jira event.
        const approverFixture = createUserFixture({
            username: `jira-approver-${Date.now()}-${Math.random().toString(16).slice(2, 6)}`,
        });
        const approver = await adminClient.post('/users', {
            ...approverFixture,
            isTemporaryPassword: false,
        });
        expectStatus(approver, 201);
        expectSuccess(await adminClient.post(`/users/${approver.data.id}/teams`, { teamIds: [teamId] }));
        expectSuccess(
            await adminClient.post(`/users/${approver.data.id}/roles`, { roleIds: [APPROVER_ROLE_ID] })
        );
        approverClient = createApiClient({
            username: approverFixture.username,
            password: approverFixture.password,
        });
        await approverClient.authenticate();

        // Pipeline qa -> prod. The flow deploys qa before it touches prod.
        const feature = await adminClient.post(`/teams/${teamId}/features`, {
            ...createFeatureFixture(),
            stages: [qaEnvId, prodEnvId].map((environmentId, index) => ({
                environmentId,
                orderIndex: index,
                position: String(index + 1),
                bucketingKey: 'userId',
            })),
            relationships: [{ sourceId: 0, targetId: 1 }],
        });
        expectStatus(feature, 201);
        featureId = feature.data.id;

        // 2. The Jira integration: environment from customfield_10042, alias
        //    "QA" -> qa, Jira approves in qa only.
        const integration = await adminClient.post(`/teams/${teamId}/jira-integrations`, {
            name: uniqueName('jira'),
            jiraBaseUrl: 'https://acme.atlassian.net',
            environmentField: ENVIRONMENT_FIELD,
            environmentAliases: { QA: qaEnvId, Prod: prodEnvId },
            jiraApprovedEnvironmentIds: [qaEnvId],
        });
        expectStatus(integration, 201);
        integrationId = integration.data.integration.id;
        secret = integration.data.secret;
        expect(secret).toHaveLength(43);

        const rules = await adminClient.put(`/jira-integrations/${integrationId}/rules`, {
            rules: [
                { jiraStatus: 'Ready for Release', action: 'approve' },
                { jiraStatus: 'Done', action: 'deploy' },
                { jiraStatus: 'Ready for Release', action: 'request', environmentIds: [prodEnvId] },
            ],
        });
        expectStatus(rules, 200);
        expect(rules.data.items).toHaveLength(3);

        // 3. Link the issue to the feature.
        const link = await adminClient.post(`/features/${featureId}/external-links`, {
            system: 'jira',
            externalKey: issueKey,
            url: `https://acme.atlassian.net/browse/${issueKey}`,
        });
        expectStatus(link, 201);
    });

    afterAll(async () => {
        if (integrationId) {
            await cleanupResource(adminClient, '/jira-integrations', integrationId);
        }
        if (featureId) {
            await cleanupResource(adminClient, '/features', featureId);
        }
        if (policyId) {
            await cleanupResource(adminClient, '/approval-policies', policyId);
        }
        for (const environmentId of [qaEnvId, prodEnvId]) {
            if (environmentId) {
                await cleanupResource(adminClient, '/environments', environmentId);
            }
        }
        if (teamId) {
            await cleanupResource(adminClient, '/teams', teamId);
        }
    });

    it('"Ready for Release" in QA approves the qa stage as Jira', async () => {
        const response = await sendJiraEvent(
            integrationId,
            secret,
            webhookBody(issueKey, 'Ready for Release', 'QA')
        );
        expectStatus(response, 200);
        expectUuid(response.data.eventId);
        expect(response.data.duplicate).toBe(false);
        expect(response.data.unknownEnvironments).toEqual([]);

        const approve = resultFor(response, qaEnvId, 'approve');
        expect(approve.outcome).toBe('applied');
        expect(approve.to).toBe('DEPLOYMENT_APPROVED');
        expect(approve.featureKey).toBeTruthy();
        // The prod-only request rule does not target qa.
        expect(response.data.results).toHaveLength(1);

        expect(await getStageStatus(adminClient, featureId, qaEnvId)).toBe('DEPLOYMENT_APPROVED');

        const request = await getApprovalRequest(adminClient, teamId, approve.approvalRequestId);
        expect(request.status).toBe('approved');
        expect(request.approvalSource).toBe('jira');
        expect(request.externalRef).toBe(issueKey);
        expect(request.externalApprover).toMatchObject({
            account_id: JIRA_ACTOR.accountId,
            display_name: JIRA_ACTOR.displayName,
            issue_key: issueKey,
            status: 'Ready for Release',
        });
    });

    it('"Done" in QA deploys the qa stage', async () => {
        // Automation body and the X-FluxGate-Jira-Secret header, to cover both shapes.
        const response = await sendJiraEvent(
            integrationId,
            secret,
            automationBody(issueKey, 'Done', 'QA'),
            'fluxgate'
        );
        expectStatus(response, 200);

        const deploy = resultFor(response, qaEnvId, 'deploy');
        expect(deploy.outcome).toBe('applied');
        expect(deploy.from).toBe('DEPLOYMENT_APPROVED');
        expect(deploy.to).toBe('DEPLOYED');

        expect(await getStageStatus(adminClient, featureId, qaEnvId)).toBe('DEPLOYED');
    });

    it('"Done" in Prod before any approval is refused and changes nothing', async () => {
        const before = await getStageStatus(adminClient, featureId, prodEnvId);
        expect(before).toBe('NOT_DEPLOYED');

        const response = await sendJiraEvent(integrationId, secret, webhookBody(issueKey, 'Done', 'Prod'));
        expectStatus(response, 200);

        const deploy = resultFor(response, prodEnvId, 'deploy');
        expect(deploy.outcome).toBe('refused');
        expect(deploy.reason).toBe('not approved');

        expect(await getStageStatus(adminClient, featureId, prodEnvId)).toBe('NOT_DEPLOYED');
    });

    it('"Ready for Release" in Prod is not trusted: approve is refused, request leaves a pending request', async () => {
        const response = await sendJiraEvent(
            integrationId,
            secret,
            webhookBody(issueKey, 'Ready for Release', 'Prod')
        );
        expectStatus(response, 200);

        const approve = resultFor(response, prodEnvId, 'approve');
        expect(approve.outcome).toBe('refused');
        expect(approve.reason).toBe('environment not approved by Jira');

        const request = resultFor(response, prodEnvId, 'request');
        expect(request.outcome).toBe('applied');
        expect(request.to).toBe('DEPLOYMENT_REQUESTED');
        expectUuid(request.approvalRequestId);
        prodPendingRequestId = request.approvalRequestId;

        expect(await getStageStatus(adminClient, featureId, prodEnvId)).toBe('DEPLOYMENT_REQUESTED');

        const pending = await getApprovalRequest(adminClient, teamId, prodPendingRequestId);
        expect(pending.status).toBe('pending');
        expect(pending.approvalSource).toBe('fluxgate');
        expect(pending.externalRef).toBe(issueKey);
    });

    it('a human approves the prod request in FluxGate, then "Done" in Prod deploys', async () => {
        const vote = await approverClient.post(`/approval-requests/${prodPendingRequestId}/approve`, {
            comment: 'approved in FluxGate',
        });
        expectStatus(vote, 200);
        expect(vote.data.status).toBe('approved');
        expect(await getStageStatus(adminClient, featureId, prodEnvId)).toBe('DEPLOYMENT_APPROVED');

        const response = await sendJiraEvent(integrationId, secret, webhookBody(issueKey, 'Done', 'Prod'));
        expectStatus(response, 200);

        const deploy = resultFor(response, prodEnvId, 'deploy');
        expect(deploy.outcome).toBe('applied');
        expect(deploy.to).toBe('DEPLOYED');

        expect(await getStageStatus(adminClient, featureId, prodEnvId)).toBe('DEPLOYED');
    });

    it('rejects a wrong or missing secret with 401 and stores nothing', async () => {
        const wrong = await sendJiraEvent(
            integrationId,
            'not-the-secret',
            webhookBody(issueKey, 'Done', 'QA')
        );
        expectStatus(wrong, 401);

        const missing = await axios.post(
            `${BASE_URL}/integrations/jira/${integrationId}/events`,
            webhookBody(issueKey, 'Done', 'QA'),
            { headers: { 'Content-Type': 'application/json' }, validateStatus: () => true }
        );
        expectStatus(missing, 401);
    });

    it('replays a repeated delivery without running the rules again', async () => {
        const body = webhookBody(issueKey, 'Ready for Release', 'QA');

        const first = await sendJiraEvent(integrationId, secret, body);
        expectStatus(first, 200);
        expect(first.data.duplicate).toBe(false);
        // The qa stage is already deployed, so the approve rule has nothing to do.
        expect(resultFor(first, qaEnvId, 'approve').outcome).toBe('no_op');

        const activityBefore = await featureActivityCount(adminClient, featureId);

        const second = await sendJiraEvent(integrationId, secret, body);
        expectStatus(second, 200);
        expect(second.data.duplicate).toBe(true);
        expect(second.data.eventId).toBe(first.data.eventId);
        expect(second.data.results).toEqual(first.data.results);

        expect(await featureActivityCount(adminClient, featureId)).toBe(activityBefore);
    });

    it('lists every stored event with its results in the event log', async () => {
        const response = await adminClient.get(`/jira-integrations/${integrationId}/events`, {
            offset: 0,
            limit: 50,
        });
        expectStatus(response, 200);

        // Five processed events plus the first delivery of the repeated one; the
        // replay and the 401s are not stored.
        expect(response.data.meta.total).toBe(6);
        const items = response.data.items;
        expect(items).toHaveLength(6);
        for (const item of items) {
            expect(item.integrationId).toBe(integrationId);
            expect(item.issueKey).toBe(issueKey);
            expect(item.error).toBeNull();
            expect(Array.isArray(item.results)).toBe(true);
            expect(item.results.length).toBeGreaterThan(0);
        }

        // Newest first.
        const statuses = items.map((item: any) => item.jiraStatus).reverse();
        expect(statuses).toEqual([
            'Ready for Release',
            'Done',
            'Done',
            'Ready for Release',
            'Done',
            'Ready for Release',
        ]);
        expect(items[items.length - 1].jiraActor).toMatchObject({
            accountId: JIRA_ACTOR.accountId,
            displayName: JIRA_ACTOR.displayName,
        });

        const outcomes = items
            .flatMap((item: any) => item.results)
            .map((result: any) => `${result.action}:${result.outcome}`);
        expect(outcomes).toEqual(
            expect.arrayContaining(['approve:applied', 'deploy:applied', 'deploy:refused', 'approve:refused', 'request:applied'])
        );
    });
});
