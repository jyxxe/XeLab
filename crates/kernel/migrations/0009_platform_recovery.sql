-- This gate survives process death and blocks every current daemon until the
-- imported metadata, authority and filesystem have all been reconciled.
CREATE TABLE platform_recovery_state (
  singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
  recovery_id uuid NOT NULL,
  backup_id uuid NOT NULL,
  phase text NOT NULL CHECK(phase IN ('importing','rebuilding','ready')),
  recovery_owner_id uuid REFERENCES accounts(id),
  metadata_summary jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
-- Fresh host recovery is an explicit authority source, not a revived historical
-- grant or a forged account/grant creation timestamp. There is no HTTP writer.
CREATE TABLE recovery_owner_anchors (
  grant_id uuid PRIMARY KEY REFERENCES grants(id),
  account_id uuid NOT NULL REFERENCES accounts(id),
  recovery_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX recovery_owner_account_idx ON recovery_owner_anchors(account_id);
