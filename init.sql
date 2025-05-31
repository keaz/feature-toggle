CREATE TABLE environments
(
    id     UUID PRIMARY KEY,
    name   VARCHAR NOT NULL,
    active BOOLEAN NOT NULL
);

INSERT INTO public.environments(id, name, active)
VALUES ('51ecc366-f1cd-4d3d-ab73-fa60bad98f27', 'Test Environment', true),
       ('1ab6ca79-a4fc-44ba-87e2-12884edf17f7', 'To Delete Environment', true),
       ('3eef17bc-9e06-411d-b5f4-7a786e68bb96', 'For Update Environment', true)
ON CONFLICT (id) DO NOTHING;
