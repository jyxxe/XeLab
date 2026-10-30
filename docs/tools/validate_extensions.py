"""Read-only extension specification checks; no plugin, network or runtime execution.

The caller supplies the shared duplicate-key-aware JSON reader and error sink.
Strict mode uses local jsonschema/referencing resources and published fixtures.
"""
from pathlib import Path
import hashlib
import json
from urllib.parse import unquote, urlparse


VERSION = "2.8.0"
MODELS = {
    "ExtensionPoint", "ExtensionHostDescriptor", "OrdinaryExtensionHostRef",
    "MicroExtensionHostRef", "NestedExtensionHostRef", "ExtensionHostRef",
    "ExtensionManifest", "ExtensionPlugin", "CreateExtensionPlugin",
    "ExtensionPluginPatch", "ExtensionPluginVersion", "PublishExtensionVersion",
    "ExtensionDeployment", "CreateExtensionDeployment", "ExtensionPluginPage",
    "ExtensionVersionPage", "ExtensionDeploymentPage", "ExtensionSkillTarget",
}
OPERATIONS = {
    "listExtensionPlugins": ("get", "/api/platform/v1/extension-plugins", "extension.read"),
    "createExtensionPlugin": ("post", "/api/platform/v1/extension-plugins", "extension.create"),
    "getExtensionPlugin": ("get", "/api/platform/v1/extension-plugins/{extension_plugin_id}", "extension.read"),
    "updateExtensionPlugin": ("patch", "/api/platform/v1/extension-plugins/{extension_plugin_id}", "extension.update"),
    "archiveExtensionPlugin": ("delete", "/api/platform/v1/extension-plugins/{extension_plugin_id}", "extension.archive"),
    "listExtensionVersions": ("get", "/api/platform/v1/extension-plugins/{extension_plugin_id}/versions", "extension.read"),
    "publishExtensionVersion": ("post", "/api/platform/v1/extension-plugins/{extension_plugin_id}/versions", "extension.publish"),
    "getExtensionVersion": ("get", "/api/platform/v1/extension-plugins/{extension_plugin_id}/versions/{extension_version_id}", "extension.read"),
    "getExtensionSource": ("get", "/api/platform/v1/extension-plugins/{extension_plugin_id}/versions/{extension_version_id}/source", "extension.source.read"),
    "listExtensionDeployments": ("get", "/api/platform/v1/extension-deployments", "extension.read"),
    "createExtensionDeployment": ("post", "/api/platform/v1/extension-deployments", "extension.deploy"),
    "getExtensionDeployment": ("get", "/api/platform/v1/extension-deployments/{extension_deployment_id}", "extension.read"),
    "archiveExtensionDeployment": ("delete", "/api/platform/v1/extension-deployments/{extension_deployment_id}", "extension.deploy"),
    "setExtensionDeploymentEnabled": ("put", "/api/platform/v1/extension-deployments/{extension_deployment_id}/enabled", "extension.enable"),
}
OLD_REQUIRED = {
    "Manifest": {"manifest_version", "plugin_id", "version", "publisher_id", "host_api_major", "platforms", "entrypoint", "capabilities", "dependencies", "requested_rules", "minimum_resources", "health", "volumes", "routes", "config_schema", "data_schema_version", "upgrade_strategy"},
    "MicroManifest": {"format_version", "version", "description", "engine", "protocol", "entrypoint", "input_schema", "output_schema", "limits", "side_effect"},
}


def _canonical(value):
    """JCS for these safe-integer fixtures, including UTF-16 property ordering.

    Floats are deliberately rejected rather than pretending Python's float
    formatting implements ECMAScript's complete JCS number serialization.
    """
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        value.encode("utf-8", errors="strict")
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    if isinstance(value, int):
        if abs(value) > 9007199254740991:
            raise ValueError("fixture integer exceeds the I-JSON exact range")
        return str(value)
    if isinstance(value, list):
        return "[" + ",".join(_canonical(x) for x in value) + "]"
    if isinstance(value, dict) and all(isinstance(k, str) for k in value):
        keys = sorted(value, key=lambda k: k.encode("utf-16-be", errors="strict"))
        return "{" + ",".join(_canonical(k) + ":" + _canonical(value[k]) for k in keys) + "}"
    raise ValueError("JCS fixture inputs must use JSON values with safe integer numbers")


