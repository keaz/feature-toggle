import axios from 'axios';
import { createApiClient, getApiClient } from '../utils/api-client.js';
import { expectStatus } from '../utils/test-utils.js';

/**
 * CLI device login (fluxgate login --use-device-code)
 *
 * Endpoints:
 * - POST /api/v1/auth/device/authorize - start (public)
 * - POST /api/v1/auth/device/token - poll (public)
 * - POST /api/v1/auth/device/approve - approve or deny (signed in)
 */
describe('CLI device login API', () => {
    const baseUrl = process.env.API_BASE_URL || 'http://127.0.0.1:18080/api/v1';

    it('releases one working session after a person approves the code', async () => {
        const cli = createApiClient();
        const started = await cli.post('/auth/device/authorize', {});
        expectStatus(started, 200);
        const { deviceCode, userCode, verificationUriComplete, interval, expiresIn } = started.data;
        expect(userCode).toMatch(/^[BCDFGHJKMNPQRSTVWXZ]{4}-[BCDFGHJKMNPQRSTVWXZ]{4}$/);
        expect(verificationUriComplete).toContain(`/device?code=${userCode}`);
        expect(interval).toBe(5);
        expect(expiresIn).toBe(600);

        const pending = await cli.post('/auth/device/token', { deviceCode });
        expectStatus(pending, 400);
        expect(pending.data.code).toBe('authorization_pending');

        const person = await getApiClient();
        const approved = await person.post('/auth/device/approve', {
            userCode: userCode.replace('-', '').toLowerCase(),
            approve: true,
        });
        expectStatus(approved, 200);
        expect(approved.data.status).toBe('approved');

        const session = await cli.post('/auth/device/token', { deviceCode });
        expectStatus(session, 200);
        expect(session.data.refreshToken).toBeTruthy();

        // The new access token works on a protected route.
        const teams = await axios.get(`${baseUrl}/teams`, {
            headers: { Authorization: `Bearer ${session.data.token}` },
            validateStatus: () => true,
        });
        expectStatus(teams, 200);

        const again = await cli.post('/auth/device/token', { deviceCode });
        expectStatus(again, 400);
        expect(again.data.code).toBe('expired_token');
    });

    it('reports a denial and refuses approval without a session', async () => {
        const cli = createApiClient();
        const started = await cli.post('/auth/device/authorize', {});
        expectStatus(started, 200);

        const anonymous = await cli.post('/auth/device/approve', {
            userCode: started.data.userCode,
            approve: true,
        });
        expectStatus(anonymous, 401);

        const person = await getApiClient();
        const denied = await person.post('/auth/device/approve', {
            userCode: started.data.userCode,
            approve: false,
        });
        expectStatus(denied, 200);

        const poll = await cli.post('/auth/device/token', { deviceCode: started.data.deviceCode });
        expectStatus(poll, 400);
        expect(poll.data.code).toBe('access_denied');
    });
});
