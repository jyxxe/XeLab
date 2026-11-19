-- Unlike HTTP idempotency records, this link survives expiry and offline recovery.
-- A Call is admitted at most once for a fenced attempt, independently of the
-- step's stable business key, which a later authorized attempt may reuse.
CREATE TABLE task_attempt_calls (
  attempt_id uuid PRIMARY KEY REFERENCES attempts(id),
  call_id uuid NOT NULL UNIQUE REFERENCES calls(id),
  -- The deployment's installation can change after an upgrade.
  installation_id uuid NOT NULL REFERENCES installations(id)
);
