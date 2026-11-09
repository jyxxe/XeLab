-- Keep the original schema fingerprint immutable. Existing empty M4 tables
-- acquire exact source pins and original authorization provenance.
ALTER TABLE dependency_bindings ADD COLUMN consumer_installation_id uuid REFERENCES installations(id);
ALTER TABLE dependency_bindings ADD COLUMN provider_installation_id uuid REFERENCES installations(id);
ALTER TABLE dependency_bindings ADD COLUMN provider_manifest_digest text;
ALTER TABLE dependency_bindings ADD COLUMN provider_plugin_id text;
ALTER TABLE dependency_bindings ADD COLUMN provider_spec jsonb;
ALTER TABLE dependency_bindings ADD COLUMN provider_generation bigint;
ALTER TABLE dependency_bindings ADD COLUMN required_features jsonb NOT NULL DEFAULT '[]';
ALTER TABLE dependency_bindings ADD COLUMN failure_latched boolean NOT NULL DEFAULT false;
ALTER TABLE dependency_bindings ADD COLUMN required boolean NOT NULL DEFAULT true;
ALTER TABLE dependency_bindings ADD COLUMN failure_policy text NOT NULL DEFAULT 'wait' CHECK(failure_policy IN ('wait','fail','fallback','degrade'));
ALTER TABLE dependency_bindings ADD COLUMN issuer_credential_id uuid REFERENCES credentials(id);
ALTER TABLE dependency_bindings ADD COLUMN subject_id uuid REFERENCES accounts(id);
ALTER TABLE dependency_bindings ADD COLUMN peer inet;
ALTER TABLE dependency_bindings ADD COLUMN browser_origin text;
ALTER TABLE dependency_bindings ADD COLUMN source_paths jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(source_paths)='array' AND octet_length(source_paths::text)<=1048576);
ALTER TABLE dependency_bindings ADD COLUMN bound_call uuid REFERENCES calls(id);
ALTER TABLE dependency_bindings ADD COLUMN parent_binding_id uuid REFERENCES dependency_bindings(id);
CREATE UNIQUE INDEX dependency_deployment_alias ON dependency_bindings(consumer_deployment_id,alias) WHERE bound_call IS NULL;
CREATE UNIQUE INDEX dependency_request_binding ON dependency_bindings(consumer_deployment_id,bound_call,parent_binding_id) WHERE bound_call IS NOT NULL;
CREATE INDEX dependency_provider ON dependency_bindings(provider_deployment_id);
CREATE TABLE dependency_upgrade_plans (
  operation_id uuid PRIMARY KEY REFERENCES operations(id),
  consumer_deployment_id uuid NOT NULL REFERENCES deployments(id),
  target_installation_id uuid NOT NULL REFERENCES installations(id),
  bindings jsonb NOT NULL CHECK(jsonb_typeof(bindings)='array' AND octet_length(bindings::text)<=1048576),
  source_paths jsonb NOT NULL CHECK(jsonb_typeof(source_paths)='array' AND octet_length(source_paths::text)<=1048576)
);
