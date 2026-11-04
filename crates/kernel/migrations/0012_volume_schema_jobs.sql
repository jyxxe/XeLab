-- An admitted initialization intent is not a verified business schema.
ALTER TABLE volume_allocations ADD COLUMN schema_state text NOT NULL DEFAULT 'unversioned'
  CHECK(schema_state IN ('unversioned','initializing','verified'));
ALTER TABLE volume_allocations ADD COLUMN schema_deployment_id uuid REFERENCES deployments(id);
ALTER TABLE volume_allocations ADD COLUMN schema_plugin_id text;
ALTER TABLE volume_allocations ADD COLUMN schema_publisher_id uuid REFERENCES publishers(id);
ALTER TABLE volume_allocations ADD COLUMN schema_manifest_digest text;
ALTER TABLE volume_allocations ADD COLUMN schema_operation_id uuid REFERENCES operations(id);
ALTER TABLE volume_allocations ADD COLUMN schema_proof jsonb;
ALTER TABLE volume_allocations ADD CONSTRAINT volume_schema_binding_check CHECK(
  schema_state='unversioned' OR
  (data_schema_version IS NOT NULL AND data_schema_version<>'none'
   AND length(data_schema_version) BETWEEN 1 AND 64
   AND schema_deployment_id IS NOT NULL AND schema_plugin_id IS NOT NULL
   AND schema_publisher_id IS NOT NULL AND schema_operation_id IS NOT NULL
   AND schema_manifest_digest ~ '^sha256:[0-9a-f]{64}$')
);
ALTER TABLE volume_allocations ADD CONSTRAINT volume_schema_proof_check CHECK(
  (schema_proof IS NULL OR (jsonb_typeof(schema_proof)='object' AND octet_length(schema_proof::text)<=65536))
  AND (schema_state<>'verified' OR schema_proof IS NOT NULL)
);
-- The existing API requires creation disabled, approval, then explicit enable.
-- Its operation/instance identities are retained through a supervised bootstrap.
CREATE TABLE volume_schema_jobs (
  operation_id uuid PRIMARY KEY REFERENCES operations(id),
  kind text NOT NULL CHECK(kind IN ('bootstrap','upgrade','rollback')),
  deployment_id uuid NOT NULL REFERENCES deployments(id),
  instance_id uuid NOT NULL UNIQUE REFERENCES instances(id),
  generation bigint NOT NULL CHECK(generation BETWEEN 1 AND 9007199254740991),
  installation_id uuid NOT NULL REFERENCES installations(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  subject_id uuid NOT NULL REFERENCES accounts(id),
  peer inet NOT NULL,
  authority jsonb NOT NULL CHECK(octet_length(authority::text)<=1048576),
  deployment_spec jsonb NOT NULL,
  installation_metadata jsonb NOT NULL,
  source_schema text NOT NULL CHECK(length(source_schema) BETWEEN 1 AND 64),
  target_schema text NOT NULL CHECK(length(target_schema) BETWEEN 1 AND 64 AND target_schema<>'none'),
  scratch_volume_id uuid NOT NULL UNIQUE REFERENCES volume_allocations(volume_id),
  volume_ids jsonb NOT NULL CHECK(jsonb_typeof(volume_ids)='array' AND jsonb_array_length(volume_ids) BETWEEN 1 AND 16),
  volume_plan jsonb NOT NULL CHECK(jsonb_typeof(volume_plan)='array' AND octet_length(volume_plan::text)<=65536),
  state text NOT NULL CHECK(state IN ('planning','planned','initializing','applying','verifying','verified','failed')),
  plan jsonb CHECK(plan IS NULL OR (jsonb_typeof(plan)='object' AND octet_length(plan::text)<=65536)),
  volume_fence bigint CHECK(volume_fence BETWEEN 1 AND 9007199254740991),
  backup_id uuid REFERENCES backups(id),
  failure jsonb,
  expires_at timestamptz NOT NULL DEFAULT now()+interval '1 hour',
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK(source_schema<>target_schema),
  CHECK(kind<>'bootstrap' OR source_schema='none'),
  CHECK(kind='bootstrap' OR backup_id IS NOT NULL),
  CHECK(state NOT IN ('planned','initializing','applying','verifying','verified') OR (plan IS NOT NULL AND volume_fence IS NOT NULL)),
  CHECK(state<>'applying' OR kind<>'bootstrap')
);
CREATE UNIQUE INDEX volume_schema_active_idx ON volume_schema_jobs(deployment_id)
  WHERE state NOT IN ('verified','failed');
CREATE INDEX volume_schema_work_idx ON volume_schema_jobs(updated_at,operation_id)
  WHERE state NOT IN ('verified','failed');
