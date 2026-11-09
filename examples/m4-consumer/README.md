# M4 consumer acceptance fixture

This independent ordinary Python plugin exercises fixed dependency resolution,
HostService capability and Skill forwarding, persistent event dedupe, a leased
subscription and duplicate acknowledgment. It uses generated protobuf bindings
and the kernel-issued workload certificate, without a kernel SDK.

`org.xelab.m4-consumer.run` takes exactly `message`, `skill_id` and
`subscription_id`. The Skill must be the registered echo candidate; create a
fresh subscription to `org.xelab.m4-consumer.completed` before invoking. The
required echo dependency is fixed at deployment scope. All six proxy actions
are optional requests and must be explicitly approved. No input selects a URL.

The capability uses conservative `non_repeatable` semantics. A failed or
interrupted invocation may have created downstream calls or published/acked an
event; inspect the persistent core records rather than replaying with a new key.
The consumer never retries a business request automatically. Its bounded local
result cache supports only the same live instance and is not crash recovery.

## Fixture-only diagnostics

Only this signed reference image may attach the response-only initial binary
metadata key `xelab-m4-fixture-diag-bin` to `CapabilityService.QueryResult`.
The requester must have the actual Kernel mTLS identity and supply the exact
original business key for that failed Call. Successful and unrecorded Calls,
wrong keys and other peers receive no diagnostic metadata. The response remains
`UNKNOWN` with no outcome: child effects may have committed. The diagnostic is
advisory, never a result, an authorization proof or permission to replay.

The decoded metadata is at most 1024 bytes of UTF-8 JSON. Its closed schema has
required fields `version` (integer exactly `1`), `code` (exactly
`M4_FIXTURE_DIAGNOSTIC`), `stage`, `kind` and `status`. Unknown fields, arrays,
nulls and other value types are rejected. `stage` is one of:

```text
host.open
dependency.resolve, dependency.resolve.validate
dependency.invoke, dependency.invoke.validate
skill.resolve, skill.resolve.validate
skill.check, skill.check.validate
skill.invoke, skill.invoke.validate
event.publish, event.publish.duplicate, event.publish.validate
event.subscribe, event.delivery.validate
event.ack, event.ack.duplicate, event.ack.validate
unknown
```

`kind` is exactly `RPC`, `ValueError`, `KeyError`, `TypeError`, `StopIteration`
or `OSError`; `status` is an exact standard gRPC status enum name. Optional fields
are booleans only: `binding_contract_equal`, `binding_deadline_within_parent`,
`binding_unexpired`, `skill_ready`, `skill_target_equal`, `skill_access_ready`,
`event_duplicate_same` and `delivery_event_equal`. No input, payload, key,
delegation, certificate, lease or arbitrary exception text is included.

The local success/failure/active ledger shares a 1024-Call bound. Failed Calls
cannot dispatch Host methods again under the same ID; changed fingerprints or
business keys are refused. The OCI test harness alone reads and sanitizes this
metadata. Production clients and HostService do not consume or accept this key;
their metadata guards and the actual QueryResult 2000 ms deadline are unchanged.

```bash
python3 examples/m4-consumer/build_artifact.py --publisher-id <publisher-uuid> \
  --provider-manifest examples/generated/xelab/plugin.json
docker build -f examples/m4-consumer/Dockerfile -t xelab-m4-consumer .
python3 examples/m4-consumer/test_fixture.py
```

Install `grpcio-tools` and `jsonschema` in the test environment first. The
provider contract is copied into the image member `/xelab/dependency-contract.json`;
its digest comes from the existing echo manifest or reference builder. The image
digest covers this member. Topic schemas remain inline in the signed manifest.

The tests generate real protobuf bindings in a temporary directory and validate
schemas plus mocked protocol responses. They do not prove OCI execution, mTLS
network identity, cgroup isolation or production host acceptance. The privileged
integration harness must install/sign the actual image and retain its CI logs.
AWS remains DEFERRED; formal production host acceptance remains in Issue #50.

The existing ignored OCI lifecycle test requires `XELAB_M4_OCI_REQUIRED=1`
and the imported immutable `XELAB_TEST_M4_CONSUMER_IMAGE`. The CI job builds
this image and checks a fresh `XELAB_M4_OCI_EVIDENCE` receipt plus the
`M4_OCI_REQUIRED_ACCEPTANCE_PASSED` sentinel; absent assets are errors.
The real scenario invokes both consumers sharing one provider within the
unchanged component resource pool, checks their fixed installation/generation,
and verifies actual process exit and retained/released reservations when the
required provider is disabled. A Docker import/readonly probe is packaging
evidence only and cannot substitute for this privileged scenario.
