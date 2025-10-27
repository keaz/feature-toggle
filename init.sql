-- Drop and recreate the constraints to ensure they're correct
ALTER TABLE IF EXISTS public.features_pipeline_stages
DROP CONSTRAINT IF EXISTS features_pipeline_stages_feature_id_fkey;

ALTER TABLE IF EXISTS public.features_pipeline_stages
ADD CONSTRAINT features_pipeline_stages_feature_id_fkey FOREIGN KEY (feature_id) REFERENCES public.features (id) ON DELETE CASCADE;

-- Update the parent_stage_id constraint to reference features_pipeline_stages instead of pipeline_stages
ALTER TABLE IF EXISTS public.features_pipeline_stages
DROP CONSTRAINT IF EXISTS features_pipeline_stages_parent_stage_id_fkey;

ALTER TABLE IF EXISTS public.features_pipeline_stages
ADD CONSTRAINT features_pipeline_stages_parent_stage_id_fkey FOREIGN KEY (parent_stage_id) REFERENCES public.features_pipeline_stages (id) ON DELETE CASCADE;

-- Delete data in the correct order to avoid foreign key constraint violations
DELETE FROM public.feature_dependencies;

DELETE FROM public.feature_stage_criteria;

DELETE FROM public.features_pipeline_stages;

DELETE FROM public.features;
-- clients and origins
DELETE FROM public.client_web_origins;

DELETE FROM public.clients;

DELETE FROM public.pipeline_stages;

DELETE FROM public.pipelines;

DELETE FROM public.environments;

DELETE FROM public.teams;

DELETE FROM public.user_flag_assignments;

-- Users cleanup and seed for tests
DELETE FROM public.users;

-- Seed users
INSERT INTO
    public.users (
        id,
        username,
        password_hash,
        first_name,
        last_name,
        email,
        is_admin,
        created_at,
        updated_at,
        last_login
    )
