CREATE TABLE feature_dependencies
(
    feature_id    UUID REFERENCES features (id) ON DELETE CASCADE,
    depends_on_id UUID REFERENCES features (id) ON DELETE CASCADE,
    PRIMARY KEY (feature_id, depends_on_id),
    CHECK (feature_id <> depends_on_id)
);