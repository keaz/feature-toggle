CREATE TABLE features
(
    id           UUID PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    description  TEXT,
    feature_type TEXT NOT NULL CHECK (feature_type IN ('Simple', 'Contextual')),
    pipeline_id  UUID REFERENCES pipelines (id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ DEFAULT now()
);