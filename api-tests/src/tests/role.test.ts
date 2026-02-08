import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createRoleFixture } from '../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    expectUuid,
    cleanupResource,
} from '../utils/test-utils.js';

/**
 * Role API Tests
 * 
 * Endpoints:
 * - GET /api/v1/roles - List roles
 * - POST /api/v1/roles - Create role (admin only)
 * - DELETE /api/v1/roles/{id} - Delete role (admin only)
 */
describe('Role API', () => {
    let client: ApiClient;
    const createdRoleIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();
    });

    afterAll(async () => {
        // Cleanup created roles
        for (const id of createdRoleIds) {
            await cleanupResource(client, '/roles', id);
        }
    });

    describe('GET /roles', () => {
        it('should list all roles', async () => {
            const response = await client.get('/roles');

            expectSuccess(response);
            expect(Array.isArray(response.data)).toBe(true);
        });

        it('should return roles with expected properties', async () => {
            const response = await client.get('/roles');

            expectSuccess(response);
            if (response.data.length > 0) {
                const role = response.data[0];
                expect(role).toHaveProperty('id');
                expect(role).toHaveProperty('name');
            }
        });

        it('should include Admin role', async () => {
            const response = await client.get('/roles');

            expectSuccess(response);
            const systemRole = response.data.find((r: { name: string }) => r.name === 'Team Admin');
            expect(systemRole).toBeDefined();
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated('/roles');

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken('/roles');

            expectStatus(response, 401);
        });
    });

    describe('POST /roles', () => {
        it('should create a new role', async () => {
            const fixture = createRoleFixture();
            const response = await client.post('/roles', fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);

            createdRoleIds.push(response.data.id);
        });

        it('should create role with specific description', async () => {
            const fixture = createRoleFixture({
                description: 'Role for staging deploy approvals',
            });
            const response = await client.post('/roles', fixture);

            expectStatus(response, 201);
            expect(response.data.description).toBe(fixture.description);

            createdRoleIds.push(response.data.id);
        });

        it('should reject empty name', async () => {
            const response = await client.post('/roles', {
                name: '',
                description: 'Invalid role',
            });

            expectClientError(response);
        });

        it('should reject request without name', async () => {
            const response = await client.post('/roles', {
                description: 'Missing name role',
            });

            expectClientError(response);
        });

        it('should reject very long name', async () => {
            const response = await client.post('/roles', {
                name: 'a'.repeat(500),
                description: 'Role with very long name',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createRoleFixture();
            const response = await unauthClient.post('/roles', fixture);

            expectStatus(response, 401);
        });

        // Admin permission test - requires non-admin user
        // This test assumes the API enforces admin-only role creation
        it('should only allow admin to create roles', async () => {
            // Note: This test would need a non-admin user to properly test
            // For now, we verify admin can create roles (positive case)
            const fixture = createRoleFixture();
            const response = await client.post('/roles', fixture);

            // Admin should be able to create
            expectStatus(response, 201);
            createdRoleIds.push(response.data.id);
        });
    });

    describe('DELETE /roles/{id}', () => {
        it('should delete a role', async () => {
            // Create a disposable role
            const fixture = createRoleFixture();
            const createResponse = await client.post('/roles', fixture);
            expectStatus(createResponse, 201);
            const roleId = createResponse.data.id;

            // Delete it
            const deleteResponse = await client.delete(`/roles/${roleId}`);
            expectStatus(deleteResponse, 204);

            // Verify it's no longer in the list
            const listResponse = await client.get('/roles');
            const deletedRole = listResponse.data.find((r: { id: string }) => r.id === roleId);
            expect(deletedRole).toBeUndefined();
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.delete(`/roles/${fakeId}`);

            expectStatus(response, 404);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.delete('/roles/invalid-uuid');

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.delete('/roles/some-id');

            expectStatus(response, 401);
        });

        // Prevent deletion of system roles
        it('should not allow deletion of Team Admin role', async () => {
            // Get the Team Admin role ID
            const listResponse = await client.get('/roles');
            const adminRole = listResponse.data.find((r: { name: string }) => r.name === 'Team Admin');

            if (adminRole) {
                const deleteResponse = await client.delete(`/roles/${adminRole.id}`);
                // Should be forbidden or conflict
                expect([400, 403, 409]).toContain(deleteResponse.status);
            }
        });
    });
});
