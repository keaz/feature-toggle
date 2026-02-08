import axios, { AxiosInstance, AxiosResponse, AxiosError } from 'axios';

/**
 * Configuration for the API client
 */
export interface ApiClientConfig {
    baseUrl: string;
    username: string;
    password: string;
}

/**
 * Default configuration - can be overridden via environment variables
 */
const defaultConfig: ApiClientConfig = {
    baseUrl: process.env.API_BASE_URL || 'http://localhost:8080/api/v1',
    username: process.env.API_USERNAME || 'admin',
    password: process.env.API_PASSWORD || 'password123',
};

/**
 * Login response from the auth endpoint
 */
interface LoginResponse {
    token: string;
    userId: string;
    expiresAt: string;
}

/**
 * API Client wrapper with authentication support
 */
export class ApiClient {
    private client: AxiosInstance;
    private token: string | null = null;
    private config: ApiClientConfig;

    constructor(config: Partial<ApiClientConfig> = {}) {
        this.config = { ...defaultConfig, ...config };
        this.client = axios.create({
            baseURL: this.config.baseUrl,
            headers: {
                'Content-Type': 'application/json',
            },
            validateStatus: () => true, // Don't throw on any status code
        });
    }

    /**
     * Authenticate with the API and store the JWT token
     */
    async authenticate(): Promise<void> {
        const response = await this.client.post<LoginResponse>('/auth/login', {
            username: this.config.username,
            password: this.config.password,
        });

        if (response.status !== 200) {
            throw new Error(`Authentication failed: ${response.status} - ${JSON.stringify(response.data)}`);
        }

        this.token = response.data.token;
        this.client.defaults.headers.common['Authorization'] = `Bearer ${this.token}`;
    }

    /**
     * Create admin user if it doesn't exist.
     * This is used as a prerequisite step before authentication.
     * Handles 409 Conflict gracefully (admin already exists).
     */
    async ensureAdminExists(): Promise<void> {
        console.log('🔧 Ensuring admin user exists...');

        const adminPayload = {
            username: this.config.username,
            password: this.config.password,
            firstName: 'Test',
            lastName: 'Admin',
            email: `${this.config.username}@test.local`,
        };

        const response = await this.client.post('/admins', adminPayload);

        if (response.status === 201) {
            console.log(`✅ Admin user '${this.config.username}' created successfully`);
        } else if (response.status === 409) {
            console.log(`ℹ️ Admin user '${this.config.username}' already exists`);
        } else {
            throw new Error(`Failed to create admin user: ${response.status} - ${JSON.stringify(response.data)}`);
        }
    }

    /**
     * Clear authentication (for testing unauthenticated requests)
     */
    clearAuth(): void {
        this.token = null;
        delete this.client.defaults.headers.common['Authorization'];
    }

    /**
     * Restore authentication after clearing
     */
    restoreAuth(): void {
        if (this.token) {
            this.client.defaults.headers.common['Authorization'] = `Bearer ${this.token}`;
        }
    }

    /**
     * Check if the client is authenticated
     */
    isAuthenticated(): boolean {
        return this.token !== null;
    }

    /**
     * GET request
     */
    async get<T = any>(url: string, params?: Record<string, unknown>): Promise<AxiosResponse<T>> {
        return this.client.get<T>(url, { params });
    }

    /**
     * POST request
     */
    async post<T = any>(url: string, data?: unknown): Promise<AxiosResponse<T>> {
        return this.client.post<T>(url, data);
    }

    /**
     * PATCH request
     */
    async patch<T = any>(url: string, data?: unknown): Promise<AxiosResponse<T>> {
        return this.client.patch<T>(url, data);
    }

    /**
     * PUT request
     */
    async put<T = any>(url: string, data?: unknown): Promise<AxiosResponse<T>> {
        return this.client.put<T>(url, data);
    }

    /**
     * DELETE request
     */
    async delete<T = any>(url: string): Promise<AxiosResponse<T>> {
        return this.client.delete<T>(url);
    }

    /**
     * Make a request without authentication (for testing auth failures)
     */
    async getUnauthenticated<T = any>(url: string): Promise<AxiosResponse<T>> {
        return axios.get<T>(`${this.config.baseUrl}${url}`, {
            headers: { 'Content-Type': 'application/json' },
            validateStatus: () => true,
        });
    }

    /**
     * Make a request with an invalid token (for testing auth validation)
     */
    async getWithInvalidToken<T = any>(url: string): Promise<AxiosResponse<T>> {
        return axios.get<T>(`${this.config.baseUrl}${url}`, {
            headers: {
                'Content-Type': 'application/json',
                'Authorization': 'Bearer invalid-token-12345',
            },
            validateStatus: () => true,
        });
    }
}

/**
 * Singleton instance for shared use across tests
 */
let sharedClient: ApiClient | null = null;

/**
 * Get or create a shared authenticated API client
 */
export async function getApiClient(): Promise<ApiClient> {
    if (!sharedClient) {
        sharedClient = new ApiClient();
        await sharedClient.ensureAdminExists();
        await sharedClient.authenticate();
    }
    return sharedClient;
}

/**
 * Create a new API client (for tests needing isolated state)
 */
export function createApiClient(config?: Partial<ApiClientConfig>): ApiClient {
    return new ApiClient(config);
}

/**
 * Reset the shared client (useful for test cleanup)
 */
export function resetSharedClient(): void {
    sharedClient = null;
}
