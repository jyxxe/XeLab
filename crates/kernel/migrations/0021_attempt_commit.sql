-- Historical claims have no trustworthy worker identity to backfill.
ALTER TABLE attempts ADD COLUMN worker_id uuid;

-- Keep terminal evidence out of the public attempt projection. Each attempt
-- can accept exactly one result, independently of later changes to its step.
CREATE TABLE attempt_commit_receipts (
  attempt_id uuid PRIMARY KEY REFERENCES attempts(id),
  state text NOT NULL CHECK (state IN ('succeeded','failed','unknown')),
  output jsonb NOT NULL,
  error jsonb NOT NULL,
  CHECK ((state = 'succeeded' AND error = 'null'::jsonb)
      OR (state IN ('failed','unknown') AND output = 'null'::jsonb AND jsonb_typeof(error) IN ('null','object')))
);
