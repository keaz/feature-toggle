CREATE TABLE feature_environment_states (
                                            feature_id UUID REFERENCES features(id) ON DELETE CASCADE,
                                            environment_id UUID REFERENCES environments(id) ON DELETE CASCADE,
                                            enabled BOOLEAN DEFAULT FALSE,
                                            PRIMARY KEY (feature_id, environment_id)
);
