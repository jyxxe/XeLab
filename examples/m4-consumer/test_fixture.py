#!/usr/bin/env python3
"""Schema and mocked protocol tests. These are not OCI or mTLS acceptance."""
import hashlib
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import build_artifact as builder


class Abort(Exception):
    def __init__(self, code, message):
        self.code = code
        super().__init__(message)


class Context:
    def auth_context(self):
        return {"x509_subject_alternative_name": [b"kernel.xelab.local", b"spiffe://xelab.local/kernel"]}
    def abort(self, code, message):
        raise Abort(code, message)
    def is_active(self):
        return True
    def time_remaining(self):
        return 60
    def add_callback(self, callback):
        self.callback = callback


class Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.bindings = tempfile.TemporaryDirectory()
        proto = ROOT / "docs" / "contracts" / "proto"
        subprocess.run([sys.executable, "-m", "grpc_tools.protoc", "-I" + str(proto),
                        "--python_out=" + cls.bindings.name, "--grpc_python_out=" + cls.bindings.name,
                        *[str(proto / "xelab" / "v1" / (name + ".proto"))
                          for name in ["common", "lifecycle", "capability", "host"]]], check=True)
        sys.path.insert(0, cls.bindings.name)
        spec = importlib.util.spec_from_file_location("m4_consumer_server", HERE / "server.py")
        cls.server = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.server)

    @classmethod
    def tearDownClass(cls):
        sys.path.remove(cls.bindings.name)
        cls.bindings.cleanup()

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.publisher = str(uuid.uuid4())
        self.manifest = builder.build(self.root, self.publisher)
        self.instance, self.deployment = str(uuid.uuid4()), str(uuid.uuid4())
        self.environment = mock.patch.dict("os.environ", {"XELAB_INSTANCE_ID": self.instance,
                "XELAB_DEPLOYMENT_ID": self.deployment, "XELAB_GENERATION": "7"})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.consumer = self.server.Consumer(self.root / "xelab" / "plugin.json")
        self.value = {"message": "M4 round trip", "skill_id": str(uuid.uuid4()), "subscription_id": str(uuid.uuid4())}
        payload = self.server.encoded(self.value)
        self.request = self.server.cap.InvokeRequest(call_id=str(uuid.uuid4()), contract=self.consumer.contract,
                input_json=payload, input_digest=self.server.digest(payload),
                context=self.server.common.RpcContext(request_id=str(uuid.uuid4()), delegation_token="opaque-parent",
                    deadline_unix_ms=int(time.time() * 1000) + 60000, idempotency_key="business-key", subject_id=str(uuid.uuid4())))

    def proxy(self):
        server, consumer, value = self.server, self.consumer, self.value
        class Deliveries:
            canceled = False
            def __iter__(self):
                return self
            def __next__(self):
                return server.host.EventDelivery(delivery_id=str(uuid.uuid4()), event_id=proxy.event_id,
                    topic=builder.TOPIC, version="1.0.0", payload_json=server.encoded({"message": value["message"]}),
                    attempt=1, lease_token="opaque-lease", lease_expires_unix_ms=int(time.time() * 1000) + 30000)
            def cancel(self):
                self.canceled = True
        class Proxy:
            def __init__(self):
                self.calls = []
                self.event_id = str(uuid.uuid4())
                self.deliveries = Deliveries()
                self.publishes = 0
            def record(self, name, request, timeout):
                assert 0 < timeout <= 10
                self.calls.append((name, request))
            def Resolve(self, request, timeout):
                self.record("Resolve", request, timeout)
                return server.host.ResolveResponse(binding_id=str(uuid.uuid4()), provider_deployment_id=str(uuid.uuid4()),
                    contract=consumer.dependency, expires_unix_ms=request.context.deadline_unix_ms)
            def result(self, call):
                raw = server.encoded({"message": value["message"]})
                return server.cap.InvokeResponse(call_id=call, output_json=raw, output_digest=server.digest(raw))
            def Invoke(self, request, timeout):
                self.record("Invoke", request, timeout)
                return self.result(request.call_id)
            def ResolveSkill(self, request, timeout):
                self.record("ResolveSkill", request, timeout)
                definition = {"target": {"kind": "capability", "plugin_id": "org.xelab.echo", "contract": consumer.dependency_value}}
                raw = server.encoded(definition)
                return server.host.ResolveSkillResponse(skill_id=request.skill_id, ready=True,
                    definition_json=raw, definition_digest=server.digest(raw))
            def CheckSkillAccess(self, request, timeout):
                self.record("CheckSkillAccess", request, timeout)
                return server.host.CheckSkillAccessResponse(skill_id=request.skill_id,
                    definition_digest=request.expected_definition_digest, ready=True)
            def InvokeSkill(self, request, timeout):
                self.record("InvokeSkill", request, timeout)
                return self.result(str(uuid.uuid4()))
            def PublishEvent(self, request, timeout):
                self.record("PublishEvent", request, timeout)
                self.publishes += 1
                return server.host.PublishEventResponse(event_id=self.event_id, duplicate=self.publishes > 1)
            def Subscribe(self, request, timeout):
                self.record("Subscribe", request, timeout)
                return self.deliveries
            def AckEvent(self, request, timeout):
                self.record("AckEvent", request, timeout)
                return server.common.Ack(request_id=request.context.request_id, accepted=True)
        proxy = Proxy()
        return proxy

    def test_generated_manifest_and_exact_contract_package(self):
        import jsonschema
        from referencing import Registry, Resource
        from referencing.jsonschema import DRAFT202012
        document = json.loads((ROOT / "docs" / "contracts" / "openapi.json").read_bytes())
        registry = Registry().with_resource("urn:xelab:openapi", Resource.from_contents(document, default_specification=DRAFT202012))
        jsonschema.Draft202012Validator({"$ref": "urn:xelab:openapi#/components/schemas/Manifest"},
            registry=registry, format_checker=jsonschema.FormatChecker()).validate(self.manifest)
        cap = self.manifest["capabilities"][0]
        self.assertEqual(cap["side_effect"], "non_repeatable")
        directory = self.root / "xelab" / "contracts" / cap["contract"]["name"] / "1.0.0"
        checks = "".join(name + " " + hashlib.sha256((directory / name).read_bytes()).hexdigest() + "\n"
                         for name in ["input.schema.json", "output.schema.json", "stream.schema.json", "semantics.json"])
        self.assertEqual(cap["contract"]["schema_digest"], builder.digest(checks.encode()))
        schema = self.manifest["event_topics"][0]["payload_schema"]
        self.assertEqual(self.manifest["event_topics"][0]["schema_digest"], builder.digest(json.dumps(schema, sort_keys=True, separators=(",", ":")).encode()))
        dependency = json.loads((self.root / "xelab" / "dependency-contract.json").read_bytes())
        self.assertEqual(dependency, builder.echo_contract(None, self.publisher))
        provider_file = self.root / "provider.json"
        provider_file.write_text(json.dumps({"capabilities":[{"contract":dependency}]}))
        from_provider = self.root / "from-provider"
        builder.build(from_provider, self.publisher, provider_manifest=provider_file)
        self.assertEqual(json.loads((from_provider / "xelab" / "dependency-contract.json").read_bytes()), dependency)
        provider_file.write_text(json.dumps({"capabilities":[{"contract":{**dependency,"url":"https://invalid/"}}]}))
        with self.assertRaises(ValueError):
            builder.build(self.root / "invalid-provider",self.publisher,provider_manifest=provider_file)
        jsonschema.Draft202012Validator(cap["input_schema"], format_checker=jsonschema.FormatChecker()).validate(self.value)
        output = self.consumer.execute(self.proxy(), self.request, self.value, Context(), self.server.threading.Event())
        jsonschema.Draft202012Validator(cap["output_schema"], format_checker=jsonschema.FormatChecker()).validate(output)
        second = self.root / "same"
        builder.build(second, self.publisher)
        self.assertEqual((second / "xelab" / "plugin.json").read_bytes(), (self.root / "xelab" / "plugin.json").read_bytes())

    def test_host_contexts_are_distinct_but_keep_the_original_delegation(self):
        proxy = self.proxy()
        result = self.consumer.execute(proxy, self.request, self.value, Context(), self.server.threading.Event())
        self.assertTrue(result["duplicate"] and result["acked"])
        self.assertEqual(result["event_id"], result["delivery_event_id"])
        self.assertEqual([name for name, _ in proxy.calls], ["Resolve", "Invoke", "ResolveSkill", "CheckSkillAccess", "InvokeSkill", "PublishEvent", "PublishEvent", "Subscribe", "AckEvent", "AckEvent"])
        contexts = [request.context for _, request in proxy.calls]
        self.assertEqual(len({c.request_id for c in contexts}), len(contexts))
        self.assertEqual(len({c.idempotency_key for c in contexts}), len(contexts))
        for context in contexts:
            self.assertEqual(context.delegation_token, self.request.context.delegation_token)
            self.assertEqual(context.subject_id, self.request.context.subject_id)
            self.assertEqual(context.deadline_unix_ms, self.request.context.deadline_unix_ms)
            self.assertEqual(context.actor_instance_id, self.instance)
        self.assertEqual(proxy.calls[0][1].binding_scope_id, self.deployment)
        publishes = [request for name, request in proxy.calls if name == "PublishEvent"]
        self.assertEqual(publishes[0].dedupe_key, self.request.call_id)
        self.assertEqual(publishes[0].dedupe_key, publishes[1].dedupe_key)
        acks = [request for name, request in proxy.calls if name == "AckEvent"]
        self.assertEqual(acks[0].lease_token, acks[1].lease_token)
        self.assertEqual(acks[0].delivery_id, acks[1].delivery_id)
        self.assertTrue(proxy.deliveries.canceled)

    def test_bad_output_dedupe_and_delivery_fail_without_replaying(self):
        proxy = self.proxy()
        original = proxy.Invoke
        def corrupted(request, timeout):
            result = original(request, timeout)
            result.output_digest = self.server.digest(b"wrong")
            return result
        proxy.Invoke = corrupted
        with self.assertRaises(ValueError):
            self.consumer.execute(proxy, self.request, self.value, Context(), self.server.threading.Event())
        self.assertEqual([name for name, _ in proxy.calls], ["Resolve", "Invoke"])
        proxy = self.proxy()
        original = proxy.PublishEvent
        def invalid_duplicate(request, timeout):
            result = original(request, timeout)
            result.duplicate = False
            return result
        proxy.PublishEvent = invalid_duplicate
        with self.assertRaises(ValueError):
            self.consumer.execute(proxy, self.request, self.value, Context(), self.server.threading.Event())
        self.assertNotIn("Subscribe", [name for name, _ in proxy.calls])

    def test_wrong_delivery_is_not_acked_and_cancel_closes_the_child_channel(self):
        proxy = self.proxy()
        original = proxy.Subscribe
        def unrelated(request, timeout):
            deliveries = original(request, timeout)
            event = next(deliveries)
            event.event_id = str(uuid.uuid4())
            class Stream:
                def __iter__(self): return self
                def __next__(self): return event
                def cancel(self): deliveries.cancel()
            return Stream()
        proxy.Subscribe = unrelated
        with self.assertRaises(ValueError):
            self.consumer.execute(proxy, self.request, self.value, Context(), self.server.threading.Event())
        self.assertFalse(any(name == "AckEvent" for name, _ in proxy.calls))
        self.assertTrue(proxy.deliveries.canceled)
        channel = mock.MagicMock()
        event = self.server.threading.Event()
        self.consumer.active[self.request.call_id] = event
        self.consumer.channels[self.request.call_id] = channel
        self.consumer.Cancel(self.server.cap.CancelCallRequest(call_id=self.request.call_id,
            context=self.request.context), Context())
        self.assertTrue(event.is_set())
        channel.close.assert_called_once()

    def test_initialization_identity_untrusted_peer_and_invalid_input(self):
        context = Context()
        with self.assertRaises(Abort) as error:
            self.consumer.Initialize(self.server.life.InitializeRequest(instance_id=self.instance,
                deployment_id=self.deployment, generation=8, config_json=b"{}", config_digest=self.server.digest(b"{}")), context)
        self.assertEqual(error.exception.code, self.server.grpc.StatusCode.FAILED_PRECONDITION)
        context.auth_context = lambda: {"x509_subject_alternative_name": [b"spiffe://xelab.local/instance/forged"]}
        with self.assertRaises(Abort) as error:
            self.consumer.Describe(self.server.life.DescribeRequest(), context)
        self.assertEqual(error.exception.code, self.server.grpc.StatusCode.PERMISSION_DENIED)
        self.request.input_json = b'{"message":"x","message":"y","skill_id":"forged"}'
        self.request.input_digest = self.server.digest(self.request.input_json)
        with self.assertRaises(Abort) as error:
            self.consumer.Invoke(self.request, Context())
        self.assertEqual(error.exception.code, self.server.grpc.StatusCode.INVALID_ARGUMENT)

    def test_expiry_and_cancellation_stop_before_any_host_call(self):
        self.request.context.deadline_unix_ms = int(time.time() * 1000) - 1
        proxy = self.proxy()
        with self.assertRaises(Abort) as error:
            self.consumer.execute(proxy, self.request, self.value, Context(), self.server.threading.Event())
        self.assertEqual(error.exception.code, self.server.grpc.StatusCode.DEADLINE_EXCEEDED)
        self.assertEqual(proxy.calls, [])
        self.request.context.deadline_unix_ms += 60000
        canceled = self.server.threading.Event()
        canceled.set()
        with self.assertRaises(Abort) as error:
            self.consumer.execute(proxy, self.request, self.value, Context(), canceled)
        self.assertEqual(error.exception.code, self.server.grpc.StatusCode.CANCELLED)

    def test_completed_original_call_replay_and_query_do_not_dispatch_again(self):
        self.consumer.initialized = True
        proxy = self.proxy()
        channel = mock.MagicMock()
        channel.__enter__.return_value = channel
        with mock.patch.object(self.consumer, "open_host", return_value=channel), mock.patch.object(self.server.host_service, "HostServiceStub", return_value=proxy):
            first = self.consumer.Invoke(self.request, Context())
            calls = len(proxy.calls)
            self.assertEqual(self.consumer.Invoke(self.request, Context()), first)
            self.assertEqual(len(proxy.calls), calls)
        known = self.consumer.QueryResult(self.server.cap.QueryResultRequest(call_id=self.request.call_id,
            idempotency_key=self.request.context.idempotency_key), Context())
        self.assertEqual(known.completed, first)
        with self.assertRaises(Abort):
            self.consumer.QueryResult(self.server.cap.QueryResultRequest(call_id=self.request.call_id, idempotency_key="different"), Context())
        unknown = self.consumer.QueryResult(self.server.cap.QueryResultRequest(call_id=str(uuid.uuid4()), idempotency_key="unknown"), Context())
        self.assertEqual(unknown.state, self.server.common.EXECUTION_STATE_UNKNOWN)

    def test_endpoint_never_accepts_input_urls_or_untrusted_scheme(self):
        self.assertEqual(self.server.trusted_endpoint("https://172.29.240.1:7443"), "172.29.240.1:7443")
        for invalid in ["http://127.0.0.1:7443", "https://user@127.0.0.1:7443", "https://127.0.0.1:7443/path", "https://127.0.0.1:7443?url=elsewhere", "https://127.0.0.1:7443#fragment"]:
            with self.assertRaises(ValueError):
                self.server.trusted_endpoint(invalid)


if __name__ == "__main__":
    unittest.main(verbosity=2)
