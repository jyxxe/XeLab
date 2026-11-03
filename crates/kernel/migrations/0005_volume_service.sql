-- Project identities are allocated before filesystem work and never recycled.
CREATE SEQUENCE volume_project_ids AS integer MINVALUE 1 MAXVALUE 2147483647 NO CYCLE;
CREATE TABLE volume_storage_root (
  id uuid PRIMARY KEY,
  singleton boolean NOT NULL DEFAULT true UNIQUE CHECK(singleton),
  filesystem_uuid uuid NOT NULL,
  state text NOT NULL CHECK(state IN ('preparing','ready')),
  capacity_bytes bigint NOT NULL DEFAULT 0 CHECK(capacity_bytes>=0),
  reserved_bytes bigint NOT NULL DEFAULT 0 CHECK(reserved_bytes>=0 AND reserved_bytes<=capacity_bytes),
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK(state<>'ready' OR capacity_bytes>=1048576)
);
CREATE TABLE volume_allocations (
  volume_id uuid PRIMARY KEY,
  root_id uuid NOT NULL REFERENCES volume_storage_root(id),
  project_id integer NOT NULL UNIQUE CHECK(project_id>0),
  owner_id uuid NOT NULL REFERENCES accounts(id),
  spec jsonb NOT NULL,
  capacity_bytes bigint NOT NULL CHECK(capacity_bytes>=1048576),
  inode_limit bigint NOT NULL CHECK(inode_limit>=1),
  metadata_charge bigint NOT NULL CHECK(metadata_charge>=65536),
  desired_capacity bigint NOT NULL CHECK(desired_capacity>=capacity_bytes),
  desired_inodes bigint NOT NULL CHECK(desired_inodes>=inode_limit),
  desired_metadata_charge bigint NOT NULL CHECK(desired_metadata_charge>=metadata_charge),
  state text NOT NULL CHECK(state IN ('creating','ready','resizing','deleting','deleted','quarantined')),
  operation_id uuid NOT NULL REFERENCES operations(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  peer inet NOT NULL,
  data_schema_version text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX volume_allocations_work_idx ON volume_allocations(created_at,volume_id) WHERE state IN ('creating','resizing','deleting','quarantined');
CREATE VIEW kernel_volumes AS
SELECT v.*, coalesce((SELECT jsonb_agg(a.instance_id ORDER BY a.instance_id) FROM volume_attachments a WHERE a.volume_id=v.id),'[]'::jsonb) AS attached_instance_ids FROM volumes v;
CREATE TABLE volume_holds (
  volume_id uuid PRIMARY KEY REFERENCES volumes(id),
  operation_id uuid NOT NULL REFERENCES operations(id),
  kind text NOT NULL CHECK(kind IN ('backup','restore','migration')),
  created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE volume_allocations ADD COLUMN observed_at timestamptz NOT NULL DEFAULT now();
CREATE INDEX volume_observation_idx ON volume_allocations(observed_at,volume_id) WHERE state='ready';
ALTER TABLE volume_attachments ADD COLUMN generation bigint NOT NULL DEFAULT 0;
ALTER TABLE volume_attachments ADD COLUMN mount_path text NOT NULL DEFAULT '';
CREATE TABLE volume_mount_approvals (
  deployment_id uuid PRIMARY KEY REFERENCES deployments(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  peer inet NOT NULL,
  generation bigint NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
