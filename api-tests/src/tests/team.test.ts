import { ApiClient, getApiClient, createApiClient } from '../utils/api-client.js';
import { createTeamFixture } from '../utils/test-fixtures.js';
import {
    expectStatus,
    expectSuccess,
    expectClientError,
    expectUuid,
    expectIsoDate,
    cleanupResource,
} from '../utils/test-utils.js';

/**
 * Team API Tests
 * 
 * Endpoints:
 * - GET /api/v1/teams - List teams (for current user)
 * - POST /api/v1/teams - Create team
 * - PATCH /api/v1/teams/{id} - Update team
 */
describe('Team API', () => {
    let client: ApiClient;
    const createdTeamIds: string[] = [];

    beforeAll(async () => {
        client = await getApiClient();
    });

    afterAll(async () => {
        // Note: Team deletion may not be available or allowed
        // These are here for safety but may need adjustment
        for (const id of createdTeamIds) {
            try {
                await client.delete(`/teams/${id}`);
            } catch {
                // Ignore cleanup errors
            }
        }
    });

    describe('GET /teams', () => {
        it('should list teams for authenticated user', async () => {
            const response = await client.get('/teams');

            expectSuccess(response);
            expect(Array.isArray(response.data)).toBe(true);
        });

        it('should return teams with expected properties', async () => {
            const response = await client.get('/teams');

            expectSuccess(response);
            if (response.data.length > 0) {
                const team = response.data[0];
                expect(team).toHaveProperty('id');
                expect(team).toHaveProperty('name');
                expect(team).toHaveProperty('createdAt');
            }
        });

        it('should return 401 without authentication', async () => {
            const response = await client.getUnauthenticated('/teams');

            expectStatus(response, 401);
        });

        it('should return 401 with invalid token', async () => {
            const response = await client.getWithInvalidToken('/teams');

            expectStatus(response, 401);
        });
    });

    describe('POST /teams', () => {
        it('should create a new team', async () => {
            const fixture = createTeamFixture();
            const response = await client.post('/teams', fixture);

            expectStatus(response, 201);
            expectUuid(response.data.id);
            expect(response.data.name).toBe(fixture.name);
            expect(response.data.description).toBe(fixture.description);

            createdTeamIds.push(response.data.id);
        });

        it('should create team with just a name', async () => {
            const fixture = { name: createTeamFixture().name };
            const response = await client.post('/teams', fixture);

            expectStatus(response, 201);
            expect(response.data.name).toBe(fixture.name);

            createdTeamIds.push(response.data.id);
        });

        it('should reject duplicate team names', async () => {
            const fixture = createTeamFixture();

            // Create first team
            const first = await client.post('/teams', fixture);
            expectStatus(first, 201);
            createdTeamIds.push(first.data.id);

            // Try to create duplicate
            const duplicate = await client.post('/teams', fixture);
            expectStatus(duplicate, 409); // Conflict
        });

        it('should reject empty name', async () => {
            const response = await client.post('/teams', {
                name: '',
                description: 'Test description',
            });

            expectClientError(response);
        });

        it('should reject request without name', async () => {
            const response = await client.post('/teams', {
                description: 'Just a description',
            });

            expectClientError(response);
        });

        it('should reject name with special characters', async () => {
            const response = await client.post('/teams', {
                name: '<script>alert("xss")</script>',
            });

            // Should either reject or sanitize the name
            expectClientError(response);
        });

        it('should reject very long name', async () => {
            const response = await client.post('/teams', {
                name: 'a'.repeat(500),
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const fixture = createTeamFixture();
            const response = await unauthClient.post('/teams', fixture);

            expectStatus(response, 401);
        });
    });

    describe('PATCH /teams/{id}', () => {
        let testTeamId: string;

        beforeAll(async () => {
            // Create a test team
            const fixture = createTeamFixture();
            const response = await client.post('/teams', fixture);
            testTeamId = response.data.id;
            createdTeamIds.push(testTeamId);
        });

        it('should update team name', async () => {
            const newName = createTeamFixture().name;
            const response = await client.patch(`/teams/${testTeamId}`, {
                name: newName,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
        });

        it('should update team description', async () => {
            const newDescription = 'Updated description for testing';
            const response = await client.patch(`/teams/${testTeamId}`, {
                description: newDescription,
            });

            expectSuccess(response);
            expect(response.data.description).toBe(newDescription);
        });

        it('should update both name and description', async () => {
            const newName = createTeamFixture().name;
            const newDescription = 'Both updated';
            const response = await client.patch(`/teams/${testTeamId}`, {
                name: newName,
                description: newDescription,
            });

            expectSuccess(response);
            expect(response.data.name).toBe(newName);
            expect(response.data.description).toBe(newDescription);
        });

        it('should return 404 for non-existent ID', async () => {
            const fakeId = '00000000-0000-0000-0000-000000000000';
            const response = await client.patch(`/teams/${fakeId}`, {
                name: 'New Name',
            });

            expectStatus(response, 404);
        });

        it('should reject empty name update', async () => {
            const response = await client.patch(`/teams/${testTeamId}`, {
                name: '',
            });

            expectClientError(response);
        });

        it('should return 400 for invalid UUID format', async () => {
            const response = await client.patch('/teams/invalid-uuid', {
                name: 'New Name',
            });

            expectClientError(response);
        });

        it('should return 401 without authentication', async () => {
            const unauthClient = createApiClient();
            const response = await unauthClient.patch(`/teams/${testTeamId}`, {
                name: 'Unauthorized Update',
            });

            expectStatus(response, 401);
        });
    });
});
