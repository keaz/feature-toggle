-- Introduce a canonical UUID environment reference for analytics while
-- keeping the legacy text environment_id column for backward-compatible payloads.

ALTER TABLE feature_evaluations
    ADD COLUMN IF NOT EXISTS environment_ref_id UUID;

CREATE OR REPLACE FUNCTION sync_feature_evaluations_environment_ref_id()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.environment_ref_id := NULL;

    IF NEW.environment_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$' THEN
        NEW.environment_ref_id := NEW.environment_id::uuid;
    ELSIF NEW.client_id IS NOT NULL THEN
        SELECT c.environment_id
          INTO NEW.environment_ref_id
          FROM clients c
         WHERE c.id = NEW.client_id;
    END IF;

    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_feature_evaluations_environment_ref_id
    ON feature_evaluations;

CREATE TRIGGER trg_feature_evaluations_environment_ref_id
BEFORE INSERT OR UPDATE OF environment_id, client_id
ON feature_evaluations
FOR EACH ROW
EXECUTE FUNCTION sync_feature_evaluations_environment_ref_id();

UPDATE feature_evaluations fe
   SET environment_ref_id = CASE
       WHEN fe.environment_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
           THEN fe.environment_id::uuid
       ELSE c.environment_id
   END
  FROM clients c
 WHERE fe.client_id = c.id
   AND fe.environment_ref_id IS NULL;

UPDATE feature_evaluations fe
   SET environment_ref_id = fe.environment_id::uuid
 WHERE fe.environment_ref_id IS NULL
   AND fe.environment_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$';

CREATE INDEX IF NOT EXISTS idx_feature_evaluations_environment_ref_id
    ON feature_evaluations(environment_ref_id);

CREATE INDEX IF NOT EXISTS idx_feature_evaluations_feature_env_ref_time
    ON feature_evaluations(feature_key, environment_ref_id, evaluated_at DESC);

ALTER TABLE feature_evaluations
    DROP CONSTRAINT IF EXISTS feature_evaluations_environment_ref_id_fkey;

ALTER TABLE feature_evaluations
    ADD CONSTRAINT feature_evaluations_environment_ref_id_fkey
    FOREIGN KEY (environment_ref_id) REFERENCES environments(id) ON DELETE SET NULL;
