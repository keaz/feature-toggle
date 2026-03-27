-- Enforce ownership integrity at the database boundary.
-- The existing seed data already satisfies these relationships; this migration
-- intentionally fails future inconsistent writes instead of trying to silently
-- remap legacy rows.

-- Clients must point at an environment owned by the same team.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM pg_constraint
         WHERE conname = 'environments_team_id_id_key'
    ) THEN
        ALTER TABLE environments
            ADD CONSTRAINT environments_team_id_id_key UNIQUE (team_id, id);
    END IF;
END;
$$;

-- Repair any pre-existing drift before the foreign key is enforced.
-- This keeps the migration safe for databases that already contain a small
-- amount of legacy bad data from earlier application-level writes.
UPDATE clients c
SET environment_id = e.id
FROM (
    SELECT DISTINCT ON (team_id) id, team_id
    FROM environments
    WHERE active = true
    ORDER BY team_id, id
) e
WHERE c.team_id = e.team_id
  AND NOT EXISTS (
      SELECT 1
      FROM environments env
      WHERE env.id = c.environment_id
        AND env.team_id = c.team_id
  );

UPDATE clients c
SET environment_id = e.id
FROM (
    SELECT DISTINCT ON (team_id) id, team_id
    FROM environments
    ORDER BY team_id, id
) e
WHERE c.team_id = e.team_id
  AND NOT EXISTS (
      SELECT 1
      FROM environments env
      WHERE env.id = c.environment_id
        AND env.team_id = c.team_id
  );

WITH missing_teams AS (
    SELECT DISTINCT c.team_id
    FROM clients c
    LEFT JOIN environments e ON e.team_id = c.team_id
    WHERE e.id IS NULL
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
WHERE c.team_id = i.team_id
  AND NOT EXISTS (
      SELECT 1
      FROM environments env
      WHERE env.id = c.environment_id
        AND env.team_id = c.team_id
  );

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM pg_constraint
         WHERE conname = 'clients_team_environment_fkey'
    ) THEN
        ALTER TABLE clients
            ADD CONSTRAINT clients_team_environment_fkey
            FOREIGN KEY (team_id, environment_id)
            REFERENCES environments(team_id, id)
            ON DELETE RESTRICT;
    END IF;
END;
$$;

-- Stage-context links must stay within the owning feature team.
CREATE OR REPLACE FUNCTION validate_feature_stage_context_team_match()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    stage_team_id uuid;
    context_team_id uuid;
BEGIN
    SELECT f.team_id
      INTO stage_team_id
      FROM features_pipeline_stages fps
      JOIN features f ON f.id = fps.feature_id
     WHERE fps.id = NEW.stage_id;

    SELECT c.team_id
      INTO context_team_id
      FROM contexts c
     WHERE c.id = NEW.context_id;

    IF stage_team_id IS NULL OR context_team_id IS NULL THEN
        RAISE EXCEPTION
            'feature_stage_contexts references missing stage % or context %',
            NEW.stage_id,
            NEW.context_id
            USING ERRCODE = '23503';
    END IF;

    IF stage_team_id <> context_team_id THEN
        RAISE EXCEPTION
            'feature_stage_contexts requires stage % and context % to belong to the same team',
            NEW.stage_id,
            NEW.context_id
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_validate_feature_stage_context_team_match
    ON feature_stage_contexts;

CREATE TRIGGER trg_validate_feature_stage_context_team_match
BEFORE INSERT OR UPDATE OF stage_id, context_id
ON feature_stage_contexts
FOR EACH ROW
EXECUTE FUNCTION validate_feature_stage_context_team_match();

COMMENT ON FUNCTION validate_feature_stage_context_team_match() IS
'Rejects stage-context links that cross team ownership boundaries.';

-- Selected variant controls must exist on the feature that owns the stage.
CREATE OR REPLACE FUNCTION validate_feature_stage_criterion_variant_reference()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    criterion_feature_id uuid;
    variant_exists boolean;
BEGIN
    IF NEW.selected_variant_control IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT fps.feature_id
      INTO criterion_feature_id
      FROM features_pipeline_stages fps
     WHERE fps.id = NEW.stage_id;

    IF criterion_feature_id IS NULL THEN
        RAISE EXCEPTION
            'feature_stage_criteria references missing stage %',
            NEW.stage_id
            USING ERRCODE = '23503';
    END IF;

    SELECT EXISTS (
        SELECT 1
          FROM feature_variants fv
         WHERE fv.feature_id = criterion_feature_id
           AND fv.control = NEW.selected_variant_control
    )
    INTO variant_exists;

    IF NOT variant_exists THEN
        RAISE EXCEPTION
            'feature_stage_criteria selected variant % does not exist for feature %',
            NEW.selected_variant_control,
            criterion_feature_id
            USING ERRCODE = '23503';
    END IF;

    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_validate_feature_stage_criterion_variant_reference
    ON feature_stage_criteria;

CREATE TRIGGER trg_validate_feature_stage_criterion_variant_reference
BEFORE INSERT OR UPDATE OF stage_id, selected_variant_control
ON feature_stage_criteria
FOR EACH ROW
EXECUTE FUNCTION validate_feature_stage_criterion_variant_reference();

COMMENT ON FUNCTION validate_feature_stage_criterion_variant_reference() IS
'Rejects criteria that reference variants not owned by the feature for the owning stage.';
