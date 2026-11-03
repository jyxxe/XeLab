-- Approval and call snapshots retain exact source alternatives, not tokens.
CREATE TABLE host_proxy_approvals (
  deployment_id uuid PRIMARY KEY REFERENCES deployments(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  peer inet NOT NULL,
  approval_digest text NOT NULL,
  source_paths jsonb NOT NULL CHECK(jsonb_typeof(source_paths)='array'),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE host_rpc_contexts (
  call_id uuid PRIMARY KEY REFERENCES calls(id),
  peer inet NOT NULL,
  source_paths jsonb NOT NULL CHECK(jsonb_typeof(source_paths)='array'),
  read_blob_ids jsonb NOT NULL CHECK(jsonb_typeof(read_blob_ids)='array'),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE host_blob_outputs (
  call_id uuid NOT NULL REFERENCES calls(id),
  idempotency_key text NOT NULL,
  blob_id uuid NOT NULL,
  fingerprint text NOT NULL,
  reference jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY(call_id,idempotency_key),
  UNIQUE(blob_id)
);
