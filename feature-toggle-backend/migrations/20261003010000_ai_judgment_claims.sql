-- Retry sweep bookkeeping for ai_judgments.
-- `attempts` now counts only runs that reached the TypeSafe API. `claims`
-- counts how often the retry sweep picked the row up (bounded, so a row whose
-- runs never get an API permit cannot loop forever), and `claimed_at` spaces
-- those claims so a run in progress is not picked up again by another node.
ALTER TABLE ai_judgments
    ADD COLUMN claims INT NOT NULL DEFAULT 0,
    ADD COLUMN claimed_at TIMESTAMPTZ NULL;
