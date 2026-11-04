-- A new password/activation after offline recovery never revives old grants.
-- Preserve the deny-all ceiling from previous recoveries with a valid public
-- InheritanceSpec. selected requires a nonempty rule/group selection.
UPDATE inheritance_grants SET mode='all_snapshot',revision=revision+1,updated_at=now()
  WHERE mode='selected' AND maximum_rules='[]'::jsonb AND issuer_credential_id IS NULL
    AND EXISTS(SELECT 1 FROM platform_recovery_state);
CREATE TABLE recovered_account_activations (
  account_id uuid PRIMARY KEY REFERENCES accounts(id),
  operation_id uuid NOT NULL UNIQUE REFERENCES operations(id),
  recovery_id uuid NOT NULL,
  credential_id uuid NOT NULL REFERENCES credentials(id),
  subject_id uuid NOT NULL REFERENCES accounts(id),
  peer inet NOT NULL,
  authority jsonb NOT NULL CHECK(jsonb_typeof(authority)='array' AND octet_length(authority::text)<=1048576),
  token_digest bytea NOT NULL CHECK(octet_length(token_digest)=32),
  expires_at timestamptz NOT NULL,
  state text NOT NULL CHECK(state IN ('pending','used','revoked')),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
