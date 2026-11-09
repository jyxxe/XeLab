#!/usr/bin/python3
"""Independent bounded M4 OCI fixture; generated protobufs, no kernel imports."""
import concurrent.futures
import hashlib
import json
import os
import pathlib
import threading
import time
import uuid
from urllib.parse import urlsplit

import grpc
from xelab.v1 import common_pb2 as common
from xelab.v1 import lifecycle_pb2 as life
from xelab.v1 import lifecycle_pb2_grpc as lifecycle
from xelab.v1 import capability_pb2 as cap
from xelab.v1 import capability_pb2_grpc as capability
from xelab.v1 import host_pb2 as host
from xelab.v1 import host_pb2_grpc as host_service
from build_artifact import PLUGIN, TOPIC

CALL_LIMIT = 1024
STAGES = frozenset({"host.open", "dependency.resolve", "dependency.resolve.validate",
    "dependency.invoke", "dependency.invoke.validate", "skill.resolve", "skill.resolve.validate",
    "skill.check", "skill.check.validate", "skill.invoke", "skill.invoke.validate",
    "event.publish", "event.publish.duplicate", "event.publish.validate", "event.subscribe",
    "event.delivery.validate", "event.ack", "event.ack.duplicate", "event.ack.validate", "unknown"})
DIAGNOSTIC_CHECKS = frozenset({"binding_contract_equal", "binding_deadline_within_parent",
    "binding_unexpired", "skill_ready", "skill_target_equal", "skill_access_ready",
    "event_duplicate_same", "delivery_event_equal"})
DIAGNOSTIC_KINDS = frozenset({"RPC", "ValueError", "KeyError", "TypeError", "StopIteration", "OSError"})


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def strict_json(raw, maximum=65536):
    if not raw or len(raw) > maximum:
        raise ValueError("JSON size exceeds the signed bound")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("Duplicate JSON member")
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=unique,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError("Nonfinite JSON")))


def canonical_uuid(value):
    if not isinstance(value, str) or str(uuid.UUID(value)) != value or uuid.UUID(value).int == 0:
        raise ValueError("Canonical nonzero UUID required")
    return value


def require_kernel(context):
    names = context.auth_context().get("x509_subject_alternative_name", [])
    if [name for name in names if name.startswith(b"spiffe://")] != [b"spiffe://xelab.local/kernel"]:
        context.abort(grpc.StatusCode.PERMISSION_DENIED, "Kernel identity required")


def trusted_endpoint(value):
    parsed = urlsplit(value)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.port is None
            or parsed.username is not None or parsed.password is not None
            or parsed.path or parsed.query or parsed.fragment or any(c.isspace() for c in value)):
        raise ValueError("Trusted runtime HostService endpoint required")
    return parsed.netloc


