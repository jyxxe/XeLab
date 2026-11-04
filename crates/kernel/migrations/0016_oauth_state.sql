-- Appended after the real HTTP migration 15; never rewrite the schema baseline.
-- Public Client/OAuth models are unchanged. Only trusted internal bindings live here.
ALTER TABLE clients ADD COLUMN created_by uuid REFERENCES accounts(id);
ALTER TABLE authorization_requests ADD COLUMN scope text NOT NULL DEFAULT '' CHECK(octet_length(scope)<=512);
ALTER TABLE authorization_requests ADD COLUMN origin text NOT NULL DEFAULT '' CHECK(octet_length(origin)<=512);
ALTER TABLE authorization_requests ADD COLUMN client_revision bigint NOT NULL DEFAULT 1 CHECK(client_revision>0);
ALTER TABLE authorization_requests ADD COLUMN fixed_paths jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(fixed_paths)='array' AND octet_length(fixed_paths::text)<=1048576);
ALTER TABLE authorization_requests ADD COLUMN hmac_key_id text NOT NULL DEFAULT '';
CREATE INDEX authorization_requests_session_idx ON authorization_requests(session_id,status,expires_at);
CREATE TABLE oauth_families (
  id uuid PRIMARY KEY,
  subject_id uuid NOT NULL REFERENCES accounts(id),
  session_id uuid NOT NULL REFERENCES credentials(id),
  client_id text NOT NULL REFERENCES clients(client_id),
  client_revision bigint NOT NULL CHECK(client_revision>0),
  origin text NOT NULL CHECK(octet_length(origin) BETWEEN 1 AND 512),
  scope text NOT NULL CHECK(octet_length(scope) BETWEEN 1 AND 512),
  maximum_rules jsonb NOT NULL CHECK(jsonb_typeof(maximum_rules)='array'),
  fixed_paths jsonb NOT NULL CHECK(jsonb_typeof(fixed_paths)='array' AND octet_length(fixed_paths::text)<=1048576),
  expires_at timestamptz NOT NULL,
  status text NOT NULL CHECK(status IN ('active','revoked')),
  created_at timestamptz NOT NULL,
  updated_at timestamptz NOT NULL
);
CREATE INDEX oauth_families_session_idx ON oauth_families(session_id,status);
CREATE INDEX oauth_families_client_idx ON oauth_families(client_id,status);
CREATE INDEX credentials_family_idx ON credentials(family_id,status);
CREATE TABLE oauth_refresh_tokens (
  credential_id uuid PRIMARY KEY REFERENCES credentials(id),
  family_id uuid NOT NULL REFERENCES oauth_families(id),
  used_at timestamptz,
  replacement_id uuid REFERENCES credentials(id),
  created_at timestamptz NOT NULL,
  CHECK((used_at IS NULL)=(replacement_id IS NULL))
);
CREATE TABLE oauth_login_intents (
  id uuid PRIMARY KEY,
  hmac_key_id text NOT NULL,
  verification_digest bytea NOT NULL UNIQUE CHECK(octet_length(verification_digest)=32),
  client_id text NOT NULL REFERENCES clients(client_id),
  client_revision bigint NOT NULL CHECK(client_revision>0),
  request_spec jsonb NOT NULL CHECK(octet_length(request_spec::text)<=8192),
  peer inet NOT NULL,
  expires_at timestamptz NOT NULL,
  consumed_at timestamptz,
  created_at timestamptz NOT NULL
);
CREATE INDEX oauth_login_intents_expiry_idx ON oauth_login_intents(expires_at);
