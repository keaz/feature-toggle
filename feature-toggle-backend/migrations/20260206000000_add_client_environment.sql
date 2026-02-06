-- Add environment_id to clients and backfill from team environments

ALTER TABLE clients
    ADD COLUMN IF NOT EXISTS environment_id UUID;

-- Prefer active environments per team
UPDATE clients c
SET environment_id = e.id
FROM (
    SELECT DISTINCT ON (team_id) id, team_id
    FROM environments
    WHERE active = true
    ORDER BY team_id, id
) e
WHERE c.environment_id IS NULL
  AND c.team_id = e.team_id;

-- Fallback to any environment per team
UPDATE clients c
SET environment_id = e.id
FROM (
    SELECT DISTINCT ON (team_id) id, team_id
    FROM environments
    ORDER BY team_id, id
) e
WHERE c.environment_id IS NULL
  AND c.team_id = e.team_id;

-- If a team has no environments at all, create a default one and assign it
WITH missing_teams AS (
    SELECT DISTINCT c.team_id
    FROM clients c
    LEFT JOIN environments e ON e.team_id = c.team_id
    WHERE c.environment_id IS NULL
      AND e.id IS NULL
),
inserted AS (
    INSERT INTO environments (id, name, active, team_id, environment_type)
    SELECT gen_random_uuid(), 'Default', true, team_id, 'Development'
    FROM missing_teams
    RETURNING id, team_id
)
UPDATE clients c
SET environment_id = i.id
FROM inserted i
WHERE c.environment_id IS NULL
  AND c.team_id = i.team_id;

-- Final safety pass in case any nulls remain
UPDATE clients c
SET environment_id = e.id
FROM (
    SELECT DISTINCT ON (team_id) id, team_id
    FROM environments
    ORDER BY team_id, id
) e
WHERE c.environment_id IS NULL
  AND c.team_id = e.team_id;

ALTER TABLE clients
    ALTER COLUMN environment_id SET NOT NULL,
    ADD CONSTRAINT clients_environment_id_fkey
        FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE RESTRICT;

CREATE INDEX idx_clients_environment_id ON clients(environment_id);
