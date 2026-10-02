-- Flag kind (docs/ai-judgments/design.md §4.4). Does not affect evaluation.
ALTER TABLE features
  ADD COLUMN flag_kind TEXT NULL
    CHECK (flag_kind IN ('release','experiment','ops','permission','config')),
  ADD COLUMN flag_kind_source TEXT NULL CHECK (flag_kind_source IN ('ai','user')),
  ADD COLUMN flag_kind_confidence REAL NULL;
