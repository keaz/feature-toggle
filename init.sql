-- Drop and recreate the constraints to ensure they're correct
ALTER TABLE IF EXISTS public.features_pipeline_stages DROP CONSTRAINT IF EXISTS features_pipeline_stages_feature_id_fkey;
ALTER TABLE IF EXISTS public.features_pipeline_stages ADD CONSTRAINT features_pipeline_stages_feature_id_fkey FOREIGN KEY (feature_id) REFERENCES public.features(id) ON DELETE CASCADE;

-- Update the parent_stage_id constraint to reference features_pipeline_stages instead of pipeline_stages
ALTER TABLE IF EXISTS public.features_pipeline_stages DROP CONSTRAINT IF EXISTS features_pipeline_stages_parent_stage_id_fkey;
ALTER TABLE IF EXISTS public.features_pipeline_stages ADD CONSTRAINT features_pipeline_stages_parent_stage_id_fkey FOREIGN KEY (parent_stage_id) REFERENCES public.features_pipeline_stages(id) ON DELETE CASCADE;

-- Delete data in the correct order to avoid foreign key constraint violations
DELETE
FROM public.feature_dependencies;
DELETE
FROM public.feature_stage_criteria;
DELETE
FROM public.features_pipeline_stages;
DELETE
FROM public.features;
-- clients and origins
DELETE
FROM public.client_web_origins;
DELETE
FROM public.clients;
DELETE
FROM public.pipeline_stages;
DELETE
FROM public.pipelines;
DELETE
FROM public.environments;
DELETE
FROM public.teams;

INSERT INTO public.teams(id, name, description)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Team', 'This is a test team'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'Update Team', 'This is a test team'),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', 'Delete Team', 'This is a delete team')
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.environments(id, name, active, team_id)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Environment', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', 'To Delete Environment', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'For Update Environment', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('06f28625-df1d-499f-a4ee-5629a8b6a169', 'For Stage 1 Environment', true,
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', 'For Stage 2 Environment', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27')
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.pipelines(id, name, active, team_id)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', 'To Delete Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'For Update Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('4eef17bc-9e06-411d-b5f4-7a786e68bb96', 'Existing Pipeline', false, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb97', 'For Delete Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27')
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.pipeline_stages(id, pipeline_id, environment_id, parent_stage_id, order_index, position)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', NULL, 0, '{ "x": 250, "y": 250 }'),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        '06f28625-df1d-499f-a4ee-5629a8b6a169', NULL, 0, '{ "x": 250, "y": 250 }'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', NULL, 0, '{ "x": 250, "y": 250 }'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb97', '4eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', NULL, 0, '{ "x": 250, "y": 250 }'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb99', '3eef17bc-9e06-411d-b5f4-7a786e68bb97',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', NULL, 0, '{ "x": 250, "y": 250 }')
ON CONFLICT (id) DO NOTHING;

DELETE
FROM public.features;

INSERT INTO public.features(id, key, description, feature_type, team_id, created_at)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Feature', 'This is a test feature', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'Update Feature', 'This is a feature to update', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb97', 'Delete Feature', 'This is a feature to delete', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('4eef17bc-9e06-411d-b5f4-7a786e68bb98', 'Existing Feature', 'This is an existing feature', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('5eef17bc-9e06-411d-b5f4-7a786e68bb99', 'Test Contextual Feature', 'This is a contextual feature', 'Contextual', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('6eef17bc-9e06-411d-b5f4-7a786e68bb91', 'Dependency Feature', 'This is a dependency feature', 'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('6eef17bc-9e06-411d-b5f4-7a786e68bb81', 'Another feature', 'This is a dependency feature', 'Simple',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now())
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.features_pipeline_stages(id, feature_id, environment_id, parent_stage_id, order_index, position, enabled)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', NULL, 0, '{ "x": 250, "y": 250 }', true),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', NULL, 0, '{ "x": 250, "y": 250 }', true),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', '5eef17bc-9e06-411d-b5f4-7a786e68bb99',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', NULL, 0, '{ "x": 250, "y": 250 }', true),
       ('6eef17bc-9e06-411d-b5f4-7a786e68bb81', '6eef17bc-9e06-411d-b5f4-7a786e68bb81',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', NULL, 0, '{ "x": 250, "y": 250 }', true)
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.feature_dependencies(feature_id, depends_on_id)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', '6eef17bc-9e06-411d-b5f4-7a786e68bb91'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', '6eef17bc-9e06-411d-b5f4-7a786e68bb91')
ON CONFLICT (feature_id, depends_on_id) DO NOTHING;

-- Seed clients
INSERT INTO public.clients(id, team_id, name, description, enabled, client_type, api_key)
VALUES ('a1b2c3d4-0000-4000-8000-000000000001', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Web Client 1',
        'Seed web client', true, 'Web', 'TEST_WEB_KEY_1'),
       ('a1b2c3d4-0000-4000-8000-000000000002', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Backend Client 1',
        'Seed backend client', true, 'Backend', 'TEST_BACKEND_KEY_1')
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.client_web_origins(id, client_id, origin)
VALUES ('b1b2c3d4-0000-4000-8000-000000000001', 'a1b2c3d4-0000-4000-8000-000000000001', 'http://localhost:5173'),
       ('b1b2c3d4-0000-4000-8000-000000000002', 'a1b2c3d4-0000-4000-8000-000000000001', 'https://example.com')
ON CONFLICT (id) DO NOTHING;

-- Optionally set bucketing_key for a known stage
UPDATE public.features_pipeline_stages
SET bucketing_key = 'userId'
WHERE id = '3eef17bc-9e06-411d-b5f4-7a786e68bb96';

-- Seed contexts for tests (appended by automation)
-- Ensure contexts tables are clean and then insert deterministic data
DELETE
FROM public.context_entries;
DELETE
FROM public.contexts;

-- Insert two contexts for the known test team with predictable keys
INSERT INTO public.contexts(id, team_id, key)
VALUES ('cb461425-373b-49d9-9634-9a248612d7b7', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'filter-alpha'),
       ('fcc0dfca-07b0-44ad-8d9a-21f2cd450d10', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'filter-beta')
ON CONFLICT (id) DO NOTHING;

-- Entries for each context
INSERT INTO public.context_entries(id, context_id, value)
VALUES ('bbdb4e6e-0ac9-4a1e-b83b-78ba663f3d6f', 'cb461425-373b-49d9-9634-9a248612d7b7', 'X'),
       ('093dadfa-8452-4631-a9dd-fa7eb090cdad', 'fcc0dfca-07b0-44ad-8d9a-21f2cd450d10', 'Y'),
       ('535575bc-3dbe-4fde-a974-5673ab727149', 'fcc0dfca-07b0-44ad-8d9a-21f2cd450d10', 'Z')
ON CONFLICT (id) DO NOTHING;

-- Seed feature stage criteria for tests
-- Link to an existing features_pipeline_stages row and existing contexts
INSERT INTO public.feature_stage_criteria(id, stage_id, context_key, context_id, rollout_percentage)
VALUES (
    '11111111-1111-4111-8111-111111111111',
    '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
    'filter',
    'cb461425-373b-49d9-9634-9a248612d7b7',
    50
), (
    '22222222-2222-4222-8222-222222222222',
    '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
    'filter',
    'fcc0dfca-07b0-44ad-8d9a-21f2cd450d10',
    30
)
ON CONFLICT (id) DO NOTHING;
