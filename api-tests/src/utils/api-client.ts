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
    baseUrl: process.env.API_BASE_URL || 'http://127.0.0.1:18080/api/v1',
    username: process.env.API_USERNAME || 'api-test-admin',
    password: process.env.API_PASSWORD || 'password123',
};

/**
 * Login response from the auth endpoint
 */
interface LoginResponse {
    token: string;
    user?: {
        id: string;
        username: string;
    };
    isTemporary?: boolean;
}

interface AuthStatusResponse {
    adminConfigured: boolean;
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
            maxRedirects: 0,
            validateStatus: () => true, // Don't throw on any status code
        });
    }

    /**
     * Retry an authenticated request once if token became invalid.
     * This keeps long-running suites stable when tokens are revoked mid-run.
     */
    private async withAuthRetry<T>(
        path: string,
        request: () => Promise<AxiosResponse<T>>
    ): Promise<AxiosResponse<T>> {
        const response = await request();
        if (response.status !== 401 || !this.token) {
            return response;
        }

        // Do not recurse on auth bootstrap endpoints.
        if (
            path === '/auth/login' ||
            path === '/auth/status' ||
            path === '/admins'
        ) {
            return response;
        }

        console.warn(`Received 401 for ${path}; re-authenticating and retrying once`);
        await this.authenticate();
        return request();
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
            if (response.status >= 300 && response.status < 400) {
                const location = (response.headers as Record<string, string | undefined>)?.location;
                throw new Error(
                    `Authentication failed with redirect: ${response.status} from ${this.config.baseUrl}/auth/login` +
                    `${location ? ` -> ${location}` : ''}. ` +
                    'This usually means API_BASE_URL points to a proxy/frontend or wrong backend endpoint.'
                );
            }
            if (response.status === 500 && (response.data as any)?.error === 'internal') {
                throw new Error(
                    `Authentication failed: ${response.status} - ${JSON.stringify(response.data)}. ` +
                    'Backend reported a database error during login. Ensure database migrations are applied ' +
                    '(especially jwt_tokens and user_roles related migrations), then restart the backend.'
                );
            }
            throw new Error(`Authentication failed: ${response.status} - ${JSON.stringify(response.data)}`);
        }

        this.token = response.data.token;
        this.client.defaults.headers.common['Authorization'] = `Bearer ${this.token}`;
    }

    /**
     * Ensure admin exists and authenticate.
     * Use this as a prerequisite flow before running authenticated tests.
     */
    async bootstrapAuth(): Promise<void> {
        await this.ensureApiReachable();
        await this.ensureAdminExists();
        await this.authenticate();
    }

    /**
     * Validate API base URL before auth flow to catch wrong targets early.
     */
    async ensureApiReachable(): Promise<void> {
        const response = await this.client.get('/health');
        if (response.status === 200) {
            return;
        }

        if (response.status >= 300 && response.status < 400) {
            const location = (response.headers as Record<string, string | undefined>)?.location;
            throw new Error(
                `Health check redirected: ${response.status} from ${this.config.baseUrl}/health` +
                `${location ? ` -> ${location}` : ''}. ` +
                'This usually means API_BASE_URL points to a frontend/proxy instead of the backend API.'
            );
        }

        throw new Error(
            `API health check failed: ${response.status} - ${JSON.stringify(response.data)} ` +
            `(base URL: ${this.config.baseUrl})`
        );
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
        } else if (response.status >= 300 && response.status < 400) {
            const location = (response.headers as Record<string, string | undefined>)?.location;
            throw new Error(
                `Create-admin request redirected: ${response.status} from ${this.config.baseUrl}/admins` +
                `${location ? ` -> ${location}` : ''}. ` +
                'This usually means API_BASE_URL is incorrect for backend API calls.'
            );
        } else if (response.status === 401 || response.status === 403) {
            // Some backend configurations protect /admins once setup is complete.
            const statusResponse = await this.client.get<AuthStatusResponse>('/auth/status');
            if (statusResponse.status === 200 && statusResponse.data?.adminConfigured) {
                console.log('ℹ️ Admin already configured; skipping create-admin bootstrap');
                return;
            }
            throw new Error(
                `Failed to create admin user: ${response.status} - ${JSON.stringify(response.data)}`
            );
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
        return this.withAuthRetry(url, () => this.client.get<T>(url, { params }));
    }

    /**
     * POST request
     */
    async post<T = any>(url: string, data?: unknown): Promise<AxiosResponse<T>> {
        return this.withAuthRetry(url, () => this.client.post<T>(url, data));
    }

    /**
     * PATCH request
     */
    async patch<T = any>(url: string, data?: unknown): Promise<AxiosResponse<T>> {
        return this.withAuthRetry(url, () => this.client.patch<T>(url, data));
    }

    /**
     * PUT request
     */
    async put<T = any>(url: string, data?: unknown): Promise<AxiosResponse<T>> {
        return this.withAuthRetry(url, () => this.client.put<T>(url, data));
    }

    /**
     * DELETE request
     */
    async delete<T = any>(url: string): Promise<AxiosResponse<T>> {
        return this.withAuthRetry(url, () => this.client.delete<T>(url));
    }

    /**
     * Make a request without authentication (for testing auth failures)
     */
    async getUnauthenticated<T = any>(url: string): Promise<AxiosResponse<T>> {
        return axios.get<T>(`${this.config.baseUrl}${url}`, {
            headers: { 'Content-Type': 'application/json' },
            maxRedirects: 0,
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
            maxRedirects: 0,
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
        await sharedClient.bootstrapAuth();
    } else if (!sharedClient.isAuthenticated()) {
        await sharedClient.bootstrapAuth();
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
