-- Manual decisions link to the fenced attempt without rewriting its receipt.
CREATE TABLE task_step_resolutions (
  attempt_id uuid PRIMARY KEY REFERENCES attempts(id),
  resolution text NOT NULL CHECK (resolution IN ('confirmed_succeeded','confirmed_not_executed')),
  evidence text NOT NULL CHECK (char_length(evidence) BETWEEN 1 AND 2048 AND btrim(evidence)<>''),
  subject_id uuid NOT NULL REFERENCES accounts(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  credential_chain uuid[] NOT NULL
    CHECK (array_position(credential_chain, credential_id) IS NOT NULL
       AND array_position(credential_chain, NULL::uuid) IS NULL),
  request_id uuid NOT NULL UNIQUE,
  created_at timestamptz NOT NULL
);

-- A cancellation of an unknown task may have changed no rows. Its scoped
-- audit record still prevents a later resolution from resuming the workflow.
CREATE INDEX audit_unknown_task_cancel_idx ON audit_events(resource)
  WHERE action='cancelTask' AND outcome='succeeded' AND reason_code='COMMITTED';
