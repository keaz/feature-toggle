#!/usr/bin/env node

/**
 * FluxGate Test Data Population Script
 *
 * This script populates the FluxGate database with comprehensive test data using GraphQL API ONLY.
 * NO SQL scripts are used - everything is done via GraphQL mutations and queries.
 *
 * Authentication Flow:
 * 1. Create admin user using createAdmin mutation (is_admin=true, no token required)
 * 2. Login as admin to get JWT token
 * 3. Use JWT token for ALL subsequent GraphQL mutations and queries
 *
 * Data Created:
 * - 6 users (1 admin with is_admin=true + 5 regular users with assigned roles)
 * - 4 teams
 * - 6 contexts with various entries
 * - 6 clients (Web and Backend types)
 * - 16 environments (team-specific: E-Commerce=5, Analytics=4, Platform=4, Mobile=3)
 * - 5 pipelines with branching structure and JSON positions {"x": 100, "y": 100}
 *   - 2 pipelines with branches (Analytics and E-Commerce APAC)
 * - 40 features (mix of Simple and Contextual with stage criteria)
 *
 * For Contextual features, the script also creates:
 * - feature_stage_contexts (linking contexts to feature stages)
 * - feature_stage_criteria (rollout percentages per context)
 *
 * Usage:
 *   node populate_test_data.js
 *
 * Requirements:
 *   - Node.js 18+ (for native fetch support)
 *   - FluxGate backend running at http://localhost:8080/graphql
 */

const GRAPHQL_ENDPOINT = 'http://localhost:8080/graphql';
const DATABASE_URL = 'postgres://postgres:root123@localhost:5432/feature_toggle';

// Test data password (hashed with argon2)
const DEFAULT_PASSWORD = 'password123';
const ADMIN_USERNAME = process.env.ADMIN_USERNAME || 'admin';
const ADMIN_PASSWORD = process.env.ADMIN_PASSWORD || DEFAULT_PASSWORD;

const DEPLOY_APPROVAL_POLICY_NAME = 'Test Data Deploy Policy';

const ROLE_IDS = {
  approver: '00000000-0000-0000-0000-000000000001',
  requester: '00000000-0000-0000-0000-000000000002',
  teamAdmin: '00000000-0000-0000-0000-000000000003'
};

// Color codes for console output
const colors = {
  reset: '\x1b[0m',
  bright: '\x1b[1m',
  green: '\x1b[32m',
  blue: '\x1b[34m',
  yellow: '\x1b[33m',
  red: '\x1b[31m',
  cyan: '\x1b[36m'
};

// Helper function to make GraphQL requests
async function graphqlRequest(query, variables = {}, token = null) {
  const headers = {
    'Content-Type': 'application/json',
  };

  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const response = await fetch(GRAPHQL_ENDPOINT, {
    method: 'POST',
    headers,
    body: JSON.stringify({ query, variables }),
  });

  const result = await response.json();

  if (result.errors) {
    console.error(`${colors.red}GraphQL Error:${colors.reset}`, JSON.stringify(result.errors, null, 2));
    throw new Error(`GraphQL Error: ${result.errors[0]?.message || 'Unknown error'}`);
  }

  return result.data;
}

// Helper function to log progress
function logStep(message, isSubstep = false) {
  const prefix = isSubstep ? '  →' : '▶';
  console.log(`${colors.cyan}${prefix} ${message}${colors.reset}`);
}

function logSuccess(message) {
  console.log(`${colors.green}✓ ${message}${colors.reset}`);
}

function logError(message) {
  console.error(`${colors.red}✗ ${message}${colors.reset}`);
}

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

function getJwtRoles(token) {
  try {
    const payload = token.split('.')[1];
    if (!payload) {
      return [];
    }
    const normalized = payload.replace(/-/g, '+').replace(/_/g, '/');
    const json = JSON.parse(Buffer.from(normalized, 'base64').toString('utf8'));
    return Array.isArray(json.roles) ? json.roles : [];
  } catch (error) {
    return [];
  }
}

// Helper queries for existing data (supports resume/idempotent runs)
async function getUserByUsername(token, username) {
  const userByUsernameQuery = `
    query UserByUsername($username: String!) {
      userByUsername(username: $username) {
        id
        username
        email
      }
    }
  `;

  try {
    const data = await graphqlRequest(userByUsernameQuery, { username }, token);
    if (data.userByUsername) {
      return data.userByUsername;
    }
  } catch (error) {
    // Fall back to search if direct lookup is unavailable
  }

  const searchQuery = `
    query Users($name: String!, $pageNumber: Int!, $pageSize: Int!) {
      users(name: $name, pageNumber: $pageNumber, pageSize: $pageSize) {
        items {
          id
          username
          email
        }
      }
    }
  `;

  const data = await graphqlRequest(searchQuery, { name: username, pageNumber: 1, pageSize: 50 }, token);
  return data.users.items.find(user => user.username === username) || null;
}

async function ensureUserRoles(token, username, roleIds) {
  const user = await getUserByUsername(token, username);
  if (!user) {
    logError(`User ${username} not found; cannot assign roles`);
    return;
  }

  const mutation = `
    mutation AssignUserRoles($userId: ID!, $input: AssignUserRolesInput!) {
      assignUserRoles(userId: $userId, input: $input) {
        id
        name
      }
    }
  `;

  try {
    await graphqlRequest(mutation, { userId: user.id, input: { roleIds } }, token);
    logSuccess(`Ensured roles for ${username}`);
  } catch (error) {
    logError(`Failed to assign roles to ${username}: ${error.message}`);
  }
}

async function getTeams(token) {
  const query = `
    query Teams {
      teams {
        id
        name
        description
      }
    }
  `;

  const data = await graphqlRequest(query, {}, token);
  return data.teams || [];
}

async function getContextsByTeam(token, teamId) {
  const query = `
    query Contexts($teamId: ID!) {
      contexts(teamId: $teamId) {
        items {
          id
          key
          entries {
            id
            value
          }
        }
      }
    }
  `;

  const data = await graphqlRequest(query, { teamId }, token);
  return data.contexts.items || [];
}

async function getClientsByTeam(token, teamId) {
  const query = `
    query Clients($teamId: ID!) {
      clients(teamId: $teamId) {
        items {
          id
          name
          enabled
          clientType
        }
      }
    }
  `;

  const data = await graphqlRequest(query, { teamId }, token);
  return data.clients.items || [];
}

