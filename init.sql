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
FROM public.features_pipeline_stages;
DELETE
FROM public.features;
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
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', NULL, 0, 0),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', '1ab6ca79-a4fc-44ba-87e2-12884edf17f7',
        '06f28625-df1d-499f-a4ee-5629a8b6a169', NULL, 0, 0),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', NULL, 0, 0)
ON CONFLICT (id) DO NOTHING;

DELETE
FROM public.features;

INSERT INTO public.features(id, name, description, feature_type, team_id, created_at)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Feature', 'This is a test feature', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'Update Feature', 'This is a feature to update', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb97', 'Delete Feature', 'This is a feature to delete', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('4eef17bc-9e06-411d-b5f4-7a786e68bb98', 'Existing Feature', 'This is an existing feature', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('5eef17bc-9e06-411d-b5f4-7a786e68bb99', 'Test Contextual Feature', 'This is a contextual feature', 'Contextual', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now()),
       ('6eef17bc-9e06-411d-b5f4-7a786e68bb91', 'Dependency Feature', 'This is a dependency feature', 'Simple', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', now())
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.features_pipeline_stages(id, feature_id, environment_id, parent_stage_id, order_index, position, enabled)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', '51ecc366-f1cd-4d3d-ab73-fa60bad98f27',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', NULL, 0, '{ "x": 250, "y": 250 }', true),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', '3eef17bc-9e06-411d-b5f4-7a786e68bb96',
        '51ecc366-f1cd-4d3d-ab73-fa60bad98f27', NULL, 0, '{ "x": 250, "y": 250 }', true),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', '5eef17bc-9e06-411d-b5f4-7a786e68bb99',
        '78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017', NULL, 0, '{ "x": 250, "y": 250 }', true)
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.feature_dependencies(feature_id, depends_on_id)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', '6eef17bc-9e06-411d-b5f4-7a786e68bb91'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', '6eef17bc-9e06-411d-b5f4-7a786e68bb91')
ON CONFLICT (feature_id, depends_on_id) DO NOTHING;