class Consumer(lifecycle.PluginLifecycleServicer, capability.CapabilityServiceServicer):
    def __init__(self, manifest_path="/xelab/plugin.json"):
        self.raw = pathlib.Path(manifest_path).read_bytes()
        self.manifest = strict_json(self.raw, 1048576)
        self.contract = common.ContractRef(**self.manifest["capabilities"][0]["contract"])
        self.dependency_value = strict_json(pathlib.Path(manifest_path).with_name("dependency-contract.json").read_bytes(), 4096)
        if (set(self.dependency_value) != {"name", "version", "schema_digest"}
                or self.dependency_value["name"] != "org.xelab.echo" or self.dependency_value["version"] != "1.0.0"
                or not isinstance(self.dependency_value["schema_digest"],str)
                or len(self.dependency_value["schema_digest"]) != 71
                or not self.dependency_value["schema_digest"].startswith("sha256:")
                or any(c not in "0123456789abcdef" for c in self.dependency_value["schema_digest"][7:])):
            raise ValueError("Fixed image dependency contract differs")
        self.dependency = common.ContractRef(**self.dependency_value)
        self.instance = canonical_uuid(os.environ["XELAB_INSTANCE_ID"])
        self.deployment = canonical_uuid(os.environ["XELAB_DEPLOYMENT_ID"])
        self.generation = int(os.environ["XELAB_GENERATION"])
        if self.generation < 1 or self.manifest["plugin_id"] != PLUGIN:
            raise ValueError("Invalid workload generation or manifest")
        self.initialized, self.draining = False, False
        self.lock = threading.Lock()
        self.active = {}
        self.channels = {}
        self.results = {}
        self.failures = {}

    def Describe(self, request, context):
        require_kernel(context)
        return life.DescribeResponse(manifest_json=self.raw, manifest_digest=digest(self.raw))

    def Initialize(self, request, context):
        require_kernel(context)
        try:
            valid = (request.instance_id == self.instance and request.deployment_id == self.deployment
                     and request.generation == self.generation and digest(request.config_json) == request.config_digest
                     and strict_json(request.config_json) == {})
        except (ValueError, UnicodeDecodeError):
            valid = False
        if not valid:
            context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Identity or configuration differs")
        with self.lock:
            self.initialized, self.draining = True, False
        return life.InitializeResponse(observed_generation=self.generation, initialized=True)

    def GetHealth(self, request, context):
        require_kernel(context)
        with self.lock:
            ready = self.initialized and not self.draining
        return life.HealthResponse(alive=True, observed_generation=self.generation,
                                   capabilities=[life.CapabilityHealth(contract=self.contract, ready=ready)])

    def Drain(self, request, context):
        require_kernel(context)
        with self.lock:
            self.draining = True
            active = len(self.active)
        return life.DrainResponse(accepting_requests=False, active_calls=active, drained=active == 0)

    def Shutdown(self, request, context):
        require_kernel(context)
        with self.lock:
            self.draining = True
            for event in self.active.values():
                event.set()
            channels = list(self.channels.values())
        for channel in channels:
            channel.close()
        return common.Ack(request_id=request.context.request_id, accepted=True)

    def open_host(self):
        identity = pathlib.Path("/run/xelab/identity")
        credentials = grpc.ssl_channel_credentials((identity / "ca.crt").read_bytes(),
            (identity / "tls.key").read_bytes(), (identity / "tls.crt").read_bytes())
        endpoint = trusted_endpoint(os.environ["XELAB_HOST_RPC_ENDPOINT"])
        return grpc.secure_channel(endpoint, credentials, options=[
            ("grpc.ssl_target_name_override", "kernel.xelab.local"),
            ("grpc.max_receive_message_length", 1048576), ("grpc.max_send_message_length", 1048576),
            ("grpc.enable_retries", 0)])

    def execute(self, proxy, request, value, context, canceled, stage=None):
        parent = request.context
        def mark(name):
            if stage is not None:
                stage["stage"] = name
        def observed(name, value):
            if stage is not None:
                stage[name] = bool(value)
            return value
        def child(phase):
            return common.RpcContext(request_id=str(uuid.uuid4()), delegation_token=parent.delegation_token,
                deadline_unix_ms=parent.deadline_unix_ms, idempotency_key=request.call_id + ":" + phase,
                subject_id=parent.subject_id, actor_instance_id=self.instance)
        def timeout():
            remaining = min((parent.deadline_unix_ms - int(time.time() * 1000)) / 1000,
                            context.time_remaining() or 60, 10)
            if canceled.is_set() or not context.is_active():
                context.abort(grpc.StatusCode.CANCELLED, "Call canceled")
            if remaining <= 0:
                context.abort(grpc.StatusCode.DEADLINE_EXCEEDED, "Parent call expired")
            return remaining
        payload = encoded({"message": value["message"]})
        mark("dependency.resolve")
        binding = proxy.Resolve(host.ResolveRequest(context=child("resolve"), contract=self.dependency,
                                binding_scope_id=self.deployment), timeout=timeout())
        mark("dependency.resolve.validate")
        canonical_uuid(binding.binding_id)
        provider = canonical_uuid(binding.provider_deployment_id)
        equal = observed("binding_contract_equal", binding.contract == self.dependency)
        clipped = observed("binding_deadline_within_parent", binding.expires_unix_ms <= parent.deadline_unix_ms)
        fresh = observed("binding_unexpired", binding.expires_unix_ms > int(time.time() * 1000))
        if not equal or not clipped or not fresh:
            raise ValueError("Fixed dependency response differs")
        child_id = str(uuid.uuid4())
        mark("dependency.invoke")
        dependency_result = proxy.Invoke(cap.InvokeRequest(context=child("dependency"), call_id=child_id,
            contract=self.dependency, input_json=payload, input_digest=digest(payload), binding_id=binding.binding_id), timeout=timeout())
        mark("dependency.invoke.validate")
        if dependency_result.call_id != child_id:
            raise ValueError("Dependency Call ID differs")
        def business(result):
            canonical_uuid(result.call_id)
            if digest(result.output_json) != result.output_digest or strict_json(result.output_json) != {"message": value["message"]}:
                raise ValueError("Downstream output differs")
        business(dependency_result)
        mark("skill.resolve")
        resolved = proxy.ResolveSkill(host.ResolveSkillRequest(context=child("resolve-skill"), skill_id=value["skill_id"]), timeout=timeout())
        mark("skill.resolve.validate")
        definition = strict_json(resolved.definition_json, 1048576)
        observed("skill_ready", resolved.ready)
        target = definition.get("target", {}) if isinstance(definition, dict) else {}
        observed("skill_target_equal", isinstance(target, dict) and target.get("contract") == self.dependency_value)
        if (resolved.skill_id != value["skill_id"] or not resolved.ready
                or definition["target"]["kind"] != "capability"
                or definition["target"]["plugin_id"] not in ("org.xelab.echo", "org.xelab.echo-python")
                or definition["target"]["contract"] != self.dependency_value):
            raise ValueError("Fixed Skill target is not the approved echo contract")
        mark("skill.check")
        access = proxy.CheckSkillAccess(host.CheckSkillAccessRequest(context=child("check-skill"),
            skill_id=value["skill_id"], expected_definition_digest=resolved.definition_digest), timeout=timeout())
        mark("skill.check.validate")
        observed("skill_access_ready", access.ready)
        if not access.ready or access.skill_id != value["skill_id"] or access.definition_digest != resolved.definition_digest:
            raise ValueError("Fixed Skill access changed")
        mark("skill.invoke")
        skill_result = proxy.InvokeSkill(host.SkillInvokeRequest(context=child("skill"), skill_id=value["skill_id"],
                                        input_json=payload, input_digest=digest(payload)), timeout=timeout())
        mark("skill.invoke.validate")
        business(skill_result)
        mark("event.publish")
        first = proxy.PublishEvent(host.PublishEventRequest(context=child("publish"), topic=TOPIC, version="1.0.0",
            payload_json=payload, dedupe_key=request.call_id), timeout=timeout())
        mark("event.publish.duplicate")
        second = proxy.PublishEvent(host.PublishEventRequest(context=child("publish-duplicate"), topic=TOPIC, version="1.0.0",
            payload_json=payload, dedupe_key=request.call_id), timeout=timeout())
        mark("event.publish.validate")
        canonical_uuid(first.event_id)
        observed("event_duplicate_same", first.event_id == second.event_id and second.duplicate)
        if first.event_id != second.event_id or not second.duplicate:
            raise ValueError("Event dedupe result differs")
        mark("event.subscribe")
        deliveries = proxy.Subscribe(host.SubscribeRequest(context=child("subscribe"), subscription_id=value["subscription_id"]), timeout=timeout())
        try:
            delivery = next(deliveries)
            mark("event.delivery.validate")
            observed("delivery_event_equal", delivery.event_id == first.event_id)
            if (delivery.event_id != first.event_id or delivery.topic != TOPIC or delivery.version != "1.0.0"
                    or strict_json(delivery.payload_json) != {"message": value["message"]}
                    or not 1 <= delivery.attempt <= 10 or not delivery.lease_token
                    or delivery.lease_expires_unix_ms <= int(time.time() * 1000)):
                raise ValueError("Delivery is not the just-published event")
            canonical_uuid(delivery.delivery_id)
            for phase in ("ack", "ack-duplicate"):
                mark("event.ack" if phase == "ack" else "event.ack.duplicate")
                ack = proxy.AckEvent(host.AckEventRequest(context=child(phase), subscription_id=value["subscription_id"],
                    delivery_id=delivery.delivery_id, lease_token=delivery.lease_token, outcome=host.AckEventRequest.OUTCOME_ACK), timeout=timeout())
                mark("event.ack.validate")
                if not ack.accepted:
                    raise ValueError("Acknowledgment was not accepted")
        finally:
            deliveries.cancel()
        return {"message": value["message"], "dependency_call_id": dependency_result.call_id,
                "skill_call_id": skill_result.call_id, "event_id": first.event_id, "duplicate": second.duplicate,
                "delivery_event_id": delivery.event_id, "acked": True, "provider_deployment_id": provider}

    def remember_failure(self, request, stage, kind, status, response_code):
        # Only fixed labels and booleans leave this bounded diagnostic ledger.
        # The business outcome remains UNKNOWN: a child effect may have committed.
        diagnostic = {"version": 1, "code": "M4_FIXTURE_DIAGNOSTIC",
                      "stage": stage.get("stage") if stage.get("stage") in STAGES else "unknown",
                      "kind": kind if kind in DIAGNOSTIC_KINDS else "ValueError",
                      "status": status if status in {code.name for code in grpc.StatusCode} else "UNKNOWN"}
        diagnostic.update({name: stage[name] for name in DIAGNOSTIC_CHECKS
                           if isinstance(stage.get(name), bool)})
        raw = encoded(diagnostic).decode()
        if len(raw.encode()) > 1024:
            raw = encoded({"version": 1, "code": "M4_FIXTURE_DIAGNOSTIC", "stage": "unknown",
                           "kind": "ValueError", "status": "UNKNOWN"}).decode()
        with self.lock:
            self.failures[request.call_id] = (request.input_digest, request.context.idempotency_key,
                                               raw, response_code)
        return diagnostic["stage"]

    def Invoke(self, request, context):
        require_kernel(context)
        try:
            canonical_uuid(request.call_id)
            if (request.contract != self.contract or digest(request.input_json) != request.input_digest
                    or not request.context.delegation_token or request.context.deadline_unix_ms <= int(time.time() * 1000)):
                raise ValueError("Invalid invocation envelope")
            value = strict_json(request.input_json)
            if (not isinstance(value, dict) or set(value) != {"message", "skill_id", "subscription_id"}
                    or not isinstance(value["message"], str) or len(value["message"]) > 1024):
                raise ValueError("Invalid consumer input")
            canonical_uuid(value["skill_id"])
            canonical_uuid(value["subscription_id"])
        except (ValueError, UnicodeDecodeError, TypeError):
            context.abort(grpc.StatusCode.INVALID_ARGUMENT, "Input does not match the fixed contract")
        with self.lock:
            if not self.initialized or self.draining:
                context.abort(grpc.StatusCode.UNAVAILABLE, "Not accepting calls")
            previous = self.results.get(request.call_id)
            if previous is not None:
                if previous[0] != request.input_digest or previous[1] != request.context.idempotency_key:
                    context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Call fingerprint differs")
                return previous[2]
            failed = self.failures.get(request.call_id)
            if failed is not None:
                if failed[0] != request.input_digest or failed[1] != request.context.idempotency_key:
                    context.abort(grpc.StatusCode.FAILED_PRECONDITION, "Call fingerprint differs")
                context.abort(failed[3], "Prior composite outcome is unknown; inspect the original Call")
            if request.call_id in self.active or len(self.results) + len(self.failures) + len(self.active) >= CALL_LIMIT:
                context.abort(grpc.StatusCode.RESOURCE_EXHAUSTED, "Bounded call ledger is full or busy")
            canceled = threading.Event()
            self.active[request.call_id] = canceled
        stage = {"stage": "host.open"}
        try:
            with self.open_host() as channel:
                with self.lock:
                    self.channels[request.call_id] = channel
                context.add_callback(channel.close)
                output = encoded(self.execute(host_service.HostServiceStub(channel), request, value, context, canceled, stage))
            if len(output) > 65536:
                raise ValueError("Output exceeds signed bound")
            response = cap.InvokeResponse(call_id=request.call_id, output_json=output, output_digest=digest(output))
            with self.lock:
                self.results[request.call_id] = (request.input_digest, request.context.idempotency_key, response)
            return response
        except grpc.RpcError as error:
            code = error.code()
            status = code.name if isinstance(code, grpc.StatusCode) else "UNKNOWN"
            phase = self.remember_failure(request, stage, "RPC", status, grpc.StatusCode.UNAVAILABLE)
            context.abort(grpc.StatusCode.UNAVAILABLE, "HostService request failed at " + phase + "; outcome requires verification")
        except (ValueError, KeyError, TypeError, StopIteration, OSError) as error:
            kind = next(name for kind_type, name in ((ValueError, "ValueError"), (KeyError, "KeyError"),
                (TypeError, "TypeError"), (StopIteration, "StopIteration"), (OSError, "OSError"))
                if isinstance(error, kind_type))
            phase = self.remember_failure(request, stage, kind, "FAILED_PRECONDITION",
                                          grpc.StatusCode.FAILED_PRECONDITION)
            context.abort(grpc.StatusCode.FAILED_PRECONDITION, "HostService response violates the fixed contract at " + phase)
        finally:
            with self.lock:
                self.active.pop(request.call_id, None)
                self.channels.pop(request.call_id, None)

    def InvokeStream(self, request, context):
        require_kernel(context)
        context.abort(grpc.StatusCode.UNIMPLEMENTED, "No streaming consumer contract")

    def Cancel(self, request, context):
        require_kernel(context)
        with self.lock:
            event = self.active.get(request.call_id)
            if event is not None:
                event.set()
            channel = self.channels.get(request.call_id)
        if channel is not None:
            channel.close()
        return common.Ack(request_id=request.context.request_id, accepted=True)

    def QueryResult(self, request, context):
        require_kernel(context)
        with self.lock:
            value = self.results.get(request.call_id)
            failure = self.failures.get(request.call_id)
        if value is None:
            if failure is not None:
                if failure[1] != request.idempotency_key:
                    context.abort(grpc.StatusCode.PERMISSION_DENIED, "Original business key required")
                # UNKNOWN has no outcome under the existing wire contract.
                # This signed reference fixture exposes only bounded diagnostics
                # in transport metadata, after the original-key identity gate.
                context.send_initial_metadata((("xelab-m4-fixture-diag-bin", failure[2].encode()),))
            return cap.QueryResultResponse(state=common.EXECUTION_STATE_UNKNOWN)
        if value[1] != request.idempotency_key:
            context.abort(grpc.StatusCode.PERMISSION_DENIED, "Original business key required")
        return cap.QueryResultResponse(state=common.EXECUTION_STATE_SUCCEEDED, completed=value[2])


def main():
    consumer = Consumer()
    cert_path = pathlib.Path(os.environ["XELAB_TLS_CERT_PATH"])
    key = pathlib.Path(os.environ["XELAB_TLS_KEY_PATH"]).read_bytes()
    ca = pathlib.Path(os.environ["XELAB_TLS_CA_PATH"]).read_bytes()
    previous = [None]
    def certificates():
        cert = cert_path.read_bytes()
        if cert == previous[0]:
            return None
        value = grpc.ssl_server_certificate_configuration(((key, cert),), root_certificates=ca)
        previous[0] = cert
        return value
    credentials = grpc.dynamic_ssl_server_credentials(certificates(), certificates, require_client_authentication=True)
    server = grpc.server(concurrent.futures.ThreadPoolExecutor(max_workers=8), options=[
        ("grpc.max_receive_message_length", 1048576), ("grpc.max_send_message_length", 1048576)])
    lifecycle.add_PluginLifecycleServicer_to_server(consumer, server)
    capability.add_CapabilityServiceServicer_to_server(consumer, server)
    if not server.add_secure_port("0.0.0.0:7000", credentials):
        raise RuntimeError("Cannot bind plugin listener")
    server.start()
    server.wait_for_termination()


if __name__ == "__main__":
    main()