async function getEnvironmentsByTeam(token, teamId) {
  const query = `
    query Environments($teamId: ID!) {
      environments(teamId: $teamId) {
        items {
          id
          name
          active
          environmentType
        }
      }
    }
  `;

  const data = await graphqlRequest(query, { teamId }, token);
  return data.environments.items || [];
}

async function getPipelinesByTeam(token, teamId) {
  const query = `
    query Pipelines($teamId: ID!) {
      pipelines(teamId: $teamId) {
        items {
          id
          name
          active
        }
      }
    }
  `;

  const data = await graphqlRequest(query, { teamId }, token);
  return data.pipelines.items || [];
}

async function getFeaturesByTeam(token, teamId) {
  const query = `
    query Features($teamId: ID!) {
      features(teamId: $teamId) {
        items {
          id
          key
        }
      }
    }
  `;

  const data = await graphqlRequest(query, { teamId }, token);
  return data.features.items || [];
}

async function getApprovalPolicies(token, teamId) {
  const query = `
    query ApprovalPolicies($teamId: ID!) {
      approvalPolicies(teamId: $teamId) {
        id
        name
        appliesTo
        environmentIds
        enabled
        autoApproveAfterHours
      }
    }
  `;

  const data = await graphqlRequest(query, { teamId }, token);
  return data.approvalPolicies || [];
}

async function getFeatureWithStages(token, featureId) {
  const query = `
    query FeatureWithStages($id: ID!) {
      feature(id: $id) {
        id
        key
        stages {
          id
          status
          environment {
            id
            name
          }
        }
      }
    }
  `;

  const data = await graphqlRequest(query, { id: featureId }, token);
  if (!data || !data.feature) {
    throw new Error(`Feature not found or inaccessible: ${featureId}`);
  }
  return data.feature;
}

async function requestStageChange(token, stageId, request) {
  const mutation = `
    mutation RequestStageChange($stageId: ID!, $request: StageChangeRequest!) {
      requestStageChange(stageId: $stageId, request: $request) {
        id
        key
        pendingApprovalRequestId
      }
    }
  `;

  const data = await graphqlRequest(mutation, { stageId, request }, token);
  return data.requestStageChange;
}

async function approveChangeRequest(token, requestId) {
  const mutation = `
    mutation ApproveChangeRequest($requestId: ID!) {
      approveChangeRequest(requestId: $requestId) {
        id
        status
      }
    }
  `;

  const data = await graphqlRequest(mutation, { requestId }, token);
  return data.approveChangeRequest;
}

async function getPendingApprovalRequest(token, teamId, featureId, environmentId) {
  const query = `
    query ApprovalRequests($teamId: ID!, $statuses: [ApprovalRequestStatus!]) {
      approvalRequests(teamId: $teamId, statuses: $statuses) {
        items {
          id
          featureId
          environmentId
          status
        }
      }
    }
  `;

  const data = await graphqlRequest(query, { teamId, statuses: ['Pending'] }, token);
  return data.approvalRequests.items.find(request => (
    request.featureId === featureId && request.environmentId === environmentId
  )) || null;
}

// 1. Create initial admin (if needed) - is_admin will be set to true by the backend
async function createInitialAdmin() {
  logStep('Creating initial admin account...');

  const mutation = `
    mutation CreateAdmin($input: RegisterUserInput!) {
      createAdmin(input: $input) {
        id
        username
        email
        isAdmin
      }
    }
  `;

  const input = {
    username: ADMIN_USERNAME,
    password: ADMIN_PASSWORD,
    firstName: 'Admin',
    lastName: 'User',
    email: 'admin@fluxgate.io'
  };

  try {
    const data = await graphqlRequest(mutation, { input });
    logSuccess(`Created initial admin: ${data.createAdmin.username} (is_admin: ${data.createAdmin.isAdmin})`);
    return data.createAdmin;
  } catch (error) {
    // Admin might already exist, that's okay
    logStep('Admin account already exists or error occurred, continuing...', true);
    return null;
  }
}

// 2. Register users (requires JWT token from admin login)
async function registerUsers(token) {
  logStep('Registering additional users...');

  const users = [
    {
      username: 'alice.smith',
      password: DEFAULT_PASSWORD,
      firstName: 'Alice',
      lastName: 'Smith',
      email: 'alice.smith@fluxgate.io',
      isAdmin: false
    },
    {
      username: 'bob.johnson',
      password: DEFAULT_PASSWORD,
      firstName: 'Bob',
      lastName: 'Johnson',
      email: 'bob.johnson@fluxgate.io',
      isAdmin: false
    },
    {
      username: 'carol.williams',
      password: DEFAULT_PASSWORD,
      firstName: 'Carol',
      lastName: 'Williams',
      email: 'carol.williams@fluxgate.io',
      isAdmin: false
    },
    {
      username: 'david.brown',
      password: DEFAULT_PASSWORD,
      firstName: 'David',
      lastName: 'Brown',
      email: 'david.brown@fluxgate.io',
      isAdmin: false
    },
    {
      username: 'emma.davis',
      password: DEFAULT_PASSWORD,
      firstName: 'Emma',
      lastName: 'Davis',
      email: 'emma.davis@fluxgate.io',
      isAdmin: false
    }
  ];

  const mutation = `
    mutation RegisterUser($input: RegisterUserInput!) {
      registerUser(input: $input) {
        id
        username
        email
      }
    }
  `;

  const createdUsers = [];

  for (const user of users) {
    try {
      const existingUser = await getUserByUsername(token, user.username);
      if (existingUser) {
        createdUsers.push({
          ...user,
          id: existingUser.id
        });
        logSuccess(`Using existing user: ${user.username}`);
        continue;
      }

      const input = {
        username: user.username,
        password: user.password,
        firstName: user.firstName,
        lastName: user.lastName,
        email: user.email,
        isAdmin: user.isAdmin
      };

      // Use token for authenticated request
      const data = await graphqlRequest(mutation, { input }, token);
      createdUsers.push({
        ...user,
        id: data.registerUser.id
      });
      logSuccess(`Registered user: ${user.username}`);
    } catch (error) {
      logError(`Failed to register user ${user.username}: ${error.message}`);
    }
  }

  return createdUsers;
}