def _digest(value):
    return "sha256:" + hashlib.sha256(_canonical(value).encode("utf-8")).hexdigest()


def _strict_json(raw):
    def pairs(values):
        result = {}
        for key, value in values:
            if key in result:
                raise ValueError("duplicate JSON key")
            result[key] = value
        return result

    def constant(value):
        raise ValueError("nonfinite JSON number: " + value)

    if not 1 <= len(raw) <= 262144 or raw.startswith(b"\xef\xbb\xbf") or b"\x00" in raw:
        raise ValueError("contribution must be 1..256KiB UTF-8 without BOM/NUL")
    value = json.loads(raw.decode("utf-8", errors="strict"), object_pairs_hook=pairs, parse_constant=constant)
    pending = [(value, 1)]
    while pending:
        node, depth = pending.pop()
        if depth > 32:
            raise ValueError("contribution JSON exceeds depth 32")
        if isinstance(node, dict):
            pending.extend((v, depth + 1) for v in node.values())
        elif isinstance(node, list):
            pending.extend((v, depth + 1) for v in node)
    return value


def _valid_graph(nodes, edges):
    """Check merged dependency DAG; only consecutive host edges count to 16."""
    kinds = {"ordinary", "micro", "extension"}
    edge_kinds = {"required", "micro_runtime", "extension_host", "handler"}
    if not isinstance(nodes, dict) or not isinstance(edges, list) or any(not isinstance(k, str) or not isinstance(v, str) or v not in kinds for k, v in nodes.items()):
        return False
    adjacency = {node: set() for node in nodes}
    hosts = {node: set() for node in nodes}
    indegree = {node: 0 for node in nodes}
    seen = set()
    for edge in edges:
        if not isinstance(edge, dict) or set(edge) != {"from", "to", "kind"}:
            return False
        source, target, kind = edge["from"], edge["to"], edge["kind"]
        if not all(isinstance(value, str) for value in (source, target, kind)):
            return False
        if source not in nodes or target not in nodes or kind not in edge_kinds:
            return False
        identity = (source, target, kind)
        if identity in seen:
            return False
        seen.add(identity)
        if kind == "extension_host":
            if nodes[source] != "extension":
                return False
            hosts[source].add(target)
            if len(hosts[source]) > 1:
                return False
        if kind == "micro_runtime" and (nodes[source] != "micro" or nodes[target] != "ordinary"):
            return False
        if target not in adjacency[source]:
            adjacency[source].add(target)
            indegree[target] += 1
    ready = [node for node, degree in indegree.items() if degree == 0]
    ordered = []
    while ready:
        source = ready.pop()
        ordered.append(source)
        for target in adjacency[source]:
            indegree[target] -= 1
            if indegree[target] == 0:
                ready.append(target)
    if len(ordered) != len(nodes):
        return False
    depth = {node: 0 for node in nodes}
    for node in reversed(ordered):
        depth[node] = max((1 + depth[target] for target in hosts[node]), default=0)
    return max(depth.values(), default=0) <= 16


