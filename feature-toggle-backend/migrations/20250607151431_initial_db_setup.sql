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
    position        VARCHAR(100) NOT NULL,                                  -- to display in the UI
    UNIQUE (pipeline_id, environment_id)
);


CREATE TABLE features
(
    id           UUID PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    description  TEXT,
    feature_type TEXT         NOT NULL CHECK (feature_type IN ('Simple', 'Contextual')),
    team_id      UUID         NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ DEFAULT now(),
    UNIQUE (name, team_id)
);


CREATE TABLE features_pipeline_stages
(
    id              UUID PRIMARY KEY,
    feature_id      UUID         NOT NULL REFERENCES pipelines (id) ON DELETE CASCADE,
    environment_id  UUID         NOT NULL REFERENCES environments (id) ON DELETE CASCADE,
    parent_stage_id UUID REFERENCES pipeline_stages (id) ON DELETE CASCADE, -- for DAG/forking
    order_index     INT          NOT NULL,
    position        VARCHAR(100) NOT NULL,
    enabled         BOOLEAN      NOT NULL DEFAULT true,
    UNIQUE (feature_id, environment_id)
);

CREATE TABLE feature_dependencies
(
    feature_id    UUID NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    depends_on_id UUID NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    PRIMARY KEY (feature_id, depends_on_id),
    CHECK (feature_id <> depends_on_id)
);