// 3. Assign roles to users
async function assignUserRoles(token, users) {
  logStep('Assigning roles to users...');

  const mutation = `
    mutation AssignUserRoles($userId: ID!, $input: AssignUserRolesInput!) {
      assignUserRoles(userId: $userId, input: $input) {
        id
        name
        description
      }
    }
  `;

  // Assign roles to different users
  const assignments = [
    {
      username: 'alice.smith',
      roleIds: [ROLE_IDS.requester, ROLE_IDS.approver]
    },
    {
      username: 'bob.johnson',
      roleIds: [ROLE_IDS.requester]
    },
    {
      username: 'carol.williams',
      roleIds: [ROLE_IDS.approver]
    },
    {
      username: 'david.brown',
      roleIds: [ROLE_IDS.teamAdmin, ROLE_IDS.requester]
    },
    {
      username: 'emma.davis',
      roleIds: [ROLE_IDS.approver, ROLE_IDS.requester]
    }
  ];

  for (const assignment of assignments) {
    try {
      const user = users.find(u => u.username === assignment.username);
      if (!user) {
        logStep(`User ${assignment.username} not found, skipping role assignment`, true);
        continue;
      }

      const input = {
        roleIds: assignment.roleIds
      };

      const data = await graphqlRequest(mutation, { userId: user.id, input }, token);
      logSuccess(`Assigned ${data.assignUserRoles.length} role(s) to ${assignment.username}`);
    } catch (error) {
      logError(`Failed to assign roles to ${assignment.username}: ${error.message}`);
    }
  }
}

// 4. Login as admin user to get JWT token
async function loginAsAdmin() {
  logStep('Logging in as admin...');

  const mutation = `
    mutation Login($input: LoginInput!) {
      login(input: $input) {
        token
        user {
          id
          username
        }
      }
    }
  `;

  const input = {
    username: ADMIN_USERNAME,
    password: ADMIN_PASSWORD
  };

  const data = await graphqlRequest(mutation, { input });

  if (!data || !data.login) {
    throw new Error(`Login failed. Response: ${JSON.stringify(data)}`);
  }

  logSuccess(`Logged in as: ${data.login.user.username}`);
  return data.login.token;
}

async function loginAsAdminWithRetry(attempts = 3, delayMs = 1200) {
  let lastError = null;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      if (attempt > 1) {
        await sleep(delayMs);
      }
      return await loginAsAdmin();
    } catch (error) {
      lastError = error;
      const message = error.message || '';
      if (!message.includes('Database error occurred') || attempt === attempts) {
        throw error;
      }
    }
  }
  throw lastError;
}

// 5. Create contexts
async function createContexts(token, teams) {
  logStep('Creating contexts...');

  const contexts = [
    {
      teamIndex: 0, // E-Commerce Team
      key: 'userTier',
      entries: ['free', 'basic', 'premium', 'enterprise']
    },
    {
      teamIndex: 0,
      key: 'region',
      entries: ['us-east', 'us-west', 'eu', 'apac']
    },
    {
      teamIndex: 1, // Analytics Team
      key: 'userRole',
      entries: ['viewer', 'analyst', 'admin']
    },
    {
      teamIndex: 2, // Platform Team
      key: 'environment',
      entries: ['development', 'staging', 'production']
    },
    {
      teamIndex: 3, // Mobile Team
      key: 'deviceType',
      entries: ['phone', 'tablet', 'desktop']
    },
    {
      teamIndex: 3,
      key: 'osVersion',
      entries: ['ios14', 'ios15', 'ios16', 'android11', 'android12', 'android13']
    }
  ];

  const mutation = `
    mutation CreateContext($teamId: ID!, $input: CreateContextInput!) {
      createContext(teamId: $teamId, input: $input) {
        id
        key
        entries {
          id
          value
        }
      }
    }
  `;

  const createdContexts = [];
  const contextsByTeam = new Map();

  for (const context of contexts) {
    try {
      const teamId = teams[context.teamIndex].actualId;
      if (!contextsByTeam.has(teamId)) {
        const existingContexts = await getContextsByTeam(token, teamId);
        contextsByTeam.set(teamId, existingContexts);
      }

      const existingContext = contextsByTeam
        .get(teamId)
        .find(existing => existing.key === context.key);
      if (existingContext) {
        createdContexts.push({
          ...context,
          id: existingContext.id,
          entries: existingContext.entries
        });
        logSuccess(`Using existing context: ${context.key}`);
        continue;
      }

      const input = {
        key: context.key,
        entries: context.entries
      };

      const data = await graphqlRequest(mutation, { teamId, input }, token);
      createdContexts.push({
        ...context,
        id: data.createContext.id,
        entries: data.createContext.entries
      });
      logSuccess(`Created context: ${context.key} with ${context.entries.length} entries`);
    } catch (error) {
      logError(`Failed to create context ${context.key}: ${error.message}`);
    }
  }

  return createdContexts;
}

// 6. Create clients
async function createClients(token, teams) {
  logStep('Creating clients...');

  const clients = [
    {
      teamIndex: 0, // E-Commerce Team
      name: 'E-Commerce Web App',
      description: 'Main e-commerce web application',
      enabled: true,
      clientType: 'Web',
      webOrigins: ['http://localhost:3000', 'https://ecommerce.example.com']
    },
    {
      teamIndex: 0,
      name: 'E-Commerce Mobile Backend',
      description: 'Backend service for mobile app',
      enabled: true,
      clientType: 'Backend',
      webOrigins: null
    },
    {
      teamIndex: 1, // Analytics Team
      name: 'Analytics Dashboard',
      description: 'Analytics and reporting dashboard',
      enabled: true,
      clientType: 'Web',
      webOrigins: ['http://localhost:4000', 'https://analytics.example.com']
    },
    {
      teamIndex: 2, // Platform Team
      name: 'Platform Admin Console',
      description: 'Platform administration console',
      enabled: true,
      clientType: 'Web',
      webOrigins: ['http://localhost:5000', 'https://admin.example.com']
    },
    {
      teamIndex: 3, // Mobile Team
      name: 'Mobile App iOS',
      description: 'iOS mobile application backend',
      enabled: true,
      clientType: 'Backend',
      webOrigins: null
    },
    {
      teamIndex: 3,
      name: 'Mobile App Android',
      description: 'Android mobile application backend',
      enabled: true,
      clientType: 'Backend',
      webOrigins: null
    }
  ];

  const mutation = `
    mutation CreateClient($teamId: ID!, $input: CreateClientInput!) {
      createClient(teamId: $teamId, input: $input) {
        id
        name
        description
        enabled
        clientType
      }
    }
  `;

  const createdClients = [];
  const clientsByTeam = new Map();

  for (const client of clients) {
    try {
      const teamId = teams[client.teamIndex].actualId;
      if (!clientsByTeam.has(teamId)) {
        const existingClients = await getClientsByTeam(token, teamId);
        clientsByTeam.set(teamId, existingClients);
      }

      const existingClient = clientsByTeam
        .get(teamId)
        .find(existing => existing.name === client.name);
      if (existingClient) {
        createdClients.push({
          ...client,
          id: existingClient.id
        });
        logSuccess(`Using existing client: ${client.name}`);
        continue;
      }

      const input = {
        name: client.name,
        description: client.description,
        enabled: client.enabled,
        clientType: client.clientType.toUpperCase(),
        webOrigins: client.webOrigins
      };

      const data = await graphqlRequest(mutation, { teamId, input }, token);
      createdClients.push({
        ...client,
        id: data.createClient.id
      });
      logSuccess(`Created client: ${client.name} (${client.clientType})`);
    } catch (error) {
      logError(`Failed to create client ${client.name}: ${error.message}`);
    }
  }

  return createdClients;
}

