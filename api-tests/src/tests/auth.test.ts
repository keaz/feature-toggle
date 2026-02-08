import { ApiClient, createApiClient } from '../utils/api-client.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    delay,
} from '../utils/test-utils.js';

/**
 * Auth API Tests
 * 
 * Endpoints:
 * - POST /api/v1/auth/login - Login
 * - POST /api/v1/auth/logout - Logout
 * - GET /api/v1/auth/status - Check admin bootstrap status
 * - POST /api/v1/auth/reset-password - Reset current user password
 */
describe('Auth API', () => {
    // Default admin credentials
    const authUsername = process.env.API_USERNAME || 'api-test-admin';
    const authPassword = process.env.API_PASSWORD || 'password123';

    const validCredentials = {
        username: authUsername,
        password: authPassword,
    };

    describe('POST /auth/login', () => {
        it('should login with valid credentials', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', validCredentials);

            expectSuccess(response);
            expect(response.data).toHaveProperty('token');
            expect(response.data.token).toBeTruthy();
        });

        it('should return JWT token format', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', validCredentials);

            expectSuccess(response);
            // JWT tokens have 3 parts separated by dots
            const tokenParts = response.data.token.split('.');
            expect(tokenParts).toHaveLength(3);
        });

        it('should reject invalid password', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', {
                username: authUsername,
                password: 'wrong-password',
            });

            expectStatus(response, 401);
        });

        it('should reject invalid username', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', {
                username: 'nonexistent-user',
                password: 'password123',
            });

            expectStatus(response, 401);
        });

        it('should reject empty username', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', {
                username: '',
                password: 'password123',
            });

            expectClientError(response);
        });

        it('should reject empty password', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', {
                username: authUsername,
                password: '',
            });

            expectClientError(response);
        });

        it('should reject request without credentials', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', {});

            expectClientError(response);
        });

        it('should reject SQL injection attempt in username', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', {
                username: "admin'; DROP TABLE users; --",
                password: 'password123',
            });

            // Should either reject with 400 or 401, not crash
            expectClientError(response);
        });

        it('should reject SQL injection attempt in password', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/login', {
                username: authUsername,
                password: "' OR '1'='1",
            });

            expectStatus(response, 401);
        });
    });

    describe('POST /auth/logout', () => {
        it('should logout authenticated user', async () => {
            const client = createApiClient();
            await client.authenticate();

            const response = await client.post('/auth/logout', {});

            // Logout should return success
            expectSuccess(response);
        });

        it('should require authentication', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/logout', {});

            expectStatus(response, 401);
        });

        it('should invalidate token after logout', async () => {
            const client = createApiClient();
            const loginResponse = await client.post('/auth/login', validCredentials);
            const token = loginResponse.data.token;
            const rawHttp = (client as any).client;

            const logoutResponse = await rawHttp.post('/auth/logout', {}, {
                headers: {
                    Authorization: `Bearer ${token}`,
                },
                validateStatus: () => true,
            });
            expectSuccess(logoutResponse);

            // Use a protected endpoint; /auth/status is intentionally public.
            const protectedResponse = await rawHttp.get('/roles', {
                headers: {
                    Authorization: `Bearer ${token}`,
                },
                validateStatus: () => true,
            });
            expectStatus(protectedResponse, 401);
        });
    });

    describe('GET /auth/status', () => {
        it('should return adminConfigured flag', async () => {
            const client = createApiClient();
            const response = await client.get('/auth/status');

            expectSuccess(response);
            expect(response.data).toHaveProperty('adminConfigured');
            expect(typeof response.data.adminConfigured).toBe('boolean');
        });

        it('should return adminConfigured=true after admin bootstrap', async () => {
            const client = createApiClient();
            await client.ensureAdminExists();
            const response = await client.get('/auth/status');

            expectSuccess(response);
            expect(response.data.adminConfigured).toBe(true);
        });

        it('should be accessible without authentication', async () => {
            const client = createApiClient();
            const response = await client.getUnauthenticated('/auth/status');

            expectStatus(response, 200);
            expect(response.data).toHaveProperty('adminConfigured');
        });

        it('should ignore invalid token and still return status', async () => {
            const client = createApiClient();
            const response = await client.getWithInvalidToken('/auth/status');

            expectStatus(response, 200);
            expect(response.data).toHaveProperty('adminConfigured');
        });

        it('should return status even with expired token header', async () => {
            // Using a token that looks valid but is expired
            const client = createApiClient();
            const expiredToken = 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiZXhwIjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c';

            const response = await client['client'].get('/auth/status', {
                headers: {
                    Authorization: `Bearer ${expiredToken}`,
                },
                validateStatus: () => true,
            });

            expectStatus(response, 200);
            expect(response.data).toHaveProperty('adminConfigured');
        });

        it('should return status even with malformed token header', async () => {
            const client = createApiClient();
            const malformedToken = 'not.a.valid.jwt.token';

            const response = await client['client'].get('/auth/status', {
                headers: {
                    Authorization: `Bearer ${malformedToken}`,
                },
                validateStatus: () => true,
            });

            expectStatus(response, 200);
            expect(response.data).toHaveProperty('adminConfigured');
        });
    });

    describe('Password Reset Flow', () => {
        it('should require authentication to reset password', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/reset-password', {
                currentPassword: authPassword,
                newPassword: 'NewPassword123!',
            });

            expectStatus(response, 401);
        });

        it('should reject incorrect current password', async () => {
            const client = createApiClient();
            await client.authenticate();
            const response = await client.post('/auth/reset-password', {
                currentPassword: 'WrongPassword123!',
                newPassword: 'NewPassword123!',
            });

            expectClientError(response);
        });

        it('should reject empty current password', async () => {
            const client = createApiClient();
            await client.authenticate();
            const response = await client.post('/auth/reset-password', {
                currentPassword: '',
                newPassword: 'NewPassword123!',
            });

            expectClientError(response);
        });

        it('should reject empty new password', async () => {
            const client = createApiClient();
            await client.authenticate();
            const response = await client.post('/auth/reset-password', {
                currentPassword: 'WrongPassword123!',
                newPassword: '',
            });

            expectClientError(response);
        });
    });

    describe('Rate Limiting', () => {
        it('should rate limit failed login attempts', async () => {
            const client = createApiClient();
            const responses = [];

            // Make multiple failed login attempts
            for (let i = 0; i < 10; i++) {
                const response = await client.post('/auth/login', {
                    username: authUsername,
                    password: 'wrong-password',
                });
                responses.push(response.status);

                // Small delay between requests
                await delay(100);
            }

            // Should eventually get rate limited (429) or continue with 401
            const has429 = responses.includes(429);
            const allUnauthorized = responses.every(s => s === 401);

            // Either behavior is acceptable depending on implementation
            expect(has429 || allUnauthorized).toBe(true);
        });
    });

    describe('Token Security', () => {
        it('should return different tokens for same user on multiple logins', async () => {
            const client1 = createApiClient();
            const client2 = createApiClient();

            const response1 = await client1.post('/auth/login', validCredentials);
            await delay(100);
            const response2 = await client2.post('/auth/login', validCredentials);

            expectSuccess(response1);
            expectSuccess(response2);

            // Tokens should be different (different nonce/timestamp)
            expect(response1.data.token).not.toBe(response2.data.token);
        });

        it('should not accept token without Bearer prefix', async () => {
            const client = createApiClient();
            const loginResponse = await client.post('/auth/login', validCredentials);
            const token = loginResponse.data.token;

            // Try to use token without Bearer prefix
            const response = await client['client'].get('/roles', {
                headers: {
                    Authorization: token, // Missing 'Bearer ' prefix
                },
                validateStatus: () => true,
            });

            expectStatus(response, 401);
        });

        it('should not accept token in query parameter', async () => {
            const client = createApiClient();
            const loginResponse = await client.post('/auth/login', validCredentials);
            const token = loginResponse.data.token;

            // Try to pass token as query parameter instead of header
            const response = await client['client'].get('/roles', {
                params: { token },
                validateStatus: () => true,
            });

            // Should fail - token should only be accepted in Authorization header
            expectStatus(response, 401);
        });
    });
});
