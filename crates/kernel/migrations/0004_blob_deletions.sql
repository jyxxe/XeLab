-- Metadata removal and the physical deletion intent commit together.
CREATE TABLE blob_deletions (
  id uuid PRIMARY KEY,
  backend_reference text NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX blobs_expiry_idx ON blobs(expires_at,id) WHERE pin_count=0;
CREATE INDEX attempts_checkpoint_blob_idx ON attempts(checkpoint_blob_id) WHERE checkpoint_blob_id IS NOT NULL;
ALTER TABLE blobs ADD CONSTRAINT blobs_pin_count_nonnegative CHECK(pin_count>=0);
-- Persist registration intent before writing the on-disk identity marker.
CREATE TABLE blob_storage_root (
  id uuid PRIMARY KEY,
  singleton boolean NOT NULL DEFAULT true UNIQUE CHECK(singleton),
  state text NOT NULL CHECK(state IN ('preparing','ready')),
  created_at timestamptz NOT NULL DEFAULT now()
);
