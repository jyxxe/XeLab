#!/usr/bin/env python3
"""Build immutable M4 consumer inputs. Image signing happens after OCI import."""
import argparse
import hashlib
import importlib.util
import json
import pathlib
import tempfile
import uuid

PLUGIN = "org.xelab.m4-consumer"
TOPIC = PLUGIN + ".completed"


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def contract(root, name, input_schema, output_schema, side_effect):
    checks = []
    directory = None if root is None else root / "xelab" / "contracts" / name / "1.0.0"
    if directory is not None:
        directory.mkdir(parents=True, exist_ok=True)
        for parent in [root / "xelab", root / "xelab" / "contracts", directory.parent, directory]:
            parent.chmod(0o755)
    for filename, value in [("input.schema.json", input_schema), ("output.schema.json", output_schema),
                            ("stream.schema.json", None), ("semantics.json", {"side_effect": side_effect})]:
        raw = encoded(value)
        if directory is not None:
            (directory / filename).write_bytes(raw)
            (directory / filename).chmod(0o644)
        checks.append(filename + " " + digest(raw)[7:] + "\n")
    return {"name": name, "version": "1.0.0", "schema_digest": digest("".join(checks).encode())}


def echo_contract(provider_manifest=None, publisher=None, platform="linux/amd64"):
    if provider_manifest is not None:
        provider = json.loads(pathlib.Path(provider_manifest).read_bytes())
    else:
        # Derive from the existing reference artifact builder, rather than
        # duplicating or guessing its immutable contract package digest.
        source = pathlib.Path(__file__).resolve().parents[1] / "echo" / "build_artifact.py"
        spec = importlib.util.spec_from_file_location("m4_reference_echo_artifact", source)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            provider = module.build(pathlib.Path(directory), publisher, platform=platform)
    matches = [cap["contract"] for cap in provider["capabilities"]
               if cap["contract"]["name"] == "org.xelab.echo" and cap["contract"]["version"] == "1.0.0"]
    if len(matches) != 1:
        raise ValueError("Exactly one fixed reference echo contract is required")
    selected = matches[0]
    digest_value = selected.get("schema_digest")
    if (set(selected) != {"name", "version", "schema_digest"} or not isinstance(digest_value,str)
            or not digest_value.startswith("sha256:") or len(digest_value) != 71
            or any(c not in "0123456789abcdef" for c in digest_value[7:])):
        raise ValueError("Exact immutable provider ContractRef required")
    return selected


def build(root, publisher, platform="linux/amd64", provider_manifest=None):
    root = pathlib.Path(root)
    publisher = str(uuid.UUID(publisher))
    if platform not in ("linux/amd64", "linux/arm64"):
        raise ValueError("Unsupported platform")
    uuid_schema = {"type": "string", "format": "uuid"}
    message = {"type": "string", "maxLength": 1024}
    input_schema = {"type": "object", "properties": {"message": message, "skill_id": uuid_schema,
                    "subscription_id": uuid_schema}, "required": ["message", "skill_id", "subscription_id"],
                    "additionalProperties": False}
    properties = {"message": message, **{name: uuid_schema for name in
                  ["dependency_call_id", "skill_call_id", "event_id", "delivery_event_id", "provider_deployment_id"]},
                  "duplicate": {"type": "boolean"}, "acked": {"type": "boolean"}}
    output_schema = {"type": "object", "properties": properties, "required": list(properties), "additionalProperties": False}
    target = contract(root, PLUGIN + ".run", input_schema, output_schema, "non_repeatable")
    topic_schema = {"type": "object", "properties": {"message": message}, "required": ["message"], "additionalProperties": False}
    # All schema keys are ASCII and values contain no floats. These bytes are
    # the RFC8785 form used by the kernel, without the package file's newline.
    topic_digest = digest(json.dumps(topic_schema, sort_keys=True, separators=(",", ":")).encode())
    rules = [{"required": False, "rule": {"action": action, "resource": {"kind": kind, "match": "all"}}}
             for kind, actions in [("capability", ["capability.discover", "capability.invoke"]),
                                   ("skill", ["skill.read", "skill.invoke"]),
                                   ("event", ["event.publish", "event.subscribe"])] for action in actions]
    manifest = {
        "manifest_version": "1", "plugin_id": PLUGIN, "version": "1.0.0", "publisher_id": publisher,
        "host_api_major": 1, "platforms": [platform], "entrypoint": ["/usr/bin/python3", "/opt/plugin/server.py"],
        "capabilities": [{"contract": target, "input_schema": input_schema, "output_schema": output_schema,
                          "stream_item_schema": None, "side_effect": "non_repeatable",
                          "maximum_input_bytes": 65536, "maximum_output_bytes": 65536}],
        "dependencies": [{"name": "echo", "required": True, "target_kind": "capability", "target": "org.xelab.echo",
                          "version_range": "1.0.0", "binding_scope": "deployment", "failure_policy": "wait", "features": []}],
        "requested_rules": rules, "minimum_resources": {"cpu_millis": 100, "memory_bytes": 134217728,
                            "ephemeral_bytes": 1048576, "pids": 64},
        "health": {"startup_timeout_ms": 60000, "interval_ms": 5000, "timeout_ms": 2000, "failure_threshold": 3},
        "volumes": [], "routes": [], "config_schema": {"type": "object", "additionalProperties": False},
        "data_schema_version": "none", "upgrade_strategy": "recreate", "business_entry": "direct",
        "concurrency": {"max_active_requests": 4, "max_queue_requests": 0, "queue_timeout_ms": 100},
        "event_topics": [{"topic": TOPIC, "version": "1.0.0", "payload_schema": topic_schema, "schema_digest": topic_digest}],
        "skills": [{"name": PLUGIN + ".run", "version": "1.0.0", "description": "Exercise fixed dependency, Skill and durable event HostService paths",
                    "input_schema": input_schema, "output_schema": output_schema, "stream_item_schema": None,
                    "target": {"kind": "capability", "plugin_id": PLUGIN, "contract": target},
                    "mapping": "identity", "maximum_timeout_ms": 60000, "input_blobs": False, "tags": ["m4", "acceptance"]}],
    }
    dependency = echo_contract(provider_manifest, publisher, platform)
    (root / "xelab" / "dependency-contract.json").write_bytes(encoded(dependency))
    (root / "xelab" / "plugin.json").write_bytes(encoded(manifest))
    (root / "xelab" / "dependency-contract.json").chmod(0o644)
    (root / "xelab" / "plugin.json").chmod(0o644)
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publisher-id", required=True)
    parser.add_argument("--output", default="examples/generated-m4-consumer")
    parser.add_argument("--platform", default="linux/amd64", choices=["linux/amd64", "linux/arm64"])
    parser.add_argument("--provider-manifest", help="Existing signed echo manifest used by the integration harness")
    arguments = parser.parse_args()
    build(arguments.output, arguments.publisher_id, arguments.platform, arguments.provider_manifest)
