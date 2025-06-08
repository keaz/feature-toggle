CREATE TABLE teams
(
    id          UUID PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL
);

CREATE TABLE environments
(
    id      UUID PRIMARY KEY,
    name    VARCHAR NOT NULL,
    active  BOOLEAN NOT NULL,
    team_id UUID    REFERENCES teams (id) ON DELETE SET NULL,
    UNIQUE (name, team_id)
);

CREATE TABLE pipelines
(
    id     UUID PRIMARY KEY,
    name   TEXT    NOT NULL UNIQUE,
    active  BOOLEAN NOT NULL,
    team_id UUID REFERENCES teams (id) ON DELETE SET NULL,
    UNIQUE (name, team_id)
);

CREATE TABLE pipeline_stages
(
    id              UUID PRIMARY KEY,
    pipeline_id     UUID NOT NULL REFERENCES pipelines (id) ON DELETE CASCADE,
    environment_id  UUID NOT NULL REFERENCES environments (id),
    order_index     INT  NOT NULL,                        -- for linear flow
    parent_stage_id UUID REFERENCES pipeline_stages (id), -- for DAG/forking
    team_id         UUID REFERENCES teams (id) ON DELETE SET NULL,
    UNIQUE (pipeline_id, environment_id)
);


CREATE TABLE features
(
    id           UUID PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    description  TEXT,
    feature_type TEXT NOT NULL CHECK (feature_type IN ('Simple', 'Contextual')),
    pipeline_id  UUID REFERENCES pipelines (id) ON DELETE SET NULL,
    team_id      UUID REFERENCES teams (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ DEFAULT now(),
    UNIQUE (name, team_id)
);

CREATE TABLE feature_environment_states
(
    feature_id     UUID REFERENCES features (id) ON DELETE CASCADE,
    environment_id UUID REFERENCES environments (id) ON DELETE CASCADE,
    enabled        BOOLEAN DEFAULT FALSE,
    PRIMARY KEY (feature_id, environment_id)
);

CREATE TABLE feature_dependencies
(
    feature_id    UUID REFERENCES features (id) ON DELETE CASCADE,
    depends_on_id UUID REFERENCES features (id) ON DELETE CASCADE,
    PRIMARY KEY (feature_id, depends_on_id),
    CHECK (feature_id <> depends_on_id)
);