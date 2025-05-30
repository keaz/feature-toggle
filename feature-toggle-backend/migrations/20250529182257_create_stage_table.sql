CREATE TABLE pipeline_stages (
                                 id UUID PRIMARY KEY,
                                 pipeline_id UUID NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
                                 environment_id UUID NOT NULL REFERENCES environments(id),
                                 order_index INT NOT NULL, -- for linear flow
                                 parent_stage_id UUID REFERENCES pipeline_stages(id), -- for DAG/forking
                                 UNIQUE(pipeline_id, environment_id)
);