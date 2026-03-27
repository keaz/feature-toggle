import { AxiosResponse } from 'axios';
import { ApiClient } from './api-client.js';

/**
 * Common test utilities for API tests
 */

/**
 * Assert that a response has the expected status code
 */
export function expectStatus(response: AxiosResponse, expectedStatus: number): void {
    if (response.status !== expectedStatus) {
        console.error('Response body:', JSON.stringify(response.data, null, 2));
    }
    expect(response.status).toBe(expectedStatus);
}

/**
 * Assert that a response indicates success (2xx)
 */
export function expectSuccess(response: AxiosResponse): void {
    if (response.status < 200 || response.status >= 300) {
        if (response.status >= 300 && response.status < 400) {
            const location = (response.headers as Record<string, string | undefined>)?.location;
            const method = response.config?.method?.toUpperCase() || 'REQUEST';
            const reqPath = response.config?.url || '';
            console.error(
                `Redirect response: ${response.status} for ${method} ${reqPath}` +
                `${location ? ` -> ${location}` : ''}`
            );
        }
        console.error('Response body:', JSON.stringify(response.data, null, 2));
    }
    expect(response.status).toBeGreaterThanOrEqual(200);
    expect(response.status).toBeLessThan(300);
}

/**
 * Assert that a response indicates a client error (4xx)
 */
export function expectClientError(response: AxiosResponse): void {
    expect(response.status).toBeGreaterThanOrEqual(400);
    expect(response.status).toBeLessThan(500);
}

/**
 * Assert that a response has an error message
 */
export function expectErrorMessage(response: AxiosResponse): void {
    expect(response.data).toHaveProperty('error');
}

/**
 * Assert the response body has expected properties
 */
export function expectProperties(response: AxiosResponse, properties: string[]): void {
    properties.forEach(prop => {
        expect(response.data).toHaveProperty(prop);
    });
}

/**
 * Assert a paginated response structure
 */
export function expectPaginatedResponse(response: AxiosResponse): void {
    expect(response.data).toHaveProperty('items');
    expect(response.data).toHaveProperty('meta');
    expect(Array.isArray(response.data.items)).toBe(true);
    expect(response.data.meta).toHaveProperty('total');
}

/**
 * Wait for a specified duration (useful for rate limit tests)
 */
export function delay(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Clean up a created resource by ID
 */
export async function cleanupResource(
    client: ApiClient,
    url: string,
    resourceId: string
): Promise<void> {
    try {
        await client.delete(`${url}/${resourceId}`);
    } catch {
        // Ignore cleanup errors - resource may already be deleted
    }
}

/**
 * Create a resource and return its ID, with automatic cleanup registration
 */
export async function createResourceWithCleanup<T extends { id: string }>(
    client: ApiClient,
    url: string,
    data: Record<string, unknown>,
    cleanupList: Array<{ url: string; id: string }>
): Promise<T> {
    const response = await client.post<T>(url, data);
    expectSuccess(response);

    cleanupList.push({ url, id: response.data.id });
    return response.data;
}

/**
 * Clean up all resources in a cleanup list
 */
export async function cleanupAll(
    client: ApiClient,
    cleanupList: Array<{ url: string; id: string }>
): Promise<void> {
    // Clean up in reverse order (last created first)
    for (const item of cleanupList.reverse()) {
        await cleanupResource(client, item.url, item.id);
    }
}

/**
 * Known seeded test team ID - used for tests that need an existing team
 */
export const TEST_TEAM_ID = '51ecc366-f1cd-4d3d-ab73-fa60bad98f27';

/**
 * Known seeded test environment ID
 */
export const TEST_ENVIRONMENT_ID = '51ecc366-f1cd-4d3d-ab73-fa60bad98f27';

/**
 * UUID validation regex
 */
export const UUID_REGEX = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * Assert that a value is a valid UUID
 */
export function expectUuid(value: string): void {
    expect(value).toMatch(UUID_REGEX);
}

/**
 * Assert that a value is a valid ISO date string
 */
export function expectIsoDate(value: string): void {
    expect(() => new Date(value)).not.toThrow();
    expect(new Date(value).toISOString()).toBeTruthy();
}

/**
 * Helper to update a feature by fetching current state, merging updates, and sending full payload
 * Handles the mapping between FeatureResponse and UpdateFeatureRequest structures
 */
export async function updateFeature(
    client: ApiClient,
    featureId: string,
    updates: Record<string, any>
): Promise<AxiosResponse> {
    // 1. Get current feature
    const current = await client.get(`/features/${featureId}`);
    expectSuccess(current);

    // 2. Map response to request structure
    const feature = current.data;
    const payload = {
        key: feature.key,
        description: feature.description,
        featureType: feature.featureType,
        enabled: feature.enabled,
        dependencies: feature.dependencies || [],
        relationships: feature.relationships ? feature.relationships.map((r: any) => ({
            sourceId: r.sourceId,
            targetId: r.targetId
        })) : [],
        stages: feature.stages ? feature.stages.map((s: any) => ({
            environmentId: s.environment.id,
            orderIndex: s.orderIndex,
            position: s.position,
            bucketingKey: s.bucketingKey
        })) : [],
        variants: feature.variants ? feature.variants.map((v: any) => ({
            control: v.control,
            value: v.value,
            valueType: v.valueType,
            description: v.description
        })) : undefined,

        ...updates
    };

    // 3. Send update
    return client.patch(`/features/${featureId}`, payload);
}
