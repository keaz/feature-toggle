CREATE TABLE contextual_type
(
    id          UUID PRIMARY KEY,
    feature_id  UUID         NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    name        VARCHAR(100) NOT NULL UNIQUE,
    description TEXT NOT NULL,
    UNIQUE (feature_id, name)
);

CREATE TABLE contextual_entries
(
    id            UUID PRIMARY KEY,
    contextual_id UUID NOT NULL REFERENCES contextual_type (id) ON DELETE CASCADE,
    value         TEXT NOT NULL UNIQUE CHECK (value <> ''),
    UNIQUE (contextual_id, value)
);