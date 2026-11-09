-- Private, caller-bound Broker handles; raw user/delegation tokens never persist.
CREATE TABLE host_call_tickets (
 id uuid PRIMARY KEY,
 parent_call_id uuid NOT NULL REFERENCES calls(id),
 instance_id uuid NOT NULL REFERENCES instances(id),
 generation bigint NOT NULL CHECK(generation>0),
 delegation_digest bytea NOT NULL CHECK(octet_length(delegation_digest)=32),
 verification_digest bytea NOT NULL UNIQUE CHECK(octet_length(verification_digest)=32),
 rpc_context jsonb NOT NULL CHECK(jsonb_typeof(rpc_context)='object' AND octet_length(rpc_context::text)<=1024),
 invocation jsonb NOT NULL CHECK(jsonb_typeof(invocation)='object' AND octet_length(invocation::text)<=1024),
 bounds jsonb NOT NULL CHECK(jsonb_typeof(bounds)='array' AND jsonb_array_length(bounds)<=4096 AND octet_length(bounds::text)<=2097152),
 provider_deployment_id uuid REFERENCES deployments(id),
 idempotency_key text NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
 fingerprint text NOT NULL,
 expected_skill uuid REFERENCES skills(id),
 expected_contract jsonb,
 input_digest text NOT NULL,
 requested_call_id uuid,
 dependency_binding_id uuid, -- Tombstoned/deleted bindings must fail closed on reuse.
 expires_at timestamptz NOT NULL
);
CREATE UNIQUE INDEX host_ticket_replay ON host_call_tickets(parent_call_id,idempotency_key);
CREATE TABLE host_child_calls (
 child_call_id uuid PRIMARY KEY REFERENCES calls(id),
 parent_call_id uuid NOT NULL REFERENCES calls(id),
 ticket_id uuid NOT NULL REFERENCES host_call_tickets(id),
 CHECK(child_call_id<>parent_call_id)
);
