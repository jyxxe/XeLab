-- New physical volumes stay private until the version/mount publication.
CREATE TABLE deployment_upgrade_volumes (
  operation_id uuid NOT NULL REFERENCES deployment_upgrade_jobs(operation_id),
  ordinal integer NOT NULL CHECK(ordinal BETWEEN 0 AND 15),
  source_volume_id uuid NOT NULL REFERENCES volumes(id),
  target_volume_id uuid NOT NULL UNIQUE REFERENCES volume_allocations(volume_id),
  source_spec jsonb NOT NULL,
  data_schema_version text CHECK(data_schema_version IS NULL OR length(data_schema_version) BETWEEN 1 AND 64),
  state text NOT NULL CHECK(state IN ('reserved','copying','cloned','published','failed')),
  source_fence bigint CHECK(source_fence BETWEEN 1 AND 9007199254740991),
  target_fence bigint CHECK(target_fence BETWEEN 1 AND 9007199254740991),
  proof jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY(operation_id,source_volume_id),
  UNIQUE(operation_id,ordinal),
  CHECK(source_volume_id<>target_volume_id),
  CHECK(proof IS NULL OR octet_length(proof::text)<=65536),
  CHECK(state NOT IN ('copying','cloned','published') OR (source_fence IS NOT NULL AND target_fence IS NOT NULL)),
  CHECK(state NOT IN ('cloned','published') OR proof IS NOT NULL)
);