// 7. Create teams
async function createTeams(token) {
  logStep('Creating teams...');

  const teams = [
    {
      id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
      name: 'E-Commerce Team',
      description: 'Team managing e-commerce features'
    },
    {
      id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
      name: 'Analytics Team',
      description: 'Team managing analytics and reporting features'
    },
    {
      id: 'cccccccc-cccc-4ccc-8ccc-cccccccccccc',
      name: 'Platform Team',
      description: 'Team managing core platform features'
    },
    {
      id: 'dddddddd-dddd-4ddd-8ddd-dddddddddddd',
      name: 'Mobile Team',
      description: 'Team managing mobile application features'
    }
  ];

  const mutation = `
    mutation CreateTeam($input: CreateTeamInput!) {
      createTeam(input: $input) {
        id
        name
        description
      }
    }
  `;

  const createdTeams = [];
  const existingTeams = await getTeams(token);

  for (const team of teams) {
    try {
      const existingTeam = existingTeams.find(existing => existing.name === team.name);
      if (existingTeam) {
        createdTeams.push({
          ...team,
          actualId: existingTeam.id
        });
        logSuccess(`Using existing team: ${team.name} (ID: ${existingTeam.id})`);
        continue;
      }

      const input = {
        name: team.name,
        description: team.description
      };

      const data = await graphqlRequest(mutation, { input }, token);
      createdTeams.push({
        ...team,
        actualId: data.createTeam.id
      });
      logSuccess(`Created team: ${team.name} (ID: ${data.createTeam.id})`);
    } catch (error) {
      logError(`Failed to create team ${team.name}: ${error.message}`);
    }
  }

  return createdTeams;
}

// 8. Create environments (each team gets its own set of environments)
async function createEnvironments(token, teams) {
  logStep('Creating environments...');

  // Define environments for each team
  const teamEnvironments = [
    {
      teamIndex: 0, // E-Commerce Team
      environments: [
        { name: 'E-Commerce-Dev', active: true },
        { name: 'E-Commerce-QA', active: true },
        { name: 'E-Commerce-Staging', active: true },
        { name: 'E-Commerce-Prod-US-East', active: true },
        { name: 'E-Commerce-Prod-APAC', active: true }
      ]
    },
    {
      teamIndex: 1, // Analytics Team
      environments: [
        { name: 'Analytics-Dev', active: true },
        { name: 'Analytics-SIT', active: true },
        { name: 'Analytics-Prod-US-East', active: true },
        { name: 'Analytics-Prod-EU', active: true }
      ]
    },
    {
      teamIndex: 2, // Platform Team
      environments: [
        { name: 'Platform-Dev', active: true },
        { name: 'Platform-QA', active: true },
        { name: 'Platform-UAT', active: true },
        { name: 'Platform-Prod-Global', active: true }
      ]
    },
    {
      teamIndex: 3, // Mobile Team
      environments: [
        { name: 'Mobile-Dev', active: true },
        { name: 'Mobile-Staging', active: true },
        { name: 'Mobile-Prod-US-West', active: true }
      ]
    }
  ];

  const mutation = `
    mutation CreateEnvironment($teamId: ID!, $input: CreateEnvironmentInput!) {
      createEnvironment(teamId: $teamId, input: $input) {
        id
        name
        active
        teamId
        environmentType
      }
    }
  `;

  const createdEnvironments = [];
  const environmentsByTeam = new Map();

  for (const teamEnv of teamEnvironments) {
    const teamId = teams[teamEnv.teamIndex].actualId;
    if (!environmentsByTeam.has(teamId)) {
      const existingEnvironments = await getEnvironmentsByTeam(token, teamId);
      environmentsByTeam.set(teamId, existingEnvironments);
    }
    const existingEnvMap = new Map(
      environmentsByTeam.get(teamId).map(env => [env.name, env])
    );

    for (const env of teamEnv.environments) {
      try {
        const existingEnv = existingEnvMap.get(env.name);
        if (existingEnv) {
          createdEnvironments.push({
            ...env,
            id: existingEnv.id,
            teamIndex: teamEnv.teamIndex,
            teamId: teamId,
            environmentType: existingEnv.environmentType
          });
          logSuccess(`Using existing environment: ${env.name} (ID: ${existingEnv.id})`);
          continue;
        }

        const input = {
          name: env.name,
          active: env.active
        };

        const data = await graphqlRequest(mutation, { teamId, input }, token);
        createdEnvironments.push({
          ...env,
          id: data.createEnvironment.id,
          teamIndex: teamEnv.teamIndex,
          teamId: teamId,
          environmentType: data.createEnvironment.environmentType
        });
        logSuccess(`Created environment: ${env.name} (ID: ${data.createEnvironment.id})`);
      } catch (error) {
        logError(`Failed to create environment ${env.name}: ${error.message}`);
      }
    }
  }

  return createdEnvironments;
}

