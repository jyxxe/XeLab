-- Preserve baseline tables and make lease authority independently revocable.
CREATE TABLE secret_binding_authority (
  binding_id uuid PRIMARY KEY REFERENCES secret_bindings(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  peer inet NOT NULL
);
CREATE INDEX secret_bindings_deployment_idx ON secret_bindings(deployment_id, secret_id);
CREATE TABLE secret_leases (
  id uuid PRIMARY KEY,
  binding_id uuid NOT NULL REFERENCES secret_bindings(id),
  instance_id uuid NOT NULL REFERENCES instances(id),
  generation bigint NOT NULL CHECK(generation>0),
  secret_version bigint NOT NULL CHECK(secret_version>0),
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL
);
