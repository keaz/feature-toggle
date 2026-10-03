import axios, { AxiosResponse } from 'axios';
import crypto from 'node:crypto';
import http from 'node:http';
import { AddressInfo } from 'node:net';
import { ApiClient, createApiClient, getApiClient } from '../../utils/api-client.js';
import {
    createApprovalPolicyFixture,
    createEnvironmentFixture,
    createFeatureFixture,
    createTeamFixture,
    createUserFixture,
    uniqueName,
} from '../../utils/test-fixtures.js';
import { cleanupResource, expectStatus, expectSuccess } from '../../utils/test-utils.js';

/**
 * End-to-end Jira write-back (JI-50). Jira is a fake HTTP server started by the
 * test. The backend under test must run with `[jira] allow_insecure_http = true`
 * and a `ui_base_url` (see api-tests/backend-config.toml), and must reach the fake
 * Jira at FAKE_JIRA_HOST (default 127.0.0.1; the docker runner uses
 * host.docker.internal).
 *
 * The capture and the sender run every 5 s, so the tests poll for up to 30 s.
 * Test 7 fills the rate-limit bucket of the integration and must stay last.
 */

const APPROVER_ROLE_ID = '00000000-0000-0000-0000-000000000001';
const BASE_URL = process.env.API_BASE_URL || 'http://127.0.0.1:18080/api/v1';
const FAKE_JIRA_HOST = process.env.FAKE_JIRA_HOST ?? '127.0.0.1';

const ENVIRONMENT_FIELD = 'customfield_10042';
const JIRA_ACTOR = { accountId: '5b10ac8d82e05b22cc7d4ef5', displayName: 'Jane Doe' };
const ACCOUNT_EMAIL = 'fluxgate-bot@example.com';

interface RecordedRequest {
    method: string;
    url: string;
    authorization: string | undefined;
    body: any;
}

let changelogCounter = 20_500;

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

async function postEvent(
    integrationId: string,
    body: unknown,
    headers: Record<string, string>,
    raw?: string
): Promise<AxiosResponse> {
    return axios.post(
        `${BASE_URL}/integrations/jira/${integrationId}/events`,
        raw ?? body,
        {
            headers: { 'Content-Type': 'application/json', ...headers },
            maxRedirects: 0,
            validateStatus: () => true,
        }
    );
}

async function waitFor<T>(
    description: string,
    probe: () => T | undefined | false | Promise<T | undefined | false>,
    timeoutMs = 30_000
): Promise<T> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const value = await probe();
        if (value) {
            return value;
        }
        if (Date.now() > deadline) {
            throw new Error(`timed out after ${timeoutMs} ms waiting for: ${description}`);
        }
        await new Promise((resolve) => setTimeout(resolve, 500));
    }
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The text of a Jira comment body: ADF on Cloud, a plain string on Data Center. */
function commentText(body: any): string {
    const value = body?.body;
    if (typeof value === 'string') {
        return value;
    }
    const lines: string[] = [];
    for (const paragraph of value?.content ?? []) {
        lines.push((paragraph.content ?? []).map((node: any) => node.text ?? '').join(''));
    }
    return lines.join('\n');
}

