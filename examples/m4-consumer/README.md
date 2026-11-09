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
The real scenario starts two consumers sharing one provider within the
unchanged component resource pool, checks their fixed installation/generation,
and verifies actual process exit and retained/released reservations when the
required provider is disabled. A Docker import/readonly probe is packaging
evidence only and cannot substitute for this privileged scenario.