// 9. Create pipelines with proper branching and team-specific environments
async function createPipelines(token, teams, environments) {
  logStep('Creating pipelines...');

  // Create different pipeline structures for different teams
  // Position format: { "x": 250, "y": 250 } - x increases horizontally, y increases vertically
  // orderIndex must be unique per stage within the pipeline (API validation enforces uniqueness)
  const pipelines = [
    {
      name: 'E-Commerce Standard Pipeline',
      teamIndex: 0,
      stages: [
        { envName: 'E-Commerce-Dev', orderIndex: 0, position: { x: 100, y: 200 } },
        { envName: 'E-Commerce-QA', orderIndex: 1, position: { x: 300, y: 200 } },
        { envName: 'E-Commerce-Staging', orderIndex: 2, position: { x: 500, y: 200 } },
        { envName: 'E-Commerce-Prod-US-East', orderIndex: 3, position: { x: 700, y: 200 } }
      ],
      relationships: [
        { sourceId: 0, targetId: 1 },  // Dev -> QA
        { sourceId: 1, targetId: 2 },  // QA -> Staging
        { sourceId: 2, targetId: 3 }   // Staging -> Prod-US-East
      ]
    },
    {
      name: 'Analytics Multi-Region Pipeline (with branches)',
      teamIndex: 1,
      stages: [
        { envName: 'Analytics-Dev', orderIndex: 0, position: { x: 100, y: 200 } },
        { envName: 'Analytics-SIT', orderIndex: 1, position: { x: 300, y: 200 } },
        { envName: 'Analytics-Prod-US-East', orderIndex: 2, position: { x: 500, y: 100 } },  // Branch 0
        { envName: 'Analytics-Prod-EU', orderIndex: 3, position: { x: 500, y: 300 } }        // Branch 1
      ],
      relationships: [
        { sourceId: 0, targetId: 1 },  // Dev -> SIT
        { sourceId: 1, targetId: 2 },  // SIT -> Prod-US-East (branch 0)
        { sourceId: 1, targetId: 3 }   // SIT -> Prod-EU (branch 1)
      ]
    },
    {
      name: 'Platform Global Deployment Pipeline',
      teamIndex: 2,
      stages: [
        { envName: 'Platform-Dev', orderIndex: 0, position: { x: 100, y: 200 } },
        { envName: 'Platform-QA', orderIndex: 1, position: { x: 300, y: 200 } },
        { envName: 'Platform-UAT', orderIndex: 2, position: { x: 500, y: 200 } },
        { envName: 'Platform-Prod-Global', orderIndex: 3, position: { x: 700, y: 200 } }
      ],
      relationships: [
        { sourceId: 0, targetId: 1 },  // Dev -> QA
        { sourceId: 1, targetId: 2 },  // QA -> UAT
        { sourceId: 2, targetId: 3 }   // UAT -> Prod-Global
      ]
    },
    {
      name: 'Mobile Fast Track Pipeline',
      teamIndex: 3,
      stages: [
        { envName: 'Mobile-Dev', orderIndex: 0, position: { x: 100, y: 200 } },
        { envName: 'Mobile-Staging', orderIndex: 1, position: { x: 300, y: 200 } },
        { envName: 'Mobile-Prod-US-West', orderIndex: 2, position: { x: 500, y: 200 } }
      ],
      relationships: [
        { sourceId: 0, targetId: 1 },  // Dev -> Staging
        { sourceId: 1, targetId: 2 }   // Staging -> Prod-US-West
      ]
    },
    {
      name: 'E-Commerce APAC Pipeline (with branches)',
      teamIndex: 0,
      stages: [
        { envName: 'E-Commerce-Dev', orderIndex: 0, position: { x: 100, y: 200 } },
        { envName: 'E-Commerce-QA', orderIndex: 1, position: { x: 300, y: 200 } },
        { envName: 'E-Commerce-Prod-US-East', orderIndex: 2, position: { x: 500, y: 100 } },  // Branch 0
        { envName: 'E-Commerce-Prod-APAC', orderIndex: 3, position: { x: 500, y: 300 } }      // Branch 1
      ],
      relationships: [
        { sourceId: 0, targetId: 1 },  // Dev -> QA
        { sourceId: 1, targetId: 2 },  // QA -> Prod-US-East (branch 0)
        { sourceId: 1, targetId: 3 }   // QA -> Prod-APAC (branch 1)
      ]
    }
  ];

  const mutation = `
    mutation CreatePipeline($teamId: ID!, $input: CreatePipelineInput!) {
      createPipeline(teamId: $teamId, input: $input)
    }
  `;

  const createdPipelines = [];
  const pipelinesByTeam = new Map();

  for (const pipeline of pipelines) {
    const pipelineRecord = { ...pipeline, id: null };
    const teamId = teams[pipeline.teamIndex].actualId;

    try {
      if (!pipelinesByTeam.has(teamId)) {
        const existingPipelines = await getPipelinesByTeam(token, teamId);
        pipelinesByTeam.set(teamId, existingPipelines);
      }

      const existingPipeline = pipelinesByTeam
        .get(teamId)
        .find(existing => existing.name === pipeline.name);

      if (existingPipeline) {
        pipelineRecord.id = existingPipeline.id;
        createdPipelines.push(pipelineRecord);
        logSuccess(`Using existing pipeline: ${pipeline.name} (ID: ${existingPipeline.id})`);
        continue;
      }

      // Map stages to use actual environment IDs
      const stages = pipeline.stages.map(stage => {
        const env = environments.find(e => e.name === stage.envName);
        if (!env) {
          throw new Error(`Environment not found: ${stage.envName}`);
        }
        return {
          environmentId: env.id,
          orderIndex: stage.orderIndex,
          position: JSON.stringify(stage.position) // Convert position object to JSON string
        };
      });

      const input = {
        name: pipeline.name,
        stages,
        relationships: pipeline.relationships
      };

      const data = await graphqlRequest(mutation, { teamId, input }, token);
      pipelineRecord.id = data.createPipeline;
      createdPipelines.push(pipelineRecord);
      logSuccess(`Created pipeline: ${pipeline.name} (ID: ${data.createPipeline})`);
    } catch (error) {
      logError(`Failed to create pipeline ${pipeline.name}: ${error.message}`);
    } finally {
      if (!createdPipelines.includes(pipelineRecord)) {
        createdPipelines.push(pipelineRecord);
      }
    }
  }

  return createdPipelines;
}

