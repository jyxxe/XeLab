-- Upgrade holds use the same operation identity as recovery, but have their
-- own durable lifecycle. Existing recovery job identities remain valid.
ALTER TABLE deployment_storage_holds DROP CONSTRAINT deployment_storage_holds_operation_id_fkey;
ALTER TABLE deployment_storage_holds ADD CONSTRAINT deployment_storage_holds_operation_id_fkey FOREIGN KEY(operation_id) REFERENCES operations(id);
CREATE TABLE deployment_upgrade_jobs (
  operation_id uuid PRIMARY KEY REFERENCES operations(id),
  deployment_id uuid NOT NULL REFERENCES deployments(id),
  action text NOT NULL CHECK(action IN ('plugin.upgrade','plugin.rollback')),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  subject_id uuid NOT NULL REFERENCES accounts(id),
  peer inet NOT NULL,
  authority jsonb NOT NULL,
  source_installation_id uuid NOT NULL REFERENCES installations(id),
  target_installation_id uuid NOT NULL REFERENCES installations(id),
  source_metadata jsonb NOT NULL,
  target_metadata jsonb NOT NULL,
  original_spec jsonb NOT NULL,
  source_revision bigint NOT NULL CHECK(source_revision BETWEEN 1 AND 9007199254740990),
  target_spec jsonb NOT NULL,
  instances jsonb NOT NULL,
  candidate_id uuid NOT NULL UNIQUE REFERENCES instances(id),
  candidate_generation bigint NOT NULL CHECK(candidate_generation BETWEEN 1 AND 9007199254740991),
  strategy text NOT NULL CHECK(strategy IN ('recreate','parallel_compatible')),
  state text NOT NULL CHECK(state IN ('freezing','stopped','planning','planned','preparing','applying','verifying','probing','committing','succeeded','failed')),
  backup_id uuid REFERENCES backups(id),
  plan jsonb,
  volume_fence bigint CHECK(volume_fence BETWEEN 1 AND 9007199254740991),
  failure jsonb,
  expires_at timestamptz NOT NULL DEFAULT now()+interval '1 hour',
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK(source_installation_id<>target_installation_id),
  CHECK(octet_length(authority::text)<=1048576),
  CHECK(plan IS NULL OR octet_length(plan::text)<=65536)
);
CREATE UNIQUE INDEX deployment_upgrade_active_idx ON deployment_upgrade_jobs(deployment_id) WHERE state NOT IN ('succeeded','failed');
CREATE INDEX deployment_upgrade_work_idx ON deployment_upgrade_jobs(updated_at,operation_id) WHERE state NOT IN ('succeeded','failed');
