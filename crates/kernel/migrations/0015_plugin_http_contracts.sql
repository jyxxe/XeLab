-- Preserve the exact signed OpenAPI bytes, independently of mutable deployment
-- state and plugin responses. Installation and call provenance survives archive,
-- uninstall and offline recovery; previously published migrations stay unchanged.
CREATE TABLE plugin_http_contracts (
  installation_id uuid PRIMARY KEY REFERENCES installations(id),
  digest text NOT NULL CHECK(digest ~ '^sha256:[a-f0-9]{64}$'),
  raw bytea NOT NULL CHECK(octet_length(raw) BETWEEN 1 AND 4194304),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION protect_plugin_http_contract() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'Verified HTTP contract provenance is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER plugin_http_contract_immutable
  BEFORE UPDATE OR DELETE ON plugin_http_contracts
  FOR EACH ROW EXECUTE FUNCTION protect_plugin_http_contract();

-- Route approval is a live authority record, unlike immutable contract bytes.
-- Re-approval replaces it using the current Authorizer and frozen source paths.
CREATE TABLE plugin_http_approvals (
  deployment_id uuid PRIMARY KEY REFERENCES deployments(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  peer inet NOT NULL,
  approval_digest text NOT NULL CHECK(approval_digest ~ '^sha256:[a-f0-9]{64}$'),
  source_paths jsonb NOT NULL CHECK(jsonb_typeof(source_paths)='array' AND octet_length(source_paths::text)<=1048576),
  created_at timestamptz NOT NULL DEFAULT now()
);

-- Static requests share the physical instance pool without inventing a user
-- business Call, account charge or daily quota. Unknown work remains reserved
-- until the instance is positively fenced by the existing lifecycle owner.
CREATE TABLE plugin_http_requests (
  id uuid PRIMARY KEY,
  instance_id uuid NOT NULL REFERENCES instances(id),
  generation bigint NOT NULL CHECK(generation>0),
  expires_at timestamptz NOT NULL,
  state text NOT NULL CHECK(state IN ('active','unknown')),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX plugin_http_requests_instance_idx ON plugin_http_requests(instance_id,generation);

ALTER TABLE calls ADD COLUMN http_binding jsonb;
ALTER TABLE calls ADD CONSTRAINT calls_http_binding_shape CHECK(
  http_binding IS NULL OR (
    jsonb_typeof(http_binding)='object'
    AND octet_length(http_binding::text)<=8192
    AND http_binding ?& ARRAY['installation_id','deployment_id','generation',
      'http_contract_digest','operation_id','method','path','required_action']
    AND (http_binding - ARRAY['installation_id','deployment_id','generation',
      'http_contract_digest','operation_id','method','path','required_action'])='{}'::jsonb
    AND jsonb_typeof(http_binding->'installation_id')='string'
    AND jsonb_typeof(http_binding->'deployment_id')='string'
    AND jsonb_typeof(http_binding->'generation')='number'
    AND http_binding->>'generation' ~ '^[1-9][0-9]{0,18}$'
    AND jsonb_typeof(http_binding->'http_contract_digest')='string'
    AND http_binding->>'http_contract_digest' ~ '^sha256:[a-f0-9]{64}$'
    AND jsonb_typeof(http_binding->'operation_id')='string'
    AND jsonb_typeof(http_binding->'method')='string'
    AND jsonb_typeof(http_binding->'path')='string'
    AND jsonb_typeof(http_binding->'required_action')='string'
    AND length(http_binding->>'operation_id') BETWEEN 1 AND 128
    AND http_binding->>'method' IN ('GET','POST','PUT','PATCH','DELETE','HEAD')
    AND length(http_binding->>'path') BETWEEN 1 AND 2048
    AND length(http_binding->>'required_action') BETWEEN 1 AND 160
  )
);
CREATE FUNCTION protect_call_http_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF OLD.http_binding IS NOT NULL AND NEW.http_binding IS DISTINCT FROM OLD.http_binding THEN
    RAISE EXCEPTION 'Admitted HTTP call binding is immutable' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER call_http_binding_immutable
  BEFORE UPDATE OF http_binding ON calls
  FOR EACH ROW EXECUTE FUNCTION protect_call_http_binding();