// 10. Create features
async function createFeatures(token, teams, pipelines, environments, contexts) {
  logStep('Creating features...');

  const features = [
    // E-Commerce Team Features (Simple)
    {
      teamIndex: 0,
      key: 'NewCheckoutFlow',
      description: 'Enable new streamlined checkout process',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 0
    },
    {
      teamIndex: 0,
      key: 'PaymentMethodApplePay',
      description: 'Enable Apple Pay as payment method',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 0
    },
    {
      teamIndex: 0,
      key: 'ProductRecommendations',
      description: 'Show AI-powered product recommendations',
      featureType: 'Simple',
      enabled: false,
      pipelineIndex: 0
    },
    {
      teamIndex: 0,
      key: 'GiftWrapping',
      description: 'Allow customers to add gift wrapping',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 0
    },
    {
      teamIndex: 0,
      key: 'ExpressShipping',
      description: 'Enable express shipping option',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 4
    },

    // E-Commerce Team Features (Contextual)
    {
      teamIndex: 0,
      key: 'PremiumCheckout',
      description: 'Premium checkout experience for VIP customers',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 0,
      contextualSettings: [
        {
          envPattern: 'E-Commerce-Dev',
          contextKeys: ['userTier'],
          criteria: [
            { contextKey: 'userTier', rolloutPercentage: 100 }
          ]
        },
        {
          envPattern: 'E-Commerce-Prod',
          contextKeys: ['userTier'],
          criteria: [
            { contextKey: 'userTier', rolloutPercentage: 50 }
          ]
        }
      ]
    },
    {
      teamIndex: 0,
      key: 'BulkOrderDiscount',
      description: 'Special discount for bulk orders',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 0,
      contextualSettings: [
        {
          envPattern: 'E-Commerce-Staging',
          contextKeys: ['userTier'],
          criteria: [
            { contextKey: 'userTier', rolloutPercentage: 75 }
          ]
        }
      ]
    },
    {
      teamIndex: 0,
      key: 'RegionalPricing',
      description: 'Dynamic pricing based on region',
      featureType: 'Contextual',
      enabled: false,
      pipelineIndex: 0,
      contextualSettings: [
        {
          envPattern: 'E-Commerce-Dev',
          contextKeys: ['region'],
          criteria: [
            { contextKey: 'region', rolloutPercentage: 100 }
          ]
        }
      ]
    },
    {
      teamIndex: 0,
      key: 'LoyaltyProgram',
      description: 'Rewards program for returning customers',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 0
    },
    {
      teamIndex: 0,
      key: 'SeasonalPromotions',
      description: 'Time-based promotional campaigns',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 4
    },

    // Analytics Team Features (Simple)
    {
      teamIndex: 1,
      key: 'RealtimeDashboard',
      description: 'Real-time analytics dashboard',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'ExportToCSV',
      description: 'Export analytics data to CSV',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'AdvancedFilters',
      description: 'Advanced filtering options for reports',
      featureType: 'Simple',
      enabled: false,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'ScheduledReports',
      description: 'Automated scheduled report generation',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'CustomMetrics',
      description: 'Create custom analytics metrics',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 1
    },

    // Analytics Team Features (Contextual)
    {
      teamIndex: 1,
      key: 'TeamSpecificDashboards',
      description: 'Customized dashboards per team',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'RoleBasedReporting',
      description: 'Different reports based on user role',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'RegionalAnalytics',
      description: 'Analytics segmented by region',
      featureType: 'Contextual',
      enabled: false,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'BetaUserInsights',
      description: 'Special insights for beta users',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 1
    },
    {
      teamIndex: 1,
      key: 'PerformanceAlerts',
      description: 'Contextual performance alerts',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 1
    },

    // Platform Team Features (Simple)
    {
      teamIndex: 2,
      key: 'DarkMode',
      description: 'Enable dark mode UI theme',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'TwoFactorAuth',
      description: 'Two-factor authentication',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'APIRateLimiting',
      description: 'API rate limiting protection',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'AuditLogging',
      description: 'Comprehensive audit logging',
      featureType: 'Simple',
      enabled: false,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'HealthMonitoring',
      description: 'System health monitoring',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 2
    },

    // Platform Team Features (Contextual)
    {
      teamIndex: 2,
      key: 'AdminFeatures',
      description: 'Advanced features for admin users',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'BetaFeatureAccess',
      description: 'Early access to beta features',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'RegionalCompliance',
      description: 'Region-specific compliance features',
      featureType: 'Contextual',
      enabled: false,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'EnterpriseSSO',
      description: 'Enterprise single sign-on',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 2
    },
    {
      teamIndex: 2,
      key: 'CustomBranding',
      description: 'Custom branding for enterprise clients',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 2
    },

    // Mobile Team Features (Simple)
    {
      teamIndex: 3,
      key: 'PushNotifications',
      description: 'Push notification support',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'OfflineMode',
      description: 'Offline functionality',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'BiometricLogin',
      description: 'Biometric authentication',
      featureType: 'Simple',
      enabled: false,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'AppRating',
      description: 'In-app rating prompt',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'DeepLinking',
      description: 'Deep linking support',
      featureType: 'Simple',
      enabled: true,
      pipelineIndex: 3
    },

    // Mobile Team Features (Contextual)
    {
      teamIndex: 3,
      key: 'LocationBasedFeatures',
      description: 'Features based on user location',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'DeviceSpecificUI',
      description: 'UI optimized per device type',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'OSVersionFeatures',
      description: 'Features based on OS version',
      featureType: 'Contextual',
      enabled: false,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'NetworkAdaptiveQuality',
      description: 'Quality adjustment based on network',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 3
    },
    {
      teamIndex: 3,
      key: 'AppVersionGating',
      description: 'Features gated by app version',
      featureType: 'Contextual',
      enabled: true,
      pipelineIndex: 3
    }
  ];

  const mutation = `
    mutation CreateFeature($teamId: ID!, $input: CreateFeatureInput!) {
      createFeature(teamId: $teamId, input: $input)
    }
  `;

  const createdFeatures = [];
  const featuresByTeam = new Map();

  for (const feature of features) {
    try {
      const teamId = teams[feature.teamIndex].actualId;
      if (!featuresByTeam.has(teamId)) {
        const existingFeatures = await getFeaturesByTeam(token, teamId);
        featuresByTeam.set(teamId, existingFeatures);
      }

      const existingFeature = featuresByTeam
        .get(teamId)
        .find(existing => existing.key === feature.key);
      if (existingFeature) {
        createdFeatures.push({
          ...feature,
          id: existingFeature.id
        });
        logSuccess(`Using existing feature: ${feature.key} (ID: ${existingFeature.id})`);

        if (feature.featureType === 'Contextual' && feature.contextualSettings) {
          await addContextualSettings(token, existingFeature.id, [], feature.contextualSettings, contexts, teams[feature.teamIndex]);
        }
        continue;
      }

      const pipeline = pipelines[feature.pipelineIndex];
      if (!pipeline || !pipeline.stages) {
        logError(`Skipping feature ${feature.key}: pipeline index ${feature.pipelineIndex} not available`);
        continue;
      }

      // Build stages array based on pipeline stages
      const stages = pipeline.stages.map((stage, index) => {
        const env = environments.find(e => e.name === stage.envName);
        if (!env) {
          throw new Error(`Environment not found: ${stage.envName}`);
        }
        return {
          environmentId: env.id,
          orderIndex: stage.orderIndex,
          position: JSON.stringify(stage.position), // Convert position object to JSON string
          bucketingKey: null
        };
      });

      const input = {
        key: feature.key,
        description: feature.description,
        featureType: feature.featureType.toUpperCase(),
        enabled: feature.enabled,
        dependencies: [],
        relationships: pipeline.relationships,
        stages: stages
      };

      const data = await graphqlRequest(mutation, { teamId, input }, token);
      const featureId = data.createFeature;

      createdFeatures.push({
        ...feature,
        id: featureId
      });
      logSuccess(`Created feature: ${feature.key} (ID: ${featureId})`);

      // For Contextual features, add contexts and criteria to stages
      if (feature.featureType === 'Contextual' && feature.contextualSettings) {
        await addContextualSettings(token, featureId, stages, feature.contextualSettings, contexts, teams[feature.teamIndex]);
      }
    } catch (error) {
      logError(`Failed to create feature ${feature.key}: ${error.message}`);
    }
  }

  return createdFeatures;
}

