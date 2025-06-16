CREATE TABLE teams
(
    id          UUID PRIMARY KEY,
    name        VARCHAR(100) NOT NULL UNIQUE,
    description TEXT         NOT NULL
);

CREATE TABLE environments
(
    id      UUID PRIMARY KEY,
    name    VARCHAR(100) NOT NULL,
    active  BOOLEAN      NOT NULL,
    team_id UUID         NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    UNIQUE (name, team_id)
);

CREATE TABLE pipelines
(
    id      UUID PRIMARY KEY,
    name    VARCHAR(100) NOT NULL UNIQUE,
    active  BOOLEAN NOT NULL,
    team_id UUID         NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    UNIQUE (name, team_id)
);

CREATE TABLE pipeline_stages
(
    id              UUID PRIMARY KEY,
    pipeline_id     UUID         NOT NULL REFERENCES pipelines (id) ON DELETE CASCADE,
    environment_id  UUID         NOT NULL REFERENCES environments (id) ON DELETE CASCADE,
    parent_stage_id UUID REFERENCES pipeline_stages (id) ON DELETE CASCADE, -- for DAG/forking
    order_index     INT          NOT NULL,
    team_id         UUID         NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    position        VARCHAR(100) NOT NULL,                                  -- to display in the UI
    UNIQUE (team_id, pipeline_id, environment_id)
);


CREATE TABLE features
(
    id           UUID PRIMARY KEY,
    name         VARCHAR(100) NOT NULL UNIQUE,
    description  TEXT,
    feature_type TEXT         NOT NULL CHECK (feature_type IN ('Simple', 'Contextual')),
    pipeline_id  UUID         REFERENCES pipelines (id) ON DELETE SET NULL,
    team_id      UUID         NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ DEFAULT now(),
    UNIQUE (name, team_id)
);

CREATE TABLE feature_environment_states
(
    feature_id     UUID NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    environment_id UUID NULL REFERENCES environments (id) ON DELETE CASCADE,
    enabled        BOOLEAN DEFAULT FALSE,
    PRIMARY KEY (feature_id, environment_id)
);

CREATE TABLE feature_dependencies
(
    feature_id    UUID NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    depends_on_id UUID NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    PRIMARY KEY (feature_id, depends_on_id),
    CHECK (feature_id <> depends_on_id)
);