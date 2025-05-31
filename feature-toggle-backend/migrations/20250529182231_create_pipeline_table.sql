CREATE TABLE pipelines
(
    id     UUID PRIMARY KEY,
    name   TEXT    NOT NULL UNIQUE,
    active BOOLEAN NOT NULL
);