def validate_extensions(DOCS, check, read, strict=False):
    """Validate declarations and fixtures only; report through check, never write."""
    docs = Path(DOCS).resolve()
    examples = docs / "extensions/examples"

    def report(ok, message):
        check(bool(ok), "extensions: " + message)

    def load(file):
        try:
            return read(file)
        except Exception as exc:
            report(False, "cannot read " + str(file.relative_to(docs)) + ": " + str(exc))
            return None

    def local_file(folder, value):
        if not isinstance(value, str) or not value:
            raise ValueError("fixture file path must be a nonempty relative string")
        parsed = urlparse(value)
        file = (folder / value).resolve()
        if parsed.scheme or parsed.netloc or parsed.query or parsed.fragment or not file.is_relative_to(folder.resolve()):
            raise ValueError("fixture file must remain inside its example directory")
        if not file.is_file():
            raise ValueError("missing fixture file " + str(file.relative_to(docs)))
        return file

    spec = load(docs / "contracts/openapi.json")
    permission = load(docs / "contracts/permissions.json")
    internal = load(docs / "contracts/internal.schema.json")
    if not all(isinstance(x, dict) for x in (spec, permission, internal)):
        return {"operations": 0, "examples": 0, "fixtures": 0, "graphs": 0}
    schemas = spec.get("components", {}).get("schemas", {})
    report(spec.get("info", {}).get("version") == VERSION, "specification version differs")
    report(MODELS <= set(schemas), "core extension models missing: " + ", ".join(sorted(MODELS - set(schemas))))
    actual = {}
    for path, methods in spec.get("paths", {}).items():
        for method, operation in methods.items():
            name = operation.get("operationId")
            if name in OPERATIONS:
                report(name not in actual, "duplicate extension operation " + name)
                actual[name] = (method, path, operation.get("x-permission"))
    report(actual == OPERATIONS, "14 core management operations/routes/permissions differ")
    actions = permission.get("definitions", {})
    for name in {x[2] for x in OPERATIONS.values()} | {"extension.invoke", "extension.host.use"}:
        action = actions.get(name, {})
        expected_kinds = {"deployment", "micro_plugin", "extension_plugin"} if name == "extension.host.use" else {"extension_plugin"}
        report(set(action.get("resource_kinds", [])) == expected_kinds, "action resource types differ: " + name)
        for operation, (_, _, action_name) in OPERATIONS.items():
            if action_name == name:
                report(operation in action.get("operations", []), "operation absent from action " + operation)
    report("extension_plugin" in schemas.get("ResourceSelector", {}).get("properties", {}).get("kind", {}).get("enum", []), "extension_plugin resource kind absent")
    expected_targets = {"CapabilitySkillTarget": "capability", "HttpSkillTarget": "http", "MicroSkillTarget": "micro", "ExtensionSkillTarget": "extension"}
    refs = {x.get("$ref", "").split("/")[-1] for x in schemas.get("SkillTarget", {}).get("oneOf", [])}
    report(refs == set(expected_targets), "SkillTarget must have four fixed disjoint branches")
    for model, kind in expected_targets.items():
        schema = schemas.get(model, {})
        report(schema.get("additionalProperties") is False and schema.get("properties", {}).get("kind", {}).get("const") == kind, "open or mistyped Skill target " + model)
    fixed_fields = {
        "CapabilitySkillTarget": {"kind", "plugin_id", "contract"},
        "HttpSkillTarget": {"kind", "plugin_id", "http_operation_id", "http_contract_digest"},
        "MicroSkillTarget": {"kind", "plugin_id", "micro_version_id", "micro_deployment_id", "package_digest"},
        "ExtensionSkillTarget": {"kind", "plugin_id", "extension_version_id", "extension_deployment_id", "package_digest", "host_chain_digest"},
    }
    for model, fields in fixed_fields.items():
        report(set(schemas.get(model, {}).get("required", [])) == fields and set(schemas.get(model, {}).get("properties", {})) == fields, "mixed or unfixed Skill target fields: " + model)
    report(set(schemas.get("ExtensionSkillTarget", {}).get("required", [])) == {"kind", "plugin_id", "extension_version_id", "extension_deployment_id", "package_digest", "host_chain_digest"}, "extension Skill target must fix version/deployment/package/host chain")
    for model, required in OLD_REQUIRED.items():
        schema = schemas.get(model, {})
        report(set(schema.get("required", [])) == required, "old required manifest fields changed: " + model)
        report(schema.get("properties", {}).get("extension_points", {}).get("items", {}).get("$ref") == "#/components/schemas/ExtensionPoint", "optional extension points missing: " + model)
    report("extension_host" in schemas.get("Manifest", {}).get("properties", {}) and "extension_host" not in schemas.get("Manifest", {}).get("required", []), "ordinary host support must be optional")
    host_models = {"OrdinaryExtensionHostRef": "ordinary", "MicroExtensionHostRef": "micro", "NestedExtensionHostRef": "extension"}
    report({x.get("$ref", "").split("/")[-1] for x in schemas.get("ExtensionHostRef", {}).get("oneOf", [])} == set(host_models), "typed host must have three disjoint branches")
    for model, kind in host_models.items():
        schema = schemas.get(model, {})
        report(schema.get("additionalProperties") is False and schema.get("properties", {}).get("kind", {}).get("const") == kind, "open or mistyped host branch " + model)
    mpr = load(docs / "preset-plugins/micro-plugin-runtime/manifest.json")
    if isinstance(mpr, dict):
        descriptor = mpr.get("extension_host", {})
        report(mpr.get("version") == "1.3.0", "MPR extension host version must be 1.3.0")
        report({"micro", "extension"} <= set(descriptor.get("host_kinds", [])) and {"data", "skill"} <= set(descriptor.get("modes", [])), "MPR must explicitly support micro/extension data/skill hosting")
    catalog = load(docs / "preset-plugins/catalog.json")
    planned = load(docs / "preset-plugins/planned-catalog.json")
    if isinstance(catalog, dict):
        report(len(catalog.get("presets", [])) == 7 and len(catalog.get("micro_presets", [])) == 1, "extension fixtures must not enter OCI/micro preset lists")
        report(not catalog.get("extension_presets"), "extension examples are not installable presets")
    if isinstance(planned, dict):
        report(len(planned.get("entries", [])) == 32 and all(e.get("kind") in {"ordinary", "micro"} for e in planned.get("entries", [])), "32 planned preset entries must remain separate from extension examples")

    # Point packages use the same four-file list hash as ordinary capability contracts.
    point_packages = {}
    point_fields = ["point_id", "mode", "side_effect", "maximum_timeout_ms", "maximum_input_bytes", "maximum_output_bytes", "maximum_contributions", "contribution_schema", "config_schema"]
    contract_files = ["input.schema.json", "output.schema.json", "stream.schema.json", "semantics.json"]
    for folder in sorted((examples / "point-contracts").glob("*")):
        if not folder.is_dir():
            continue
        try:
            files = {name: local_file(folder, name) for name in contract_files}
            parts = "".join(name + " " + hashlib.sha256(files[name].read_bytes()).hexdigest() + "\n" for name in contract_files)
            semantics = read(files["semantics.json"])
            identity = (semantics.get("point_id"), semantics.get("version"))
            report(identity not in point_packages, "duplicate point contract identity: " + str(identity))
            point_packages[identity] = {"digest": "sha256:" + hashlib.sha256(parts.encode()).hexdigest(), "semantics": semantics, "input": read(files["input.schema.json"]), "output": read(files["output.schema.json"])}
            stream = read(files["stream.schema.json"])
            report(stream.get("type") == "null" and set(stream) <= {"$schema", "type"}, "extension point stream must be disabled: " + folder.name)
        except Exception as exc:
            report(False, "point contract failed " + folder.name + ": " + str(exc))
    report(bool(point_packages), "point-contracts must contain actual four-file packages")
    used_points = set()

    def check_points(manifest, label):
        for point in manifest.get("extension_points", []):
            contract = point.get("contract", {})
            identity = (point.get("point_id"), contract.get("version"))
            package = point_packages.get(identity)
            report(package is not None, "missing point contract package: " + label + "/" + str(identity))
            if package:
                used_points.add(identity)
                report(contract.get("schema_digest") == package["digest"], "four-file point digest differs: " + label + "/" + str(identity))
                report(all(package["semantics"].get(key) == point.get(key) for key in point_fields), "point semantics/limits/schemas differ: " + label + "/" + str(identity))
                report(package["input"] == point.get("input_schema") and package["output"] == point.get("output_schema"), "point input/output raw schemas differ: " + label + "/" + str(identity))

    validate_case = None
    if strict:
        try:
            from jsonschema import Draft202012Validator, FormatChecker
            from referencing import Registry, Resource
            from referencing.jsonschema import DRAFT202012

            def retrieve(uri):
                parsed = urlparse(uri)
                file = Path(unquote(parsed.path)).resolve()
                if parsed.scheme != "file" or parsed.netloc not in {"", "localhost"} or not file.is_relative_to(docs):
                    raise ValueError("only local docs schema resources may be retrieved")
                return Resource.from_contents(read(file), default_specification=DRAFT202012)

            registry = Registry(retrieve=retrieve)
            checker = FormatChecker()

            def validate_case(model, value, label, expected=True, internal_type=False):
                try:
                    document = docs / ("contracts/internal.schema.json" if internal_type else "contracts/openapi.json")
                    fragment = "#/$defs/" if internal_type else "#/components/schemas/"
                    validator = Draft202012Validator({"$ref": document.as_uri() + fragment + model}, registry=registry, format_checker=checker)
                    accepted = validator.is_valid(value)
                    report(accepted == expected, "schema fixture mismatch: " + label)
                except Exception as exc:
                    report(False, "schema fixture failed " + label + ": " + str(exc))
        except ImportError as exc:
            report(False, "strict dependencies missing: " + str(exc))

    index = load(examples / "index.json")
    report(isinstance(index, list), "examples index must be an array")
    entries = index if isinstance(index, list) else []
    report({e.get("directory") for e in entries if isinstance(e, dict)} == {"ordinary", "micro", "nested"}, "examples must cover ordinary, micro and nested hosts")
    example_count = 0
    for entry in entries:
        label = str(entry.get("directory")) if isinstance(entry, dict) else "invalid"
        try:
            if not isinstance(entry, dict) or label not in {"ordinary", "micro", "nested"}:
                raise ValueError("unknown example directory")
            folder = examples / label
            files = {key: local_file(folder, entry.get(key)) for key in ["manifest", "contribution", "artifact", "create", "skill", "closure"]}
            manifest, artifact, create, skill, closure = (read(files[key]) for key in ["manifest", "artifact", "create", "skill", "closure"])
            check_points(manifest, label + "/extension-manifest")
            raw = files["contribution"].read_bytes()
            contribution = _strict_json(raw)
            source_digest = "sha256:" + hashlib.sha256(raw).hexdigest()
            package_digest = _digest({"manifest": manifest, "source_digest": source_digest})
            report(artifact.get("source_digest") == source_digest and artifact.get("package_digest") == package_digest, label + " actual source/package digest differs")
            report(artifact.get("source_file") == entry["contribution"] and artifact.get("manifest_file") == entry["manifest"], label + " artifact file references differ")
            target = skill.get("target", {})
            report(target.get("kind") == "extension" and target.get("package_digest") == package_digest == closure.get("package_digest"), label + " candidate package provenance differs")
            report(target.get("extension_version_id") == create.get("extension_version_id") == closure.get("extension_version_id"), label + " fixed extension version differs")
            report(target.get("host_chain_digest") == _digest(closure), label + " JCS host-chain digest differs")
            report(manifest.get("host_kind") == create.get("host", {}).get("kind"), label + " manifest/typed host kind differs")
            hosts = closure.get("hosts", [])
            report(isinstance(hosts, list) and 1 <= len(hosts) <= 16, label + " closure host chain length differs")
            if hosts:
                report(hosts[0].get("host") == create.get("host") and hosts[0].get("point_id") == manifest.get("point_id") and hosts[0].get("contract") == manifest.get("contract"), label + " first host edge differs")
                report(hosts[0].get("config_digest") == _digest(create.get("config")), label + " fixed config digest differs")
                report(all(h.get("host", {}).get("kind") == "extension" for h in hosts[:-1]) and hosts[-1].get("host", {}).get("kind") in {"ordinary", "micro"}, label + " closure must terminate at its first non-extension host")
                ids = [target.get("extension_deployment_id")] + [h["host"].get("extension_deployment_id") for h in hosts if h.get("host", {}).get("kind") == "extension"]
                report(len(ids) == len(set(ids)), label + " repeated extension deployment in host chain")
                last = hosts[-1].get("host", {})
                if last.get("kind") == "ordinary":
                    report((closure.get("root_installation_id"), closure.get("root_installation_digest"), closure.get("root_deployment_id")) == (last.get("installation_id"), last.get("installation_digest"), last.get("deployment_id")), label + " ordinary physical root differs")
                elif last.get("kind") == "micro":
                    report(target.get("plugin_id") == mpr.get("plugin_id"), label + " micro root must use actual MPR plugin")
            handler = closure.get("handler")
            if manifest.get("mode") == "data":
                report(handler is None, label + " data mode cannot contain a handler")
            else:
                report(isinstance(handler, dict) and handler.get("skill_id") == manifest.get("handler_skill_id") and handler.get("definition_digest") == manifest.get("handler_definition_digest"), label + " fixed handler provenance differs")
            if validate_case:
                for key, model in [("manifest", "ExtensionManifest"), ("create", "CreateExtensionDeployment"), ("skill", "SkillDefinition")]:
                    validate_case(model, read(files[key]), label + "/" + key)
                validate_case("ExtensionIdentityClosure", closure, label + "/closure", internal_type=True)
            if entry.get("host_manifest") is not None:
                host_manifest = read(local_file(folder, entry["host_manifest"]))
                check_points(host_manifest, label + "/host-manifest")
                host_schema = entry.get("host_schema", {"ordinary": "Manifest", "micro": "MicroManifest", "nested": "ExtensionManifest"}[label])
                report(host_schema in {"Manifest", "MicroManifest", "ExtensionManifest"}, label + " unknown host manifest schema")
                if validate_case:
                    validate_case(host_schema, host_manifest, label + "/host_manifest")
                if host_schema == "MicroManifest":
                    host_artifact = read(local_file(folder, "host-artifact.json"))
                    host_source = local_file(folder, host_artifact.get("source_file"))
                    host_raw = host_source.read_bytes()
                    host_raw.decode("utf-8", errors="strict")
                    report(1 <= len(host_raw) <= 262144 and not host_raw.startswith(b"\xef\xbb\xbf") and b"\x00" not in host_raw, label + " micro host source bytes invalid")
                    host_source_digest = "sha256:" + hashlib.sha256(host_raw).hexdigest()
                    host_package_digest = _digest({"manifest": host_manifest, "source_digest": host_source_digest})
                    report(host_artifact.get("manifest_file") == entry["host_manifest"] and host_artifact.get("source_digest") == host_source_digest, label + " actual new micro host source digest differs")
                    report(host_artifact.get("package_digest") == host_package_digest == create.get("host", {}).get("package_digest"), label + " actual new micro host package digest differs")
                point = next((p for p in host_manifest.get("extension_points", []) if p.get("point_id") == manifest.get("point_id")), None)
                report(point is not None, label + " declared host extension point missing")
                if point:
                    report(point.get("contract") == manifest.get("contract") and point.get("mode") == manifest.get("mode"), label + " exact point contract/mode differs")
                    report(skill.get("input_schema") == point.get("input_schema") and skill.get("output_schema") == point.get("output_schema"), label + " candidate business schemas must come from host point")
                    timeout = create.get("timeout_ms", schemas.get("CreateExtensionDeployment", {}).get("properties", {}).get("timeout_ms", {}).get("default", 2000))
                    report(skill.get("maximum_timeout_ms") == timeout <= point.get("maximum_timeout_ms", 0), label + " candidate/point timeout differs")
                    if host_schema == "Manifest":
                        report(target.get("plugin_id") == host_manifest.get("plugin_id"), label + " physical root plugin identity differs")
                    if strict and validate_case:
                        for schema_key, value in [("contribution_schema", contribution), ("config_schema", create.get("config"))]:
                            try:
                                Draft202012Validator(point[schema_key], registry=registry, format_checker=checker).validate(value)
                            except Exception as exc:
                                report(False, label + " point " + schema_key + " validation failed: " + str(exc))
            example_count += 1
        except Exception as exc:
            report(False, "example failed " + label + ": " + str(exc))
    report(used_points == set(point_packages), "point packages must be linked by declared example extension points")

    fixture_count = 0
    if strict and validate_case:
        fixtures = load(examples / "validation.json")
        report(isinstance(fixtures, list) and bool(fixtures), "schema validation fixtures must be a nonempty array")
        names = set()
        for item in fixtures if isinstance(fixtures, list) else []:
            if not isinstance(item, dict):
                report(False, "schema validation fixture must be an object")
                continue
            name, model, valid = item.get("name"), item.get("schema"), item.get("valid")
            report(isinstance(name, str) and name not in names, "missing/duplicate schema fixture name")
            names.add(name)
            report(isinstance(valid, bool), "schema fixture valid flag must be boolean: " + str(name))
            in_internal = model in internal.get("$defs", {})
            report(model in schemas or in_internal, "unknown schema fixture model: " + str(model))
            validate_case(model, item.get("value"), str(name), valid, internal_type=in_internal)
            fixture_count += 1
    graphs = load(examples / "graph-validation.json")
    report(isinstance(graphs, list) and bool(graphs), "graph fixtures must be a nonempty array")
    graph_count = 0
    names = set()
    for item in graphs if isinstance(graphs, list) else []:
        if not isinstance(item, dict):
            report(False, "graph fixture must be an object")
            continue
        name, expected = item.get("name"), item.get("valid")
        report(isinstance(name, str) and name not in names, "missing/duplicate graph fixture name")
        names.add(name)
        report(isinstance(expected, bool), "graph fixture valid flag must be boolean")
        report(_valid_graph(item.get("nodes"), item.get("edges")) == expected, "merged DAG/host depth fixture mismatch: " + str(name))
        graph_count += 1
    return {"operations": len(actual), "examples": example_count, "fixtures": fixture_count, "graphs": graph_count}