// 11. Add contextual settings (contexts and criteria) to feature stages
async function addContextualSettings(token, featureId, stages, settings, allContexts, team) {
  try {
    // We need to get the feature stages IDs first
    const query = `
      query GetFeature($id: ID!) {
        feature(id: $id) {
          stages {
            id
            environment {
              name
            }
          }
        }
      }
    `;

    const featureData = await graphqlRequest(query, { id: featureId }, token);
    const featureStages = featureData.feature.stages;

    // Set stage contexts mutation
    const setContextsMutation = `
      mutation SetStageContexts($stageId: ID!, $contextIds: [ID!]!) {
        setStageContexts(stageId: $stageId, contextIds: $contextIds) {
          id
          key
        }
      }
    `;

    // Set stage criteria mutation
    const setCriteriaMutation = `
      mutation SetStageCriteria($stageId: ID!, $criteria: [CreateStageCriterionInput!]!) {
        setStageCriteria(stageId: $stageId, criteria: $criteria) {
          id
          priority
          ruleGroups {
            logicOperator
            conditions {
              contextKey
              operator
              value
              orderIndex
            }
          }
        }
      }
    `;

    for (const stage of featureStages) {
      // Find matching settings for this stage's environment
      const envSettings = settings.find(s => stage.environment.name.includes(s.envPattern));

      if (envSettings) {
        // Set contexts for this stage
        if (envSettings.contextKeys && envSettings.contextKeys.length > 0) {
          const contextIds = envSettings.contextKeys.map(key => {
            const context = allContexts.find(c => c.key === key);
            return context ? context.id : null;
          }).filter(id => id !== null);

          if (contextIds.length > 0) {
            await graphqlRequest(setContextsMutation, { stageId: stage.id, contextIds }, token);
            logStep(`  → Set ${contextIds.length} context(s) for stage ${stage.environment.name}`, true);
          }
        }

        // Set criteria for this stage
        if (envSettings.criteria && envSettings.criteria.length > 0) {
          const criteria = envSettings.criteria.map((c, index) => {
            const context = allContexts.find(ctx => ctx.key === c.contextKey);
            const entryValue = context?.entries?.[0]?.value;
            if (!entryValue) {
              return null;
            }
            return {
              priority: index,
              ruleGroups: [
                {
                  logicOperator: 'AND',
                  conditions: [
                    {
                      contextKey: c.contextKey,
                      operator: 'IN',
                      value: [entryValue],
                      orderIndex: 0
                    }
                  ]
                }
              ]
            };
          }).filter(c => c !== null);

          if (criteria.length > 0) {
            await graphqlRequest(setCriteriaMutation, { stageId: stage.id, criteria }, token);
            logStep(`  → Set ${criteria.length} criteria for stage ${stage.environment.name}`, true);
          }
        }
      }
    }
  } catch (error) {
    logStep(`  → Failed to add contextual settings: ${error.message}`, true);
  }
}

function policyAppliesToEnvironment(policy, environment) {
  if (!policy.enabled) {
    return false;
  }

  if (policy.autoApproveAfterHours !== null && policy.autoApproveAfterHours !== undefined) {
    return false;
  }

  const appliesTo = (policy.appliesTo || '').toLowerCase();
  if (appliesTo === 'all') {
    return true;
  }
  if (appliesTo === 'specific_environments') {
    return Array.isArray(policy.environmentIds) && policy.environmentIds.includes(environment.id);
  }
  if (appliesTo === 'production_only') {
    return (environment.environmentType || '').toLowerCase() === 'production';
  }
  return false;
}

async function ensureApprovalPolicies(token, environments) {
  logStep('Ensuring approval policies for deployments...');

  const envsByTeam = new Map();

  for (const env of environments) {
    if (!envsByTeam.has(env.teamId)) {
      envsByTeam.set(env.teamId, []);
    }
    envsByTeam.get(env.teamId).push(env);
  }

  const mutation = `
    mutation CreateApprovalPolicy($teamId: ID!, $input: CreateApprovalPolicyInput!) {
      createApprovalPolicy(teamId: $teamId, input: $input) {
        id
        name
        appliesTo
        environmentIds
        enabled
      }
    }
  `;

  for (const [teamId, teamEnvs] of envsByTeam.entries()) {
    if (teamEnvs.length === 0) {
      continue;
    }

    const policies = await getApprovalPolicies(token, teamId);
    const allCovered = teamEnvs.every(env =>
      policies.some(policy => policyAppliesToEnvironment(policy, env))
    );

    if (allCovered) {
      logSuccess(`Approval policy already covers target environments for team ${teamId}`);
      continue;
    }

    const input = {
      name: DEPLOY_APPROVAL_POLICY_NAME,
      description: 'Auto-generated policy for test data deployments',
      appliesTo: 'specific_environments',
      environmentIds: teamEnvs.map(env => env.id),
      requiredApprovers: 1,
      approverRoleIds: [ROLE_IDS.approver],
      autoApproveAfterHours: null,
      enabled: true
    };

    try {
      const data = await graphqlRequest(mutation, { teamId, input }, token);
      logSuccess(`Created approval policy: ${data.createApprovalPolicy.name}`);
    } catch (error) {
      logError(`Failed to create approval policy for team ${teamId}: ${error.message}`);
    }
  }
}

