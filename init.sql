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
