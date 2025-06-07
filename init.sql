INSERT INTO public.teams(id, name, description)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Team', 'This is a test team'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'Update Team', 'This is a test team'),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', 'Delete Team', 'This is a delete team')
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.environments(id, name, active, team_id)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Environment', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', 'To Delete Environment', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'For Update Environment', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27')
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.pipelines(id, name, active, team_id)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', 'To Delete Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'For Update Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('4eef17bc-9e06-411d-b5f4-7a786e68bb96', 'Existing Pipeline', false, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27'),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb97', 'For Delete Pipeline', true, '51ecc366-f1cd-4d3d-ab73-fa60bad98f27')
ON CONFLICT (id) DO NOTHING;
