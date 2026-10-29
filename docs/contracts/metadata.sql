-- Xelab specification 2.6.0: PostgreSQL logical baseline, not an applied migration.
-- UUID creation is performed by the application; timestamps are UTC timestamptz.
BEGIN;
CREATE TABLE schema_migrations (
  version bigint PRIMARY KEY, checksum text NOT NULL, applied_at timestamptz NOT NULL
);
CREATE TABLE accounts (
  id uuid PRIMARY KEY, parent_id uuid REFERENCES accounts(id),
  kind text NOT NULL CHECK (kind IN ('user','service')),
  status text NOT NULL CHECK (status IN ('pending','active','disabled','archived')),
  display_name varchar(80) NOT NULL,
  independent_owner boolean NOT NULL DEFAULT false,
  password_hash text, activation_digest bytea, activation_expires_at timestamptz,
  expires_at timestamptz,
  revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
  authz_revision bigint NOT NULL DEFAULT 1 CHECK (authz_revision > 0),
  created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  CHECK (parent_id IS DISTINCT FROM id),
  CHECK (NOT independent_owner OR (parent_id IS NULL AND kind='user'))
);
CREATE INDEX accounts_parent_idx ON accounts(parent_id,created_at,id);
CREATE TABLE account_closure (
  ancestor_id uuid REFERENCES accounts(id), descendant_id uuid REFERENCES accounts(id),
  depth integer NOT NULL CHECK (depth >= 0 AND depth < 8),
  PRIMARY KEY (ancestor_id,descendant_id),
  CHECK ((ancestor_id=descendant_id AND depth=0) OR (ancestor_id<>descendant_id AND depth>0))
);
CREATE TABLE role_definitions (
  id text PRIMARY KEY CHECK (id IN ('Owner','Operator','Member','Auditor','Guest')),
  definition jsonb NOT NULL, version bigint NOT NULL CHECK (version>0)
);
CREATE TABLE permission_definitions (
  action text PRIMARY KEY, resource_kinds jsonb NOT NULL,
  description text NOT NULL, self_service boolean NOT NULL DEFAULT false
);
CREATE TABLE permission_groups (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), name varchar(80) NOT NULL,
  visibility text NOT NULL CHECK (visibility IN ('private','public')),
  latest_version bigint NOT NULL CHECK (latest_version>0),
  status text NOT NULL CHECK (status IN ('active','archived')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE group_versions (
  group_id uuid REFERENCES permission_groups(id), version bigint CHECK(version>0),
  rules jsonb NOT NULL, created_by uuid NOT NULL REFERENCES accounts(id), created_at timestamptz NOT NULL,
  PRIMARY KEY(group_id,version), CHECK (jsonb_typeof(rules)='array')
);
CREATE TABLE credentials (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id),
  kind text NOT NULL CHECK(kind IN ('session','api_key','oauth_access','oauth_refresh')),
  name varchar(80), prefix varchar(32), hmac_key_id text NOT NULL, verification_digest bytea NOT NULL,
  issuer_credential_id uuid REFERENCES credentials(id), client_id text,
  family_id uuid, status text NOT NULL CHECK(status IN ('active','revoked','expired')),
  inheritance_mode text CHECK(inheritance_mode IN ('selected','all_snapshot','follow_parent')),
  maximum_rules jsonb, conditions jsonb NOT NULL DEFAULT '{}',
  expires_at timestamptz NOT NULL, last_used_at timestamptz,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(hmac_key_id,verification_digest), CHECK(issuer_credential_id IS DISTINCT FROM id)
);
CREATE INDEX credentials_owner_idx ON credentials(owner_id,status);
CREATE INDEX credentials_issuer_idx ON credentials(issuer_credential_id);
CREATE TABLE grants (
  id uuid PRIMARY KEY, subject_id uuid NOT NULL REFERENCES accounts(id),
  issuer_id uuid NOT NULL REFERENCES accounts(id), issuer_credential_id uuid REFERENCES credentials(id),
  role_id text REFERENCES role_definitions(id), group_id uuid, group_version bigint,
  scope jsonb NOT NULL, expires_at timestamptz,
  status text NOT NULL CHECK(status IN ('active','revoked','expired')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  FOREIGN KEY(group_id,group_version) REFERENCES group_versions(group_id,version),
  CHECK ((role_id IS NOT NULL AND group_id IS NULL AND group_version IS NULL)
      OR (role_id IS NULL AND group_id IS NOT NULL AND group_version IS NOT NULL))
);
CREATE INDEX grants_subject_idx ON grants(subject_id,status);
CREATE TABLE inheritance_grants (
  account_id uuid PRIMARY KEY REFERENCES accounts(id), parent_id uuid NOT NULL REFERENCES accounts(id),
  mode text NOT NULL CHECK(mode IN ('selected','all_snapshot','follow_parent')),
  maximum_rules jsonb, issuer_credential_id uuid REFERENCES credentials(id),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  CHECK (account_id<>parent_id), CHECK(mode='follow_parent' OR maximum_rules IS NOT NULL)
);
CREATE TABLE grant_provenance (
  id uuid PRIMARY KEY, derived_account_id uuid REFERENCES accounts(id),
  derived_credential_id uuid REFERENCES credentials(id), source_grant_id uuid NOT NULL REFERENCES grants(id),
  source_rules jsonb NOT NULL,
  CHECK((derived_account_id IS NOT NULL) <> (derived_credential_id IS NOT NULL))
);
CREATE TABLE quotas (
  account_id uuid PRIMARY KEY REFERENCES accounts(id), spec jsonb NOT NULL,
  descendants_used bigint NOT NULL DEFAULT 0 CHECK(descendants_used>=0),
  api_keys_used bigint NOT NULL DEFAULT 0 CHECK(api_keys_used>=0),
  concurrent_calls_used bigint NOT NULL DEFAULT 0 CHECK(concurrent_calls_used>=0),
  daily_calls_used bigint NOT NULL DEFAULT 0 CHECK(daily_calls_used>=0), daily_bucket date NOT NULL,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE clients (
  id uuid PRIMARY KEY, client_id text NOT NULL UNIQUE, spec jsonb NOT NULL,
  status text NOT NULL CHECK(status IN ('active','disabled')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE authorization_requests (
  id uuid PRIMARY KEY, session_id uuid NOT NULL REFERENCES credentials(id), client_id text NOT NULL REFERENCES clients(client_id),
  redirect_uri text NOT NULL, state text NOT NULL, code_challenge text NOT NULL,
  fixed_rules jsonb NOT NULL, csrf_digest bytea NOT NULL,
  code_digest bytea UNIQUE, expires_at timestamptz NOT NULL,
  status text NOT NULL CHECK(status IN ('pending','approved','denied','consumed','expired'))
);
CREATE TABLE publishers (
  id uuid PRIMARY KEY, name text NOT NULL, key_id text NOT NULL UNIQUE, public_key_pem text NOT NULL,
  status text NOT NULL CHECK(status IN ('trusted','revoked')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE installations (
  id uuid PRIMARY KEY, plugin_id text NOT NULL, version text NOT NULL, publisher_id uuid NOT NULL REFERENCES publishers(id),
  image_digest text NOT NULL, manifest_digest text NOT NULL, manifest jsonb NOT NULL,
  state text NOT NULL CHECK(state IN ('validated','installed','uninstalling','uninstalled','failed')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(plugin_id,version)
);
CREATE TABLE node_budget (
  id uuid PRIMARY KEY, singleton boolean NOT NULL DEFAULT true UNIQUE CHECK(singleton),
  config jsonb NOT NULL, host_cpu_millis bigint NOT NULL, host_memory_bytes bigint NOT NULL,
  reserved_cpu_millis bigint NOT NULL DEFAULT 0 CHECK(reserved_cpu_millis>=0),
  reserved_memory_bytes bigint NOT NULL DEFAULT 0 CHECK(reserved_memory_bytes>=0),
  admission_enabled boolean NOT NULL DEFAULT false,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE skills (
  id uuid PRIMARY KEY, name text NOT NULL, version text NOT NULL,
  installation_id uuid NOT NULL REFERENCES installations(id),
  created_by uuid NOT NULL REFERENCES accounts(id),
  definition jsonb NOT NULL, definition_digest text NOT NULL,
  enabled boolean NOT NULL DEFAULT false, archived_at timestamptz,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(name,version)
);
CREATE TABLE deployments (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), installation_id uuid NOT NULL REFERENCES installations(id),
  spec jsonb NOT NULL, approval jsonb NOT NULL DEFAULT '{}', observed_generation bigint NOT NULL DEFAULT 0,
  phase text NOT NULL CHECK(phase IN ('disabled','waiting_resources','reconciling','ready','degraded','failed','draining')),
  conditions jsonb NOT NULL DEFAULT '[]', archived_at timestamptz,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE instances (
  id uuid PRIMARY KEY, deployment_id uuid NOT NULL REFERENCES deployments(id), generation bigint NOT NULL,
  ordinal integer NOT NULL CHECK(ordinal>=0), runtime_id text UNIQUE,
  phase text NOT NULL CHECK(phase IN ('pending','starting','ready','degraded','draining','stopped','failed')),
  health text NOT NULL, limits jsonb NOT NULL, restart_count integer NOT NULL DEFAULT 0,
  last_heartbeat_at timestamptz, revision bigint NOT NULL DEFAULT 1,
  created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(deployment_id,generation,ordinal)
);
CREATE TABLE reservations (
  id uuid PRIMARY KEY, instance_id uuid UNIQUE REFERENCES instances(id), account_id uuid REFERENCES accounts(id),
  kind text NOT NULL CHECK(kind IN ('instance','call')), cpu_millis bigint NOT NULL DEFAULT 0,
  memory_bytes bigint NOT NULL DEFAULT 0, expires_at timestamptz,
  status text NOT NULL CHECK(status IN ('reserved','active','released')), created_at timestamptz NOT NULL
);
CREATE TABLE volumes (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), spec jsonb NOT NULL,
  backend_id text NOT NULL UNIQUE, capacity_bytes bigint NOT NULL CHECK(capacity_bytes>=1048576),
  used_bytes bigint NOT NULL DEFAULT 0, write_fence bigint NOT NULL DEFAULT 0,
  writer_instance_id uuid REFERENCES instances(id),
  state text NOT NULL CHECK(state IN ('available','attached','full','deleting','deleted')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE volume_attachments (
  volume_id uuid REFERENCES volumes(id), instance_id uuid REFERENCES instances(id),
  access text NOT NULL CHECK(access IN ('ro','rw')), fence bigint NOT NULL,
  PRIMARY KEY(volume_id,instance_id)
);
CREATE TABLE backups (
  id uuid PRIMARY KEY, deployment_id uuid NOT NULL REFERENCES deployments(id), created_by uuid NOT NULL REFERENCES accounts(id),
  volume_ids jsonb NOT NULL, object_reference text, manifest_digest text, off_host boolean NOT NULL,
  state text NOT NULL CHECK(state IN ('creating','ready','failed','expired')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE secrets (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), name text NOT NULL,
  version bigint NOT NULL DEFAULT 1, encrypted_value bytea NOT NULL, encryption_key_id text NOT NULL,
  expires_at timestamptz, status text NOT NULL CHECK(status IN ('active','revoked','expired')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE secret_bindings (
  id uuid PRIMARY KEY, deployment_id uuid NOT NULL REFERENCES deployments(id), secret_id uuid NOT NULL REFERENCES secrets(id),
  maximum_lease_seconds integer NOT NULL CHECK(maximum_lease_seconds BETWEEN 1 AND 900),
  status text NOT NULL CHECK(status IN ('active','revoked')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE blobs (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), content_type text NOT NULL,
  size_bytes bigint NOT NULL CHECK(size_bytes BETWEEN 0 AND 67108864), digest text NOT NULL,
  backend_reference text NOT NULL UNIQUE, expires_at timestamptz NOT NULL, pin_count bigint NOT NULL DEFAULT 0,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE execution_delegations (
  id uuid PRIMARY KEY, subject_id uuid NOT NULL REFERENCES accounts(id), issuer_credential_id uuid REFERENCES credentials(id),
  maximum_rules jsonb NOT NULL, run_as text NOT NULL CHECK(run_as IN ('user','service')),
  service_account_id uuid REFERENCES accounts(id), expires_at timestamptz NOT NULL,
  status text NOT NULL CHECK(status IN ('active','revoked','expired')), created_at timestamptz NOT NULL
);
CREATE TABLE workflows (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), name text NOT NULL,
  version bigint NOT NULL CHECK(version>0), spec jsonb NOT NULL,
  status text NOT NULL CHECK(status IN ('active','archived')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(owner_id,name,version)
);
CREATE TABLE tasks (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), workflow_id uuid NOT NULL REFERENCES workflows(id),
  delegation_id uuid NOT NULL REFERENCES execution_delegations(id), input jsonb NOT NULL, output jsonb,
  deadline_at timestamptz NOT NULL, error jsonb,
  state text NOT NULL CHECK(state IN ('queued','running','waiting_dependency','paused_authorization','cancel_requested','succeeded','failed','canceled','unknown')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE task_steps (
  id uuid PRIMARY KEY, task_id uuid NOT NULL REFERENCES tasks(id), step_key text NOT NULL,
  provider_deployment_id uuid REFERENCES deployments(id), attempt_count integer NOT NULL DEFAULT 0,
  state text NOT NULL CHECK(state IN ('pending','ready','running','succeeded','failed','canceled','unknown')),
  output jsonb, fence bigint NOT NULL DEFAULT 0,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(task_id,step_key)
);
CREATE TABLE attempts (
  id uuid PRIMARY KEY, step_id uuid NOT NULL REFERENCES task_steps(id), number integer NOT NULL,
  execution_id uuid NOT NULL UNIQUE, idempotency_key text NOT NULL, fence bigint NOT NULL,
  lease_expires_at timestamptz NOT NULL, checkpoint_blob_id uuid REFERENCES blobs(id),
  state text NOT NULL CHECK(state IN ('leased','running','succeeded','failed','expired','unknown')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(step_id,number), UNIQUE(step_id,fence)
);
CREATE UNIQUE INDEX attempts_one_active ON attempts(step_id) WHERE state IN ('leased','running');
CREATE INDEX task_steps_claim_idx ON task_steps(created_at,id) WHERE state='ready';
CREATE TABLE schedules (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), spec jsonb NOT NULL,
  delegation_id uuid NOT NULL REFERENCES execution_delegations(id), next_fire_at timestamptz,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE schedule_fires (
  schedule_id uuid REFERENCES schedules(id), scheduled_at timestamptz, task_id uuid REFERENCES tasks(id),
  outcome text NOT NULL CHECK(outcome IN ('pending','created','skipped','paused')),
  PRIMARY KEY(schedule_id,scheduled_at)
);
CREATE TABLE events (
  id uuid PRIMARY KEY, publisher_id uuid NOT NULL REFERENCES accounts(id), topic text NOT NULL,
  version text NOT NULL, payload jsonb NOT NULL, dedupe_key text NOT NULL, expires_at timestamptz NOT NULL,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE INDEX events_topic_idx ON events(topic,created_at,id);
CREATE TABLE event_dedupe (
  publisher_id uuid REFERENCES accounts(id), topic text, version text, dedupe_key text,
  event_id uuid NOT NULL REFERENCES events(id), payload_digest text NOT NULL, expires_at timestamptz NOT NULL,
  PRIMARY KEY(publisher_id,topic,version,dedupe_key)
);
CREATE TABLE subscriptions (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), spec jsonb NOT NULL,
  status text NOT NULL CHECK(status IN ('active','disabled')), cursor uuid REFERENCES events(id),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE event_deliveries (
  id uuid PRIMARY KEY, subscription_id uuid NOT NULL REFERENCES subscriptions(id), event_id uuid NOT NULL REFERENCES events(id),
  attempt integer NOT NULL, lease_digest bytea NOT NULL, lease_expires_at timestamptz NOT NULL,
  state text NOT NULL CHECK(state IN ('leased','acked','retry','dead_letter')),
  created_at timestamptz NOT NULL, UNIQUE(subscription_id,event_id,attempt)
);
CREATE UNIQUE INDEX delivery_one_leased ON event_deliveries(subscription_id) WHERE state='leased';
CREATE TABLE capability_contracts (
  name text, version text, schema_digest text NOT NULL, spec jsonb NOT NULL,
  PRIMARY KEY(name,version)
);
CREATE TABLE event_contracts (
  topic text, version text, publisher_installation_id uuid NOT NULL REFERENCES installations(id),
  schema_digest text NOT NULL, payload_schema jsonb NOT NULL,
  PRIMARY KEY(topic,version)
);
CREATE TABLE model_aliases (
  id uuid PRIMARY KEY, name text NOT NULL UNIQUE, spec jsonb NOT NULL,
  provider_deployment_id uuid NOT NULL REFERENCES deployments(id), skill_id uuid REFERENCES skills(id), enabled boolean NOT NULL DEFAULT true,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE provider_records (
  instance_id uuid REFERENCES instances(id), name text, version text,
  availability text NOT NULL CHECK(availability IN ('ready','degraded','unavailable')),
  features jsonb NOT NULL, observed_generation bigint NOT NULL, observed_at timestamptz NOT NULL,
  PRIMARY KEY(instance_id,name,version), FOREIGN KEY(name,version) REFERENCES capability_contracts(name,version)
);
CREATE TABLE dependency_bindings (
  id uuid PRIMARY KEY, consumer_deployment_id uuid NOT NULL REFERENCES deployments(id), task_id uuid REFERENCES tasks(id),
  alias text NOT NULL, provider_deployment_id uuid NOT NULL REFERENCES deployments(id),
  instance_id uuid REFERENCES instances(id), contract jsonb NOT NULL, binding_scope text NOT NULL,
  expires_at timestamptz, revision bigint NOT NULL DEFAULT 1
);
CREATE TABLE micro_plugins (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), name text NOT NULL,
  display_name text NOT NULL, status text NOT NULL CHECK(status IN ('active','archived')),
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  UNIQUE(owner_id,name)
);
CREATE TABLE micro_versions (
  id uuid PRIMARY KEY, micro_plugin_id uuid NOT NULL REFERENCES micro_plugins(id), version text NOT NULL,
  manifest jsonb NOT NULL, source_digest text NOT NULL, package_digest text NOT NULL,
  source_bytes bigint NOT NULL CHECK(source_bytes BETWEEN 1 AND 262144),
  artifact_object_key text NOT NULL UNIQUE,
  runtime_installation_id uuid NOT NULL REFERENCES installations(id), created_at timestamptz NOT NULL,
  UNIQUE(micro_plugin_id,version)
);
CREATE TABLE micro_deployments (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id),
  micro_plugin_id uuid NOT NULL REFERENCES micro_plugins(id), micro_version_id uuid NOT NULL REFERENCES micro_versions(id),
  runtime_deployment_id uuid NOT NULL REFERENCES deployments(id), runtime_installation_id uuid NOT NULL REFERENCES installations(id),
  package_digest text NOT NULL, limits jsonb NOT NULL, candidate_skill jsonb NOT NULL,
  enabled boolean NOT NULL DEFAULT false, generation bigint NOT NULL DEFAULT 1,
  phase text NOT NULL CHECK(phase IN ('disabled','preparing','ready','waiting_runtime','failed','archived')),
  reason_codes jsonb NOT NULL DEFAULT '[]', revision bigint NOT NULL DEFAULT 1,
  created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE micro_runtime_bindings (
  id uuid PRIMARY KEY, micro_deployment_id uuid NOT NULL REFERENCES micro_deployments(id),
  runtime_instance_id uuid NOT NULL REFERENCES instances(id), runtime_generation bigint NOT NULL,
  micro_generation bigint NOT NULL, package_digest text NOT NULL, prepared_handle text NOT NULL,
  expires_at timestamptz NOT NULL, status text NOT NULL CHECK(status IN ('ready','released','expired')),
  UNIQUE(micro_deployment_id,runtime_instance_id,runtime_generation,micro_generation)
);
ALTER TABLE skills ADD COLUMN source_kind text NOT NULL DEFAULT 'oci' CHECK(source_kind IN ('oci','micro'));
CREATE TABLE micro_http_bindings (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), micro_plugin_id uuid NOT NULL REFERENCES micro_plugins(id),
  micro_deployment_id uuid NOT NULL REFERENCES micro_deployments(id), endpoint_alias text NOT NULL,
  priority integer NOT NULL CHECK(priority BETWEEN 0 AND 1000), enabled boolean NOT NULL DEFAULT false,
  secret_id uuid NOT NULL REFERENCES secrets(id), timeout_ms integer NOT NULL CHECK(timeout_ms BETWEEN 100 AND 4000),
  approved_package_digest text NOT NULL, approved_endpoint_digest text NOT NULL,
  deleted_at timestamptz, revision bigint NOT NULL DEFAULT 1,
  created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX micro_http_live_alias ON micro_http_bindings(micro_deployment_id,endpoint_alias) WHERE deleted_at IS NULL;
ALTER TABLE skills ADD COLUMN micro_version_id uuid REFERENCES micro_versions(id);
ALTER TABLE skills ADD COLUMN micro_deployment_id uuid REFERENCES micro_deployments(id);
ALTER TABLE skills ADD CONSTRAINT skill_source_binding CHECK(
  (source_kind='oci' AND micro_version_id IS NULL AND micro_deployment_id IS NULL)
  OR (source_kind='micro' AND micro_version_id IS NOT NULL AND micro_deployment_id IS NOT NULL)
);
CREATE TABLE calls (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), credential_id uuid REFERENCES credentials(id),
  delegation_id uuid REFERENCES execution_delegations(id), provider_instance_id uuid REFERENCES instances(id),
  skill_id uuid REFERENCES skills(id), skill_definition_digest text,
  micro_plugin_id uuid REFERENCES micro_plugins(id), micro_version_id uuid REFERENCES micro_versions(id),
  micro_deployment_id uuid REFERENCES micro_deployments(id), package_digest text,
  parent_call_id uuid REFERENCES calls(id), root_call_id uuid REFERENCES calls(id),
  contract jsonb NOT NULL, input_digest text NOT NULL, idempotency_key text,
  state text NOT NULL CHECK(state IN ('accepted','running','succeeded','failed','canceled','unknown')),
  result jsonb, error jsonb, deadline_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL, completed_at timestamptz
);
CREATE TABLE micro_http_plans (
  call_id uuid PRIMARY KEY REFERENCES calls(id), micro_deployment_id uuid NOT NULL REFERENCES micro_deployments(id),
  package_digest text NOT NULL, snapshot_digest text NOT NULL, binding_snapshot jsonb NOT NULL,
  state text NOT NULL CHECK(state IN ('active','blocked_unknown','blocked_error','finished')),
  terminal_error jsonb, created_at timestamptz NOT NULL, expires_at timestamptz NOT NULL
);
CREATE TABLE micro_http_operations (
  id uuid PRIMARY KEY, call_id uuid NOT NULL REFERENCES micro_http_plans(call_id),
  sequence integer NOT NULL CHECK(sequence BETWEEN 1 AND 3), endpoint_alias text NOT NULL,
  binding_id uuid NOT NULL REFERENCES micro_http_bindings(id), binding_revision bigint NOT NULL,
  secret_version bigint NOT NULL, request_digest text NOT NULL,
  state text NOT NULL CHECK(state IN ('prepared','sending','completed','not_sent','unknown')),
  http_status integer CHECK(http_status BETWEEN 100 AND 599), safe_response_json jsonb, error_code text,
  retry_after_ms integer, duration_ms integer, created_at timestamptz NOT NULL, completed_at timestamptz,
  UNIQUE(call_id,sequence), UNIQUE(call_id,endpoint_alias)
);
CREATE TABLE micro_call_diagnostics (
  call_id uuid PRIMARY KEY REFERENCES calls(id), micro_plugin_id uuid NOT NULL REFERENCES micro_plugins(id),
  micro_version_id uuid NOT NULL REFERENCES micro_versions(id), micro_deployment_id uuid NOT NULL REFERENCES micro_deployments(id),
  package_digest text NOT NULL, elapsed_ms bigint NOT NULL CHECK(elapsed_ms BETWEEN 0 AND 30000),
  peak_heap_bytes bigint NOT NULL CHECK(peak_heap_bytes BETWEEN 0 AND 67108864),
  stderr text NOT NULL CHECK(octet_length(stderr)<=65536), stderr_truncated boolean NOT NULL,
  expires_at timestamptz NOT NULL
);
CREATE TABLE operations (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), kind text NOT NULL,
  state text NOT NULL CHECK(state IN ('pending','running','cancel_requested','succeeded','failed','canceled')),
  resource_id text, result jsonb, error jsonb, progress_percent integer NOT NULL DEFAULT 0,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE rpc_delegations (
  id uuid PRIMARY KEY, execution_delegation_id uuid NOT NULL REFERENCES execution_delegations(id),
  subject_id uuid NOT NULL REFERENCES accounts(id), instance_id uuid NOT NULL REFERENCES instances(id),
  generation bigint NOT NULL CHECK(generation>0),
  call_id uuid REFERENCES calls(id), execution_id uuid REFERENCES attempts(execution_id),
  parent_rpc_delegation_id uuid REFERENCES rpc_delegations(id),
  issuer_credential_id uuid REFERENCES credentials(id),
  hmac_key_id text NOT NULL, verification_digest bytea NOT NULL,
  expires_at timestamptz NOT NULL, status text NOT NULL CHECK(status IN ('active','revoked','expired')),
  created_at timestamptz NOT NULL, UNIQUE(hmac_key_id,verification_digest),
  CHECK((call_id IS NOT NULL) <> (execution_id IS NOT NULL)),
  CHECK(parent_rpc_delegation_id IS DISTINCT FROM id)
);
CREATE TABLE idempotency_records (
  subject_id uuid REFERENCES accounts(id), credential_id uuid REFERENCES credentials(id), method text, path text, key text,
  fingerprint text NOT NULL, operation_id uuid REFERENCES operations(id), resource_id text,
  response_status integer, safe_response jsonb, sensitive boolean NOT NULL DEFAULT false,
  expires_at timestamptz NOT NULL, created_at timestamptz NOT NULL,
  PRIMARY KEY(subject_id,credential_id,method,path,key), CHECK(NOT sensitive OR safe_response IS NULL)
);
CREATE TABLE outbox (
  id uuid PRIMARY KEY, topic text NOT NULL, aggregate_id uuid NOT NULL, payload jsonb NOT NULL,
  created_at timestamptz NOT NULL, delivered_at timestamptz, attempts integer NOT NULL DEFAULT 0
);
CREATE INDEX outbox_pending_idx ON outbox(created_at,id) WHERE delivered_at IS NULL;
CREATE TABLE audit_events (
  id uuid PRIMARY KEY, subject_id uuid REFERENCES accounts(id), actor_instance_id uuid REFERENCES instances(id),
  credential_id uuid REFERENCES credentials(id), delegation_id uuid REFERENCES execution_delegations(id),
  request_id uuid NOT NULL, action text NOT NULL, resource jsonb NOT NULL,
  outcome text NOT NULL CHECK(outcome IN ('allowed','denied','succeeded','failed')),
  policy_revision bigint NOT NULL, reason_code text NOT NULL,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE INDEX audit_subject_idx ON audit_events(subject_id,created_at,id);
CREATE TABLE default_ui (
  id uuid PRIMARY KEY, singleton boolean NOT NULL DEFAULT true UNIQUE CHECK(singleton),
  deployment_id uuid REFERENCES deployments(id), revision bigint NOT NULL DEFAULT 1,
  created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
CREATE TABLE skill_batches (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL REFERENCES accounts(id), subject_id uuid NOT NULL REFERENCES accounts(id),
  delegation_id uuid NOT NULL REFERENCES execution_delegations(id), skill_id uuid NOT NULL REFERENCES skills(id),
  installation_id uuid NOT NULL REFERENCES installations(id), definition_digest text NOT NULL,
  state text NOT NULL CHECK(state IN ('queued','running','waiting_dependency','paused_authorization','cancel_requested','succeeded','partially_succeeded','failed','canceled','unknown')),
  request_spec jsonb NOT NULL, counts jsonb NOT NULL, total_items integer NOT NULL CHECK(total_items BETWEEN 1 AND 1000),
  max_parallel integer NOT NULL CHECK(max_parallel BETWEEN 1 AND 16), failure_policy text NOT NULL CHECK(failure_policy IN ('continue','stop')),
  run_as text NOT NULL CHECK(run_as IN ('user','service')), service_account_id uuid REFERENCES accounts(id),
  deadline_at timestamptz NOT NULL, completed_at timestamptz, results_expires_at timestamptz, error jsonb,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL,
  CHECK((run_as='service')=(service_account_id IS NOT NULL))
);
CREATE TABLE skill_batch_items (
  batch_id uuid NOT NULL REFERENCES skill_batches(id), item_id text NOT NULL,
  item_index integer NOT NULL CHECK(item_index BETWEEN 0 AND 999), input jsonb, input_digest text NOT NULL,
  input_bytes bigint NOT NULL CHECK(input_bytes BETWEEN 0 AND 4194304),
  idempotency_key text NOT NULL, call_id uuid REFERENCES calls(id),
  state text NOT NULL CHECK(state IN ('queued','running','succeeded','failed','canceled','unknown')),
  attempts integer NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 5), worker_id uuid, execution_id uuid,
  fence bigint NOT NULL DEFAULT 0, lease_expires_at timestamptz,
  result_object_key text, result_digest text, result_bytes bigint CHECK(result_bytes BETWEEN 0 AND 16777216), error jsonb,
  resolution_source text CHECK(resolution_source IN ('automatic','manual')), resolution_evidence text,
  resolved_by uuid REFERENCES accounts(id), resolved_at timestamptz,
  started_at timestamptz, completed_at timestamptz, revision bigint NOT NULL DEFAULT 1,
  PRIMARY KEY(batch_id,item_id), UNIQUE(batch_id,item_index), UNIQUE(batch_id,idempotency_key)
);
CREATE INDEX skill_batch_claim_idx ON skill_batch_items(batch_id,item_index) WHERE state='queued';
-- request_spec stores policy only; item input occurs only in skill_batch_items.input.
CREATE INDEX skill_batch_owner_idx ON skill_batches(owner_id,created_at,id);
CREATE TABLE ui_preferences (
  id uuid PRIMARY KEY, owner_id uuid NOT NULL UNIQUE REFERENCES accounts(id), spec jsonb NOT NULL,
  revision bigint NOT NULL DEFAULT 1, created_at timestamptz NOT NULL, updated_at timestamptz NOT NULL
);
-- UI contributions derive from signed installation manifests; no unsigned mutable registry.
-- Application transaction requirements are normative in operations/persistence.md.
-- No ON DELETE CASCADE: lineage, history and data must never disappear implicitly.
COMMIT;
