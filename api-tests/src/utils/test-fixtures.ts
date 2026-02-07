import { randomUUID } from 'crypto';

/**
 * Test fixtures - generates unique test data for each test run
 * All data uses UUIDs to prevent conflicts between test runs
 */

/**
 * Generate a unique name with a prefix
 */
export function uniqueName(prefix: string): string {
    const uuid = randomUUID().substring(0, 8);
    return `${prefix}-${uuid}`;
}

/**
 * Generate a unique email
 */
export function uniqueEmail(prefix: string = 'test'): string {
    const uuid = randomUUID().substring(0, 8);
    return `${prefix}-${uuid}@test.fluxgate.io`;
}

/**
 * Environment fixture data
 */
export function createEnvironmentFixture(overrides: Partial<{
    name: string;
    active: boolean;
    environmentType: string;
}> = {}) {
    return {
        name: overrides.name || uniqueName('env'),
        active: overrides.active ?? true,
        environmentType: overrides.environmentType || 'Development',
    };
}

/**
 * Context fixture data
 */
export function createContextFixture(overrides: Partial<{
    key: string;
    entries: string[];
}> = {}) {
    return {
        key: overrides.key || uniqueName('ctx'),
        entries: overrides.entries || ['value1', 'value2', 'value3'],
    };
}

/**
 * Team fixture data
 */
export function createTeamFixture(overrides: Partial<{
    name: string;
    description: string;
}> = {}) {
    return {
        name: overrides.name || uniqueName('team'),
        description: overrides.description || 'Test team created by API automation',
    };
}

/**
 * Role fixture data
 */
export function createRoleFixture(overrides: Partial<{
    name: string;
    permissions: string[];
}> = {}) {
    return {
        name: overrides.name || uniqueName('role'),
        permissions: overrides.permissions || ['read:features', 'write:features'],
    };
}

/**
 * User fixture data
 */
export function createUserFixture(overrides: Partial<{
    username: string;
    email: string;
    firstName: string;
    lastName: string;
    password: string;
}> = {}) {
    const uuid = randomUUID().substring(0, 8);
    return {
        username: overrides.username || `user-${uuid}`,
        email: overrides.email || uniqueEmail('user'),
        firstName: overrides.firstName || 'Test',
        lastName: overrides.lastName || 'User',
        password: overrides.password || 'TestPassword123!',
    };
}

/**
 * Client fixture data
 */
export function createClientFixture(overrides: Partial<{
    name: string;
    description: string;
    clientType: 'WEB' | 'BACKEND';
    environmentId: string;
    webOrigins: string[];
}> = {}) {
    return {
        name: overrides.name || uniqueName('client'),
        description: overrides.description || 'Test client for API automation',
        clientType: overrides.clientType || 'BACKEND',
        environmentId: overrides.environmentId || '',
        webOrigins: overrides.webOrigins || [],
    };
}

/**
 * Feature fixture data
 */
export function createFeatureFixture(overrides: Partial<{
    name: string;
    description: string;
    featureType: 'SIMPLE' | 'CONTEXTUAL';
    defaultValue: boolean;
}> = {}) {
    return {
        name: overrides.name || uniqueName('feature'),
        description: overrides.description || 'Test feature for API automation',
        featureType: overrides.featureType || 'SIMPLE',
        defaultValue: overrides.defaultValue ?? false,
    };
}

/**
 * Pipeline fixture data
 */
export function createPipelineFixture(overrides: Partial<{
    name: string;
    description: string;
    stages: Array<{ name: string; order: number; environmentId?: string }>;
}> = {}) {
    return {
        name: overrides.name || uniqueName('pipeline'),
        description: overrides.description || 'Test pipeline for API automation',
        stages: overrides.stages || [
            { name: 'Development', order: 1 },
            { name: 'Staging', order: 2 },
            { name: 'Production', order: 3 },
        ],
        relationships: [],
    };
}

/**
 * Approval Policy fixture data
 */
export function createApprovalPolicyFixture(overrides: Partial<{
    name: string;
    requiredApprovers: number;
    approverRoles: string[];
    appliesTo: 'all' | 'production_only' | 'specific_environments';
}> = {}) {
    return {
        name: overrides.name || uniqueName('policy'),
        requiredApprovers: overrides.requiredApprovers || 1,
        approverRoles: overrides.approverRoles || ['Admin'],
        appliesTo: overrides.appliesTo || 'all',
    };
}

/**
 * Criterion (targeting rule) fixture data
 */
export function createCriterionFixture(overrides: Partial<{
    contextKey: string;
    operator: string;
    value: string;
    priority: number;
}> = {}) {
    return {
        contextKey: overrides.contextKey || 'userId',
        operator: overrides.operator || 'EQUALS',
        value: overrides.value || 'test-user-123',
        priority: overrides.priority || 1,
    };
}

/**
 * Validation test data - intentionally invalid data for testing validation
 */
export const invalidData = {
    emptyName: { name: '' },
    tooLongName: { name: 'a'.repeat(500) },
    invalidEmail: { email: 'not-an-email' },
    missingRequired: {},
    invalidUuid: { id: 'not-a-uuid' },
    negativeNumber: { requiredApprovers: -1 },
    specialChars: { name: '<script>alert("xss")</script>' },
};
