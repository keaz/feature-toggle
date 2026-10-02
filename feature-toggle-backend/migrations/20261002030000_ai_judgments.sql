-- TypeSafe AI judgments (docs/ai-judgments/design.md §4.1, §4.2).
-- A missing team_ai_settings row means every AI feature is off for the team.
CREATE TABLE team_ai_settings (
  team_id              UUID PRIMARY KEY REFERENCES teams(id) ON DELETE CASCADE,
  approval_risk        BOOLEAN NOT NULL DEFAULT FALSE,
  justification_check  BOOLEAN NOT NULL DEFAULT FALSE,
  flag_kind            BOOLEAN NOT NULL DEFAULT FALSE,
  nl_search            BOOLEAN NOT NULL DEFAULT FALSE,
  updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_by           UUID NULL
);

CREATE TABLE ai_judgments (
  id            UUID PRIMARY KEY,
  team_id       UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  subject_type  TEXT NOT NULL CHECK (subject_type IN
                  ('approval_request','feature','activity','freeze_window','scheduled_change')),
  subject_id    UUID NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('approval_risk','justification','flag_kind')),
  status        TEXT NOT NULL CHECK (status IN ('pending','done','failed')),
  attempts      INT  NOT NULL DEFAULT 0,
  input         JSONB NOT NULL,
  input_hash    TEXT NOT NULL,
  model         TEXT NULL,
  raw_answers   JSONB NULL,
  derived       JSONB NULL,
  input_tokens  INT NULL,
  error         TEXT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  completed_at  TIMESTAMPTZ NULL,
  UNIQUE (subject_type, subject_id, kind)
);

CREATE INDEX ai_judgments_retry_idx ON ai_judgments (status, created_at);