VALUES (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
        'admin',
        '$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$u9gQh8QH1h0kLxF8lqE3Bw',
        'Alice',
        'Admin',
        'admin@example.com',
        false,
        now(),
        now(),
        NULL
    ),
    (
        'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        'bob',
        '$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$u9gQh8QH1h0kLxF8lqE3Bw',
        'Bob',
        'Builder',
        'bob@example.com',
        false,
        now(),
        now(),
        NULL
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO
    public.teams (id, name, description)
VALUES (
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Test Team',
        'This is a test team'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        'Update Team',
        'This is a test team'
    ),
    (
        '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        'Delete Team',
        'This is a delete team'
    )
ON CONFLICT (id) DO NOTHING;

-- Seed user-team assignments
INSERT INTO
    public.user_teams (user_id, team_id)
VALUES (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96'
    ),
    (
        'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    )
ON CONFLICT (user_id, team_id) DO NOTHING;

INSERT INTO
    public.environments (id, name, active, team_id)
VALUES (
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Test Environment',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        'To Delete Environment',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        'For Update Environment',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '06f28625-df1d-499f-a4ee-5629a8b6a169',
        'For Stage 1 Environment',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017',
        'For Stage 2 Environment',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO
    public.pipelines (id, name, active, team_id)
VALUES (
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Test Pipeline',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        'To Delete Pipeline',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        'For Update Pipeline',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '4eef17bc-9e06-411d-b5f4-7a786e68bb96',
        'Existing Pipeline',
        false,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb97',
        'For Delete Pipeline',
        true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO
    public.pipeline_stages (
        id,
        pipeline_id,
        environment_id,
        parent_stage_id,
        order_index,
        position
    )
VALUES (
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    ),
    (
        '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        '06f28625-df1d-499f-a4ee-5629a8b6a169',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb97',
        '4eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb99',
        '3eef17bc-9e06-411d-b5f4-7a786e68bb97',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    )
ON CONFLICT (id) DO NOTHING;

DELETE FROM public.features;

-- Ensure the default is set correctly (fix for inverted logic)
ALTER TABLE features ALTER COLUMN kill_switch_enabled SET DEFAULT true;

INSERT INTO
    public.features (
        id,
        key,
        description,
        feature_type,
        team_id,
        created_at
    )
VALUES (
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Test Feature',
        'This is a test feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        'Update Feature',
        'This is a feature to update',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb97',
        'Delete Feature',
        'This is a feature to delete',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '4eef17bc-9e06-411d-b5f4-7a786e68bb98',
        'Existing Feature',
        'This is an existing feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '5eef17bc-9e06-411d-b5f4-7a786e68bb99',
        'Test Contextual Feature',
        'This is a contextual feature',
        'Contextual',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '6eef17bc-9e06-411d-b5f4-7a786e68bb91',
        'Dependency Feature',
        'This is a dependency feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '6eef17bc-9e06-411d-b5f4-7a786e68bb81',
        'Another feature',
        'This is a dependency feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        'edbdab01-35ea-4fa1-ba2f-6d3a09276654',
        'Paginated 1',
        'This is a Paginated feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '1edc438b-d52b-41e2-b127-827bfed126fc',
        'Paginated 2',
        'This is a Paginated feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '40ebdf65-2fa1-480c-bb01-3c253574ccf8',
        'Paginated 3',
        'This is a Paginated feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '2f661f39-6df4-4e7c-befe-dd80c19041c8',
        'Paginated 4',
        'This is a Paginated feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    ),
    (
        '25f955ff-7e4b-4a05-ada2-f82280157649',
        'Kill switch',
        'This is a Kill switch feature',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now()
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO
    public.features_pipeline_stages (
        id,
        feature_id,
        environment_id,
        parent_stage_id,
        order_index,
        position
    )
VALUES (
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    ),
    (
        '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '5eef17bc-9e06-411d-b5f4-7a786e68bb99',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    ),
    (
        '6eef17bc-9e06-411d-b5f4-7a786e68bb81',
        '6eef17bc-9e06-411d-b5f4-7a786e68bb81',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017',
        NULL,
        0,
        '{ "x": 250, "y": 250 }'
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO
    public.feature_dependencies (feature_id, depends_on_id)
VALUES (
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '6eef17bc-9e06-411d-b5f4-7a786e68bb91'
    ),
    (
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '6eef17bc-9e06-411d-b5f4-7a786e68bb91'
    )
ON CONFLICT (feature_id, depends_on_id) DO NOTHING;

-- Seed clients
INSERT INTO
    public.clients (
        id,
        team_id,
        name,
        description,
        enabled,
        client_type,
        api_key
    )
VALUES (
        'a1b2c3d4-0000-4000-8000-000000000001',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Web Client 1',
        'Seed web client',
        true,
        'Web',
        'TEST_WEB_KEY_1'
    ),
    (
        'a1b2c3d4-0000-4000-8000-000000000002',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Backend Client 1',
        'Seed backend client',
        true,
        'Backend',
        'TEST_BACKEND_KEY_2'
    ),
    (
        'a1b2c3d4-0000-4000-8000-000000000003',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Paginated 1',
        'Seed web client',
        true,
        'Web',
        'TEST_WEB_KEY_3'
    ),
    (
        'a1b2c3d4-0000-4000-8000-000000000004',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Paginated 2',
        'Seed backend client',
        true,
        'Backend',
        'TEST_BACKEND_KEY_4'
    ),
    (
        'a1b2c3d4-0000-4000-8000-000000000005',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'Paginated 3',
        'Seed backend client',
        true,
        'Backend',
        'TEST_BACKEND_KEY_5'
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO
    public.client_web_origins (id, client_id, origin)
VALUES (
        'b1b2c3d4-0000-4000-8000-000000000001',
        'a1b2c3d4-0000-4000-8000-000000000001',
        'http://localhost:5173'
    ),
    (
        'b1b2c3d4-0000-4000-8000-000000000002',
        'a1b2c3d4-0000-4000-8000-000000000001',
        'https://example.com'
    )
ON CONFLICT (id) DO NOTHING;

-- Optionally set bucketing_key for a known stage
UPDATE public.features_pipeline_stages
SET
    bucketing_key = 'userId'
WHERE
    id = '3eef17bc-9e06-411d-b5f4-7a786e68bb96';

-- Set status to DEPLOYED for seeded stages to keep tests passing
UPDATE public.features_pipeline_stages
SET
    status = 'DEPLOYED',
    enabled = true;

-- Seed contexts for tests (appended by automation)
-- Ensure contexts tables are clean and then insert deterministic data
DELETE FROM public.context_entries;

DELETE FROM public.contexts;

-- Insert two contexts for the known test team with predictable keys
INSERT INTO
    public.contexts (id, team_id, key)
VALUES (
        'cb461425-373b-49d9-9634-9a248612d7b7',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'filter-alpha'
    ),
    (
        'fcc0dfca-07b0-44ad-8d9a-21f2cd450d10',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'filter-beta'
    )
ON CONFLICT (id) DO NOTHING;

-- Entries for each context
INSERT INTO
    public.context_entries (id, context_id, value)
VALUES (
        'bbdb4e6e-0ac9-4a1e-b83b-78ba663f3d6f',
        'cb461425-373b-49d9-9634-9a248612d7b7',
        'X'
    ),
    (
        '093dadfa-8452-4631-a9dd-fa7eb090cdad',
        'fcc0dfca-07b0-44ad-8d9a-21f2cd450d10',
        'Y'
    ),
    (
        '535575bc-3dbe-4fde-a974-5673ab727149',
        'fcc0dfca-07b0-44ad-8d9a-21f2cd450d10',
        'Z'
    )
ON CONFLICT (id) DO NOTHING;

-- Seed feature stage criteria for tests
-- Link to an existing features_pipeline_stages row and existing contexts
INSERT INTO
    public.feature_stage_criteria (
        id,
        stage_id,
        context_key,
        context_id,
        rollout_percentage
    )
VALUES (
        '11111111-1111-4111-8111-111111111111',
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        'filter',
        'cb461425-373b-49d9-9634-9a248612d7b7',
        50
    ),
    (
        '22222222-2222-4222-8222-222222222222',
        '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        'filter',
        'fcc0dfca-07b0-44ad-8d9a-21f2cd450d10',
        30
    )
ON CONFLICT (id) DO NOTHING;

-- Seed feature_evaluations for integration tests
DELETE FROM public.feature_evaluations;

INSERT INTO
    public.feature_evaluations (
        id,
        feature_key,
        environment_id,
        client_id,
        evaluated_at,
        evaluation_result,
        evaluation_context,
        user_context,
        prior_assignment,
        created_at
    )
VALUES (
        'e1111111-1111-4111-8111-111111111111',
        'test-feature-create',
        'env-123',
        'a1b2c3d4-0000-4000-8000-000000000001',
        now() - interval '1 day',
        true,
        '{
    "user": "user123"
  }',
        'user123',
        false,
        now() - interval '1 day'
    ),
    (
        'e2222222-2222-4222-8222-222222222222',
        'test-feature-create',
        'env-123',
        'a1b2c3d4-0000-4000-8000-000000000002',
        now(),
        false,
        '{
         "user": "user456"
       }',
        'user456',
        true,
        now()
    ),
    (
        'e3333333-3333-4333-8333-333333333333',
        'bulk-feature-1',
        'env-123',
        'a1b2c3d4-0000-4000-8000-000000000001',
        now(),
        true,
        NULL,
        'userA',
        false,
        now()
    ),
    (
        'e4444444-4444-4444-8444-444444444444',
        'bulk-feature-2',
        'env-123',
        'a1b2c3d4-0000-4000-8000-000000000002',
        now(),
        false,
        NULL,
        'userB',
        true,
        now()
    )
ON CONFLICT (id) DO NOTHING;

-- Kill Switch Test Data
-- Add kill switch specific test features for comprehensive testing
-- NOTE: kill_switch_enabled semantics:
--   true  = feature is ENABLED (kill switch is OFF/not activated)
--   false = feature is DISABLED (kill switch is ON/activated)
-- NOTE: rollback_scheduled_at is used to schedule a future disable
--   When the scheduler runs, it picks up features where rollback_scheduled_at <= now()
--   and then executes execute_scheduled_disable to actually disable them
INSERT INTO
    public.features (
        id,
        key,
        description,
        feature_type,
        team_id,
        created_at,
        kill_switch_enabled,
        kill_switch_activated_at,
        rollback_scheduled_at
    )
VALUES
    -- Normal feature (enabled, for testing emergency disable scheduling)
    (
        'a0000000-0000-4000-8000-000000000001',
        'normal-feature',
        'Normal feature that can have kill switch scheduled',
        'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        now(),
        true,
        NULL,
        NULL
    ),

-- Feature already disabled via kill switch (for testing emergency enable)
(
    'a0000000-0000-4000-8000-000000000002',
    'disabled-feature',
    'Feature that is currently disabled by kill switch',
    'Simple',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    now(),
    false,
    now() - interval '1 hour',
    NULL
),

-- Feature with rollback scheduled for past (should be picked up by scheduler and disabled)
(
    'a0000000-0000-4000-8000-000000000003',
    'past-rollback-feature',
    'Feature with expired rollback time - scheduler should disable this',
    'Simple',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    now(),
    true,
    NULL,
    now() - interval '30 minutes'
),

-- Feature with rollback scheduled for future (should not be picked up yet)
(
    'a0000000-0000-4000-8000-000000000004',
    'future-rollback-feature',
    'Feature with future rollback scheduled - still enabled until then',
    'Simple',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    now(),
    true,
    NULL,
    now() + interval '1 hour'
),

-- Feature disabled without rollback scheduled (immediate manual disable)
(
    'a0000000-0000-4000-8000-000000000005',
    'manual-disabled-feature',
    'Feature disabled immediately without scheduling',
    'Simple',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    now(),
    false,
    now() - interval '6 hours',
    NULL
),

-- Complex contextual feature for kill switch testing (enabled)
(
    'a0000000-0000-4000-8000-000000000006',
    'contextual-kill-switch-feature',
    'Contextual feature for kill switch testing',
    'Contextual',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    now(),
    true,
    NULL,
    NULL
),
(
    'dd663d53-0bcd-44ab-b9e2-5ac27312805e',
    'feature-to-kill',
    'Feature that get killed by the test',
    'Simple',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    now(),
    true,
    NULL,
    NULL
),
(
    '00e862c2-29b4-4fd0-9dcb-4d3f274cc5c2',
    'feature-to-kill-and-enabled',
    'Feature that get killed and enabled by the test',
    'Simple',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    now(),
    true,
    NULL,
    NULL
)
ON CONFLICT (id) DO NOTHING;

-- Add pipeline stages for kill switch test features
INSERT INTO
    public.features_pipeline_stages (
        id,
        feature_id,
        environment_id,
        parent_stage_id,
        order_index,
        position,
        status,
        enabled
    )
VALUES (
        'a1111111-1111-4111-8111-111111111111',
        'a0000000-0000-4000-8000-000000000001',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 100, "y": 100 }',
        'DEPLOYED',
        true
    ),
    (
        'a1111111-1111-4111-8111-111111111112',
        'a0000000-0000-4000-8000-000000000002',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 100, "y": 150 }',
        'DEPLOYED',
        true
    ),
    (
        'a1111111-1111-4111-8111-111111111113',
        'a0000000-0000-4000-8000-000000000003',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 100, "y": 200 }',
        'DEPLOYED',
        true
    ),
    (
        'a1111111-1111-4111-8111-111111111114',
        'a0000000-0000-4000-8000-000000000004',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 100, "y": 250 }',
        'DEPLOYED',
        true
    ),
    (
        'a1111111-1111-4111-8111-111111111115',
        'a0000000-0000-4000-8000-000000000005',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 100, "y": 300 }',
        'DEPLOYED',
        true
    ),
    (
        'a1111111-1111-4111-8111-111111111116',
        'a0000000-0000-4000-8000-000000000006',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        NULL,
        0,
        '{ "x": 100, "y": 350 }',
        'DEPLOYED',
        true
    )
ON CONFLICT (id) DO NOTHING;

-- Add feature stage criteria for contextual kill switch feature
INSERT INTO
    public.feature_stage_criteria (
        id,
        stage_id,
        context_key,
        context_id,
        rollout_percentage
    )
VALUES (
        'a2222222-2222-4222-8222-222222222221',
        'a1111111-1111-4111-8111-111111111116',
        'filter',
        'cb461425-373b-49d9-9634-9a248612d7b7',
        75
    )
ON CONFLICT (id) DO NOTHING;

-- Add evaluation records for kill switch features to test impact
INSERT INTO
    public.feature_evaluations (
        id,
        feature_key,
        environment_id,
        client_id,
        evaluated_at,
        evaluation_result,
        evaluation_context,
        user_context,
        prior_assignment,
        created_at
    )
VALUES
    -- Evaluations for normal feature (before kill switch)
    (
        'a4df5bc3-48b5-43cf-925d-ca1d3f07593a',
        'normal-feature',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'a1b2c3d4-0000-4000-8000-000000000001',
        now() - interval '2 hours',
        true,
        '{
      "user": "user1"
    }',
        'user1',
        false,
        now() - interval '2 hours'
    ),
    (
        '7009be03-2b51-4da6-83e3-4a6b805c0af4',
        'normal-feature',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        'a1b2c3d4-0000-4000-8000-000000000002',
        now() - interval '1 hour',
        true,
        '{
      "user": "user2"
    }',
        'user2',
        false,
        now() - interval '1 hour'
    ),

