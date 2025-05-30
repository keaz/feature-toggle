CREATE TABLE features (
                          id UUID PRIMARY KEY,
                          name TEXT NOT NULL UNIQUE,
                          description TEXT,
                          feature_type TEXT NOT NULL, -- 'simple' or 'contextual'
                          pipeline_id UUID REFERENCES pipelines(id),
                          created_at TIMESTAMPTZ DEFAULT now()
);
