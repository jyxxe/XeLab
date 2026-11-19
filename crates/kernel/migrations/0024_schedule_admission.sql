-- Older schedules remain readable and stoppable without gaining a submission proof.
ALTER TABLE schedules ADD COLUMN admission jsonb CHECK (
  admission IS NULL OR (
    jsonb_typeof(admission) = 'object' AND admission @> '{"version":1,"task_admission":{"version":1}}'::jsonb
    AND jsonb_typeof(admission->'workflow_id') = 'string'
    AND admission->'workflow_id' = spec->'workflow_id'
    AND jsonb_typeof(admission->'task_create_paths') = 'array'
  ) IS TRUE
);
CREATE UNIQUE INDEX schedule_admission_delegation_idx ON schedules(delegation_id) WHERE admission IS NOT NULL;
