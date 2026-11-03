-- No S3 side effect precedes its durable identity. Holds survive daemon death.
CREATE TABLE backup_repositories (
  id uuid PRIMARY KEY,
  singleton boolean NOT NULL DEFAULT true UNIQUE CHECK(singleton),
  location text NOT NULL,
  region text NOT NULL,
  state text NOT NULL CHECK(state IN ('preparing','ready')),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE storage_recovery_jobs (
  operation_id uuid PRIMARY KEY REFERENCES operations(id),
  kind text NOT NULL CHECK(kind IN ('backup','restore')),
  backup_id uuid NOT NULL REFERENCES backups(id),
  deployment_id uuid NOT NULL REFERENCES deployments(id),
  repository_id uuid NOT NULL REFERENCES backup_repositories(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  subject_id uuid NOT NULL REFERENCES accounts(id),
  peer inet NOT NULL,
  authority jsonb NOT NULL,
  state text NOT NULL CHECK(state IN ('freezing','stopped','transferring','committing','succeeded','failed')),
  original_spec jsonb NOT NULL,
  installation_digest text NOT NULL,
  installation_metadata jsonb NOT NULL,
  instances jsonb NOT NULL,
  manifest jsonb,
  manifest_object_id uuid NOT NULL,
  master_key_id text NOT NULL,
  metadata_point pg_lsn,
  stopped_at timestamptz,
  failure jsonb,
  expires_at timestamptz NOT NULL DEFAULT now()+interval '1 hour',
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX storage_recovery_work_idx ON storage_recovery_jobs(updated_at,operation_id) WHERE state NOT IN ('succeeded','failed');
CREATE TABLE deployment_storage_holds (
  deployment_id uuid PRIMARY KEY REFERENCES deployments(id),
  operation_id uuid NOT NULL UNIQUE REFERENCES storage_recovery_jobs(operation_id),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE backup_job_volumes (
  operation_id uuid NOT NULL REFERENCES storage_recovery_jobs(operation_id),
  source_volume_id uuid NOT NULL REFERENCES volumes(id),
  object_id uuid NOT NULL,
  upload_id text,
  state text NOT NULL CHECK(state IN ('pending','uploading','prepared','verified','failed')),
  volume_spec jsonb NOT NULL,
  capacity_bytes bigint NOT NULL CHECK(capacity_bytes>=1048576),
  data_schema_version text,
  transfer_summary jsonb,
  part_etags jsonb,
  restored_volume_id uuid,
  cleanup_pending boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY(operation_id,source_volume_id),
  UNIQUE(operation_id,object_id)
);