async function deployStage(token, teamId, featureId, featureKey, stage) {
  const stageLabel = `${featureKey} → ${stage.environment.name}`;

  try {
    if (stage.status === 'DEPLOYED') {
      logSuccess(`Already deployed: ${stageLabel}`);
      return;
    }

    if (stage.status === 'DEPLOYMENT_APPROVED') {
      await requestStageChange(token, stage.id, 'DEPLOYED');
      logSuccess(`Deployed: ${stageLabel}`);
      return;
    }

    let pendingRequestId = null;

    if (stage.status === 'DEPLOYMENT_REQUESTED') {
      const pending = await getPendingApprovalRequest(token, teamId, featureId, stage.environment.id);
      if (!pending) {
        logError(`No pending approval request found for ${stageLabel}`);
        return;
      }
      pendingRequestId = pending.id;
    } else if (['NOT_DEPLOYED', 'DEPLOYMENT_REJECTED', 'ROLLBACKED'].includes(stage.status)) {
      const result = await requestStageChange(token, stage.id, 'DEPLOYMENT_REQUESTED');
      pendingRequestId = result.pendingApprovalRequestId;
      if (!pendingRequestId) {
        logError(`No approval request created for ${stageLabel}. Check approval policy coverage.`);
        return;
      }
    } else {
      logError(`Skipping ${stageLabel}: unsupported status ${stage.status}`);
      return;
    }

    if (pendingRequestId) {
      await approveChangeRequest(token, pendingRequestId);
    }

    const refreshed = await getFeatureWithStages(token, featureId);
    const refreshedStage = refreshed.stages.find(s => s.id === stage.id);
    if (!refreshedStage) {
      logError(`Unable to refresh stage for ${stageLabel}`);
      return;
    }

    if (refreshedStage.status !== 'DEPLOYMENT_APPROVED') {
      logError(`Expected DEPLOYMENT_APPROVED for ${stageLabel}, got ${refreshedStage.status}`);
      return;
    }

    await requestStageChange(token, stage.id, 'DEPLOYED');
    logSuccess(`Deployed: ${stageLabel}`);
  } catch (error) {
    logError(`Failed to deploy ${stageLabel}: ${error.message}`);
  }
}

async function deployFeaturesToEnvironments(token, teams, features) {
  logStep('Deploying all features to all stages...');

  for (const feature of features) {
    try {
      if (!feature || !feature.id) {
        logError(`Skipping feature with missing ID: ${feature?.key || 'unknown'}`);
        continue;
      }

      const teamId = teams[feature.teamIndex].actualId;
      const featureData = await getFeatureWithStages(token, feature.id);
      const stagesToDeploy = featureData.stages || [];

      if (stagesToDeploy.length === 0) {
        logStep(`No stages found for ${feature.key}`, true);
        continue;
      }

      for (const stage of stagesToDeploy) {
        await deployStage(token, teamId, feature.id, feature.key, stage);
      }
    } catch (error) {
      logError(`Failed to deploy feature ${feature?.key || 'unknown'}: ${error.message}`);
    }
  }
}

// Main execution function
async function main() {
  console.log(`${colors.bright}${colors.blue}`);
  console.log('╔════════════════════════════════════════════════════════════╗');
  console.log('║     FluxGate Test Data Population Script (GraphQL)        ║');
  console.log('╚════════════════════════════════════════════════════════════╝');
  console.log(colors.reset);

  console.log(`${colors.yellow}Target: ${GRAPHQL_ENDPOINT}${colors.reset}\n`);

  try {
    // Step 1: Create initial admin (no token required - this is a special mutation)
    const admin = await createInitialAdmin();
    console.log('');

    // Step 2: Login as admin to get JWT token (required for all subsequent operations)
    const token = await loginAsAdmin();
    console.log('');

    // Step 3: Register additional users (requires token)
    const users = await registerUsers(token);
    console.log('');

    // Step 4: Assign roles to users (requires token)
    await assignUserRoles(token, users);
    console.log('');

    // Step 4.1: Ensure admin can deploy across all teams
    await ensureUserRoles(token, ADMIN_USERNAME, [ROLE_IDS.requester, ROLE_IDS.approver]);
    let deploymentToken = token;
    const roles = getJwtRoles(token);
    if (!roles.includes('Requester') || !roles.includes('Approver')) {
      try {
        deploymentToken = await loginAsAdminWithRetry();
      } catch (error) {
        logError(`Failed to refresh admin token: ${error.message}`);
        deploymentToken = null;
      }
    }
    console.log('');

    // Step 5: Create teams
    const teams = await createTeams(token);
    console.log('');

    // Step 6: Create contexts
    const contexts = await createContexts(token, teams);
    console.log('');

    // Step 7: Create clients
    const clients = await createClients(token, teams);
    console.log('');

    // Step 8: Create environments
    const environments = await createEnvironments(token, teams);
    console.log('');

    // Step 8.1: Ensure approval policies exist for target deployments
    await ensureApprovalPolicies(token, environments);
    console.log('');

    // Step 9: Create pipelines
    const pipelines = await createPipelines(token, teams, environments);
    console.log('');

    // Step 10: Create features
    const features = await createFeatures(token, teams, pipelines, environments, contexts);
    console.log('');

    // Step 11: Deploy target features to target environments
    if (deploymentToken) {
      await deployFeaturesToEnvironments(deploymentToken, teams, features);
      console.log('');
    } else {
      logError('Skipping deployments because no deployment token is available.');
      console.log('');
    }

    // Summary
    console.log(`${colors.bright}${colors.green}`);
    console.log('╔════════════════════════════════════════════════════════════╗');
    console.log('║                    ✓ COMPLETED                             ║');
    console.log('╚════════════════════════════════════════════════════════════╝');
    console.log(colors.reset);
    console.log(`${colors.green}Test data population completed successfully!${colors.reset}\n`);
    console.log(`${colors.cyan}Summary:${colors.reset}`);
    console.log(`  - Users: ${users.length + 1} (including admin)`);
    console.log(`  - Teams: ${teams.length}`);
    console.log(`  - Contexts: ${contexts.length}`);
    console.log(`  - Clients: ${clients.length}`);
    console.log(`  - Environments: ${environments.length}`);
    console.log(`  - Pipelines: ${pipelines.length}`);
    console.log(`  - Features: ${features.length}`);
    console.log('');

  } catch (error) {
    logError(`Fatal error: ${error.message}`);
    console.error(error);
    process.exit(1);
  }
}

// Run the script
main();