-- Evaluations for disabled feature (should show impact of kill switch)
(
    'f0015567-aa85-49b5-afd6-b8a78071723c',
    'disabled-feature',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    'a1b2c3d4-0000-4000-8000-000000000001',
    now() - interval '30 minutes',
    false,
    '{
      "user": "user3"
    }',
    'user3',
    true,
    now() - interval '30 minutes'
),

-- Evaluations for past rollback feature
(
    '414daddc-7bdf-4e92-83ec-a47a2b6d8f95',
    'past-rollback-feature',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    'a1b2c3d4-0000-4000-8000-000000000002',
    now() - interval '3 hours',
    true,
    '{
      "user": "user4"
    }',
    'user4',
    false,
    now() - interval '3 hours'
),
(
    '95512a8a-19d5-410c-966b-97a5a3307851',
    'past-rollback-feature',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    'a1b2c3d4-0000-4000-8000-000000000001',
    now() - interval '15 minutes',
    false,
    '{
      "user": "user5"
    }',
    'user5',
    true,
    now() - interval '15 minutes'
),

-- Evaluations for contextual kill switch feature
(
    '8ee439d0-281d-48ee-8a70-26f33d97d097',
    'contextual-kill-switch-feature',
    '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
    'a1b2c3d4-0000-4000-8000-000000000001',
    now() - interval '45 minutes',
    true,
    '{
      "user": "user6",
      "filter": "X"
    }',
    'user6',
    false,
    now() - interval '45 minutes'
)
ON CONFLICT (id) DO NOTHING;
