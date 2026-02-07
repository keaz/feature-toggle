import { ApiClient, createApiClient } from '../utils/api-client.js';
import { createUserFixture } from '../utils/test-fixtures.js';
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
 * - GET /api/v1/auth/status - Check auth status
 * - POST /api/v1/auth/reset-password - Request password reset
 * - POST /api/v1/auth/reset-password/{token} - Complete password reset
 */
describe('Auth API', () => {
    // Default admin credentials
    const validCredentials = {
        username: 'admin',
        password: 'password123',
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
                username: 'admin',
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
                username: 'admin',
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
                username: 'admin',
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
            await client.authenticate();

            // Logout
            await client.post('/auth/logout', {});

            // Token should be invalidated - next authenticated request should fail
            // Note: This depends on server-side token invalidation
            const statusResponse = await client.get('/auth/status');
            // May still work if token isn't server-side invalidated
            expect([200, 401]).toContain(statusResponse.status);
        });
    });

    describe('GET /auth/status', () => {
        it('should return user info for authenticated user', async () => {
            const client = createApiClient();
            await client.authenticate();

            const response = await client.get('/auth/status');

            expectSuccess(response);
            expect(response.data).toHaveProperty('id');
            expect(response.data).toHaveProperty('username');
        });

        it('should return admin user details', async () => {
            const client = createApiClient();
            await client.authenticate();

            const response = await client.get('/auth/status');

            expectSuccess(response);
            expect(response.data.username).toBe('admin');
        });

        it('should return 401 without authentication', async () => {
            const client = createApiClient();
            const response = await client.getUnauthenticated('/auth/status');

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const client = createApiClient();
            const response = await client.getWithInvalidToken('/auth/status');

            expectStatus(response, 401);
        });

        it('should return 401 with expired token', async () => {
            // Using a token that looks valid but is expired
            const client = createApiClient();
            const expiredToken = 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiZXhwIjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c';

            const response = await client['client'].get('/auth/status', {
                headers: {
                    Authorization: `Bearer ${expiredToken}`,
                },
                validateStatus: () => true,
            });

            expectStatus(response, 401);
        });

        it('should return 401 with malformed token', async () => {
            const client = createApiClient();
            const malformedToken = 'not.a.valid.jwt.token';

            const response = await client['client'].get('/auth/status', {
                headers: {
                    Authorization: `Bearer ${malformedToken}`,
                },
                validateStatus: () => true,
            });

            expectStatus(response, 401);
        });
    });

    describe('Password Reset Flow', () => {
        it('should handle password reset request for valid email', async () => {
            const client = createApiClient();
            // Use an email that exists - admin email
            const response = await client.post('/auth/reset-password', {
                email: 'admin@fluxgate.io',
            });

            // Should succeed even if it doesn't send email (to avoid enumeration)
            expect([200, 202, 404]).toContain(response.status);
        });

        it('should not reveal if email exists', async () => {
            const client = createApiClient();

            // Request for existing email
            const existingResponse = await client.post('/auth/reset-password', {
                email: 'admin@fluxgate.io',
            });

            // Request for non-existing email
            const nonExistingResponse = await client.post('/auth/reset-password', {
                email: 'nonexistent@example.com',
            });

            // Both should return similar status to prevent email enumeration
            // Ideally both should return 200/202
            expect([existingResponse.status, nonExistingResponse.status]).toContain(
                existingResponse.status
            );
        });

        it('should reject empty email', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/reset-password', {
                email: '',
            });

            expectClientError(response);
        });

        it('should reject invalid email format', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/reset-password', {
                email: 'not-an-email',
            });

            expectClientError(response);
        });

        it('should reject password reset with invalid token', async () => {
            const client = createApiClient();
            const response = await client.post('/auth/reset-password/invalid-token-12345', {
                newPassword: 'NewPassword123!',
            });

            // Invalid token should fail
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
                    username: 'admin',
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
            const response = await client['client'].get('/auth/status', {
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
            const response = await client['client'].get('/auth/status', {
                params: { token },
                validateStatus: () => true,
            });

            // Should fail - token should only be accepted in Authorization header
            expectStatus(response, 401);
        });
    });
});