describe('Jira write-back flow', () => {
    // Fake Jira.
    let server: http.Server;
    let fakeJiraUrl = '';
    let jiraStatus = 200;
    const requests: RecordedRequest[] = [];
    const token = crypto.randomUUID();

    let adminClient: ApiClient;
    let approverClient: ApiClient;
    let teamId: string;
    let qaEnvId: string;
    let prodEnvId: string;
    let policyId: string;
    let featureId: string;
    let integrationId: string;
    let secret = '';
    let prodPendingRequestId = '';
    const issueKey = `PROJ-${Math.floor(Math.random() * 90_000) + 10_000}`;

    const issueRequests = (suffix: string) =>
        requests.filter(
            (r) =>
                r.method === 'POST' &&
                r.url.startsWith(`/rest/api/3/issue/${issueKey}/${suffix}`)
        );
    const comments = () => issueRequests('comment');
    const remoteLinks = () => issueRequests('remotelink');

    async function sendEvent(status: string, environment: string): Promise<AxiosResponse> {
        const response = await postEvent(integrationId, null, { Authorization: `Bearer ${secret}` }, JSON.stringify(webhookBody(issueKey, status, environment)));
        expectStatus(response, 200);
        return response;
    }

    async function listJobs(status: string) {
        const response = await adminClient.get(`/jira-integrations/${integrationId}/outbound-jobs`, {
            status,
            limit: 100,
        });
        expectStatus(response, 200);
        return response.data as { items: any[]; total: number };
    }

    beforeAll(async () => {
        // Fake Jira on a free port.
        server = http.createServer((req, res) => {
            const chunks: Buffer[] = [];
            req.on('data', (chunk) => chunks.push(chunk));
            req.on('end', () => {
                const text = Buffer.concat(chunks).toString('utf8');
                let body: any = text;
                try {
                    body = text ? JSON.parse(text) : undefined;
                } catch {
                    // keep the text
                }
                const url = req.url ?? '';
                requests.push({
                    method: req.method ?? '',
                    url,
                    authorization: req.headers.authorization,
                    body,
                });
                const reply = (status: number, payload: unknown) => {
                    res.writeHead(status, { 'Content-Type': 'application/json' });
                    res.end(JSON.stringify(payload));
                };
                if (jiraStatus !== 200) {
                    return reply(jiraStatus, { errorMessages: ['forced failure'] });
                }
                if (url.endsWith('/myself')) {
                    return reply(200, { displayName: 'FluxGate Bot', accountId: 'bot' });
                }
                if (url.includes('/comment')) {
                    return reply(201, { id: '10001' });
                }
                if (url.includes('/remotelink')) {
                    return reply(req.method === 'DELETE' ? 204 : 201, { id: 10002 });
                }
                return reply(404, {});
            });
        });
        await new Promise<void>((resolve) => server.listen(0, '0.0.0.0', resolve));
        fakeJiraUrl = `http://${FAKE_JIRA_HOST}:${(server.address() as AddressInfo).port}`;

        adminClient = await getApiClient();

        const team = await adminClient.post('/teams', createTeamFixture());
        expectStatus(team, 201);
        teamId = team.data.id;

        // Environment names are exactly "qa" and "prod": the comment and link
        // texts use the environment name.
        const qa = await adminClient.post(
            `/teams/${teamId}/environments`,
            createEnvironmentFixture({ name: 'qa', environmentType: 'Staging' })
        );
        expectStatus(qa, 201);
        qaEnvId = qa.data.id;

        const prod = await adminClient.post(
            `/teams/${teamId}/environments`,
            createEnvironmentFixture({ name: 'prod', environmentType: 'Production' })
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

        const approverFixture = createUserFixture({
            username: `jira-wb-approver-${Date.now()}-${Math.random().toString(16).slice(2, 6)}`,
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

        // The integration points at the fake Jira. Jira approves in qa only.
        const integration = await adminClient.post(`/teams/${teamId}/jira-integrations`, {
            name: uniqueName('jira-wb'),
            jiraBaseUrl: fakeJiraUrl,
            environmentField: ENVIRONMENT_FIELD,
            environmentAliases: { QA: qaEnvId, Prod: prodEnvId },
            jiraApprovedEnvironmentIds: [qaEnvId],
        });
        expectStatus(integration, 201);
        integrationId = integration.data.integration.id;
        secret = integration.data.secret;

        expectStatus(
            await adminClient.put(`/jira-integrations/${integrationId}/rules`, {
                rules: [
                    { jiraStatus: 'Ready for Release', action: 'approve' },
                    { jiraStatus: 'Done', action: 'deploy' },
                    { jiraStatus: 'Ready for Release', action: 'request', environmentIds: [prodEnvId] },
                ],
            }),
            200
        );

        // Write-back on: comments and remote link, Cloud (Basic auth) with a random token.
        const writeback = await adminClient.put(`/jira-integrations/${integrationId}/writeback`, {
            enabled: true,
            comments: true,
            remoteLink: true,
            authKind: 'cloud_basic',
            accountEmail: ACCOUNT_EMAIL,
            credential: token,
        });
        expectStatus(writeback, 200);
        expect(writeback.data.writeback.enabled).toBe(true);
        expect(writeback.data.writeback.hasCredential).toBe(true);
        expect(JSON.stringify(writeback.data)).not.toContain(token);

        expectStatus(
            await adminClient.post(`/features/${featureId}/external-links`, {
                system: 'jira',
                externalKey: issueKey,
                url: `${fakeJiraUrl}/browse/${issueKey}`,
            }),
            201
        );
    });

    afterAll(async () => {
        if (server) {
            await new Promise<void>((resolve) => server.close(() => resolve()));
        }
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

    it('test connection reaches the fake Jira with the token', async () => {
        const response = await adminClient.post(`/jira-integrations/${integrationId}/writeback/test`, {});
        expectStatus(response, 200);
        expect(response.data.ok).toBe(true);

        const call = requests.find((r) => r.method === 'GET' && r.url === '/rest/api/3/myself');
        expect(call).toBeDefined();
        const header = call!.authorization ?? '';
        expect(header.startsWith('Basic ')).toBe(true);
        const decoded = Buffer.from(header.slice('Basic '.length), 'base64').toString('utf8');
        expect(decoded).toBe(`${ACCOUNT_EMAIL}:${token}`);
    });

    it('a Jira approve event posts one comment with the outcome', async () => {
        const response = await sendEvent('Ready for Release', 'QA');
        expect(response.data.results.some((r: any) => r.action === 'approve' && r.outcome === 'applied')).toBe(true);

        await waitFor('comment with the approve outcome', () =>
            comments().find((r) => commentText(r.body).includes('qa: approve applied'))
        );
        const matching = comments().filter((r) => commentText(r.body).includes('qa: approve applied'));
        expect(matching).toHaveLength(1);
        expect(commentText(matching[0].body)).toContain("FluxGate: Jira status 'Ready for Release'");
        expect(matching[0].authorization?.startsWith('Basic ')).toBe(true);
    });

    it('a human approval in FluxGate posts a comment and updates the remote link', async () => {
        // The request rule leaves a pending request for prod. The approve rule is
        // refused there because prod is not Jira-approved.
        const response = await sendEvent('Ready for Release', 'Prod');
        const request = response.data.results.find((r: any) => r.action === 'request');
        expect(request.outcome).toBe('applied');
        prodPendingRequestId = request.approvalRequestId;

        const vote = await approverClient.post(`/approval-requests/${prodPendingRequestId}/approve`, {
            comment: 'approved in FluxGate',
        });
        expectStatus(vote, 200);

        await waitFor('comment "Approved for prod by"', () =>
            comments().find((r) => commentText(r.body).includes('Approved for prod by'))
        );
        const link = await waitFor('remote link with the prod stage status', () =>
            remoteLinks().find((r) => String(r.body?.object?.title ?? '').includes('prod DEPLOYMENT_APPROVED'))
        );
        expect(link.body.globalId).toBe(`fluxgate:feature:${featureId}`);
        expect(String(link.body.object.url)).toContain(featureId);
    }, 60_000);

    it('a Jira-made deploy gives no second comment', async () => {
        // qa is approved (test 2). "Done" in QA deploys it as the Jira user.
        const before = comments().length;
        const linksBefore = remoteLinks().length;

        const response = await sendEvent('Done', 'QA');
        expect(response.data.results.some((r: any) => r.action === 'deploy' && r.outcome === 'applied')).toBe(true);

        // The event comment is the one new comment.
        await waitFor('event comment for the deploy', () =>
            comments().find((r) => commentText(r.body).includes('qa: deploy applied'))
        );
        // The capture saw the Jira-made deploy: it refreshes the remote link only.
        await waitFor('remote link refresh for the deploy', () =>
            remoteLinks()
                .slice(linksBefore)
                .find((r) => String(r.body?.object?.title ?? '').includes('qa DEPLOYED'))
        );
        // Let any wrongly queued comment go out.
        await sleep(8_000);

        expect(comments().length).toBe(before + 1);
        expect(comments().filter((r) => commentText(r.body).includes('Deployed to qa by'))).toHaveLength(0);
    }, 60_000);

    it('a 401 from Jira pauses write-back', async () => {
        jiraStatus = 401;
        const callsBefore = requests.length;

        // prod is approved (test 3): "Done" in Prod deploys it and queues a comment.
        await sendEvent('Done', 'Prod');

        const paused = await waitFor('writeback.pausedReason', async () => {
            const response = await adminClient.get(`/jira-integrations/${integrationId}`);
            expectStatus(response, 200);
            return response.data.writeback.pausedReason ? response.data.writeback : undefined;
        });
        expect(String(paused.pausedReason).length).toBeGreaterThan(0);

        // A later change: the job queues and stays pending while paused.
        await sendEvent('Ready for Release', 'QA');
        const callsAtPause = requests.length;
        await sleep(7_000);
        expect(requests.length).toBe(callsAtPause);
        expect(callsAtPause).toBeGreaterThan(callsBefore);

        const pending = await listJobs('pending');
        expect(pending.total).toBeGreaterThanOrEqual(1);
        expect(pending.items.some((job) => job.kind === 'comment')).toBe(true);

        // Resume with a healthy Jira: the held jobs go out.
        jiraStatus = 200;
        expectStatus(await adminClient.post(`/jira-integrations/${integrationId}/writeback/resume`, {}), 200);
        await waitFor('pending jobs drained after resume', async () => (await listJobs('pending')).total === 0);
        const after = await adminClient.get(`/jira-integrations/${integrationId}`);
        expect(after.data.writeback.pausedReason).toBeNull();
    }, 60_000);

    it('a signed native webhook is accepted and a bad signature is 401', async () => {
        const generated = await adminClient.post(`/jira-integrations/${integrationId}/native-webhook-secret`, {});
        expectStatus(generated, 200);
        const nativeSecret: string = generated.data.secret;
        expect(nativeSecret.length).toBeGreaterThan(0);

        const raw = JSON.stringify(webhookBody(issueKey, 'Ready for Release', 'QA'));
        const signature = `sha256=${crypto.createHmac('sha256', nativeSecret).update(raw).digest('hex')}`;

        const ok = await postEvent(integrationId, null, { 'X-Hub-Signature': signature }, raw);
        expectStatus(ok, 200);
        expect(ok.data.duplicate).toBe(false);

        const wrong = `sha256=${crypto.createHmac('sha256', 'not-the-secret').update(raw).digest('hex')}`;
        const bad = await postEvent(
            integrationId,
            null,
            { 'X-Hub-Signature': wrong },
            JSON.stringify(webhookBody(issueKey, 'Ready for Release', 'QA'))
        );
        expectStatus(bad, 401);
    });

    it('bursts over the limit get 429', async () => {
        // Keep this test last: it fills the bucket of the integration.
        const responses = await Promise.all(
            Array.from({ length: 65 }, () =>
                postEvent(integrationId, null, { Authorization: `Bearer ${secret}` }, JSON.stringify(webhookBody(issueKey, 'Ready for Release', 'QA')))
            )
        );
        const limited = responses.filter((r) => r.status === 429);
        expect(limited.length).toBeGreaterThanOrEqual(1);
        expect(Number(limited[0].headers['retry-after'])).toBeGreaterThan(0);
        expect(responses.every((r) => r.status === 200 || r.status === 429)).toBe(true);
    });
});
