CREATE INDEX schedules_due_idx ON schedules(next_fire_at,id)
  WHERE admission IS NOT NULL AND spec->'enabled'='true'::jsonb;
CREATE INDEX schedule_fires_pending_idx ON schedule_fires(schedule_id,scheduled_at DESC)
  WHERE outcome='pending' AND task_id IS NULL;
CREATE INDEX schedule_fires_task_idx ON schedule_fires(schedule_id,task_id)
  WHERE task_id IS NOT NULL;
CREATE INDEX schedule_fires_task_origin_idx ON schedule_fires(task_id,schedule_id)
  WHERE task_id IS NOT NULL;
CREATE INDEX calls_execution_delegation_idx ON calls(delegation_id)
  WHERE delegation_id IS NOT NULL;
CREATE INDEX tasks_execution_delegation_idx ON tasks(delegation_id);
