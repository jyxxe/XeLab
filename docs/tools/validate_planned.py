"""Read-only checks for planned presets; imported by validate_docs.py.

The baseline uses the standard library. Strict checks resolve schemas only inside
docs and require jsonschema/referencing; no generator or network access is used.
"""
from pathlib import Path
import hashlib
import re
from urllib.parse import unquote, urlparse


VERSION = "2.6.0"
CONTRACT_VERSION = "0.1.0"
FILES = ("input.schema.json", "output.schema.json", "stream.schema.json", "semantics.json")
EXPECTED = {
    "ai-chat-openai-compatible": ("ordinary", {"ai.chat": "chat"}),
    "document-parser": ("ordinary", {"document.parse": "parse"}),
    "http-client": ("ordinary", {"http.request": "request"}),
    "notification": ("ordinary", {"notification.send": "send"}),
    "database-postgres": ("ordinary", {"database.query": "query", "database.execute": "execute"}),
    "automation-webhook-trigger": ("ordinary", {"trigger.webhook": "webhook"}),
    "email-connector": ("ordinary", {"email.send": "send"}),
    "archive-processor": ("ordinary", {"archive.extract": "extract", "archive.create": "create"}),
    "ai-reranker": ("ordinary", {"ai.rerank": "rerank"}),
    "browser-automation": ("ordinary", {"browser.navigate": "navigate", "browser.extract": "extract"}),
    "git-connector": ("ordinary", {"git.repository": "repository"}),
    "rss-reader": ("ordinary", {"feed.read": "read"}),
    "micro-json-validator": ("micro", {"json.validate": "validate"}),
    "micro-json-project": ("micro", {"json.project": "project"}),
    "micro-json-merge": ("micro", {"json.merge": "merge"}),
    "micro-csv-json": ("micro", {"convert.csv-json": "csv-json", "convert.json-csv": "json-csv"}),
    "micro-markdown-html": ("micro", {"convert.markdown-html": "markdown-html"}),
    "micro-text-normalizer": ("micro", {"text.normalize": "normalize"}),
    "micro-regex-extract": ("micro", {"text.regex-extract": "regex-extract"}),
    "micro-datetime-convert": ("micro", {"datetime.convert": "convert"}),
    "micro-flow-condition": ("micro", {"flow.condition": "condition"}),
    "micro-flow-map": ("micro", {"flow.map": "map"}),
    "micro-flow-filter": ("micro", {"flow.filter": "filter"}),
    "micro-flow-template": ("micro", {"flow.template": "template"}),
    "micro-data-redactor": ("micro", {"data.redact": "redact"}),
    "micro-url-parser": ("micro", {"url.parse": "parse"}),
    "micro-file-metadata": ("micro", {"file.metadata": "metadata"}),
    "micro-hash": ("micro", {"crypto.hash": "hash"}),
    "micro-text-chunk": ("micro", {"text.chunk": "chunk"}),
    "micro-text-encoding": ("micro", {"text.encoding-convert": "encoding-convert"}),
    "micro-xml-json": ("micro", {"convert.xml-json": "xml-json"}),
    "micro-data-sort-group": ("micro", {"data.sort": "sort", "data.group": "group"}),
}


def _nodes(value):
    if isinstance(value, dict):
        yield value
        for item in value.values():
            yield from _nodes(item)
    elif isinstance(value, list):
        for item in value:
            yield from _nodes(item)


def _links(path):
    """Return local targets, excluding links inside fenced code blocks."""
    lines = []
    fenced = False
    for line in path.read_text().splitlines():
        if line.strip().startswith(("```", "~~~")):
            fenced = not fenced
        elif not fenced:
            lines.append(line)
    for value in re.findall(r"(?<!!)\[[^\]]*\]\(([^\n)]+)\)", "\n".join(lines)):
        value = value.strip().strip("<>")
        parsed = urlparse(value)
        if not parsed.scheme and parsed.path:
            yield (path.parent / unquote(parsed.path)).resolve()


def validate_planned(DOCS, check, read, strict=False):
    """Report checks through check(bool, message); never modify any source file."""
    docs = Path(DOCS).resolve()

    def report(ok, message):
        check(bool(ok), "planned: " + message)

    def load(path):
        try:
            return read(path)
        except Exception as exc:
            report(False, f"cannot read {path.relative_to(docs)}: {exc}")
            return None

    def rows(value, label):
        report(isinstance(value, list), label + " must be an array")
        return value if isinstance(value, list) else []

    def local_reference(base, value):
        parsed = urlparse(value)
        if parsed.scheme or parsed.netloc or parsed.query:
            raise ValueError("only relative local schema references are allowed: " + value)
        target = (base.parent / unquote(parsed.path)).resolve() if parsed.path else base
        if not target.is_relative_to(docs):
            raise ValueError("reference leaves docs: " + value)
        data = read(target)
        fragment = unquote(parsed.fragment)
        if fragment and not fragment.startswith("/"):
            raise ValueError("only JSON Pointer fragments are allowed: " + value)
        for token in fragment[1:].split("/") if fragment else []:
            token = token.replace("~1", "/").replace("~0", "~")
            data = data[int(token)] if isinstance(data, list) else data[token]
        return target, fragment

    catalog_file = docs / "preset-plugins/planned-catalog.json"
    registry_file = docs / "preset-plugins/planned-contracts.json"
    catalog = load(catalog_file)
    registry = load(registry_file)
    main = load(docs / "preset-plugins/catalog.json")
    if not all(isinstance(value, dict) for value in (catalog, registry, main)):
        return {"directories": 0, "capabilities": 0, "fixtures": 0}
    for label, value in (("catalog", catalog), ("contract registry", registry)):
        report(value.get("version") == VERSION, label + " specification version differs")
        report(value.get("status") == "planned", label + " status must be planned")
    report(main.get("version") == VERSION, "main catalog specification version differs")
    report(main.get("planned_catalog") == "planned-catalog.json", "main catalog planned pointer differs")
    entries = rows(catalog.get("entries"), "catalog entries")
    report(len(entries) == 32, "expected 32 planned directories")
    by_directory = {}
    for entry in entries:
        if not isinstance(entry, dict):
            report(False, "catalog entry must be an object")
            continue
        directory = entry.get("directory")
        report(directory not in by_directory, "duplicate planned directory " + str(directory))
        by_directory[directory] = entry
    report(set(by_directory) == set(EXPECTED), "planned directory set differs from the 32 requested presets")
    ordinary = sum(e.get("kind") == "ordinary" for e in by_directory.values())
    micro = sum(e.get("kind") == "micro" for e in by_directory.values())
    report(ordinary == catalog.get("ordinary_count") == 12, "ordinary count must be 12")
    report(micro == catalog.get("micro_count") == 20, "micro count must be 20")
    installed_directories = {
        e.get("directory") for key in ("presets", "micro_presets")
        for e in rows(main.get(key), "main catalog " + key) if isinstance(e, dict)
    }
    report(not (set(EXPECTED) & installed_directories), "planned entry registered as an installable preset")

    expected_contracts = {}
    draft_schema_files = {docs / "preset-plugins/planned/shared.schema.json"}
    for directory, (kind, capabilities) in EXPECTED.items():
        entry = by_directory.get(directory, {})
        report(entry.get("kind") == kind, directory + " kind differs")
        report(entry.get("status") == "planned", directory + " status must be planned")
        report(entry.get("contract_version") == CONTRACT_VERSION, directory + " contract version differs")
        ids = rows(entry.get("capability_ids"), directory + " capability_ids")
        report(len(ids) == len(set(ids)) and set(ids) == set(capabilities), directory + " capability IDs differ")
        operations = rows(entry.get("operations"), directory + " operations")
        actual_operations = {(x.get("capability_id"), x.get("operation")) for x in operations if isinstance(x, dict)}
        report(len(operations) == len(capabilities) and actual_operations == set(capabilities.items()), directory + " operations differ")
        folder = docs / "preset-plugins" / directory
        for page in ("README.md", "interfaces.md"):
            file = folder / page
            report(file.is_file(), directory + " missing " + page)
            if file.is_file():
                content = file.read_text()
                report(VERSION in content and "planned" in content, str(file.relative_to(docs)) + " version/status absent")
        links = set()
        for page in (folder / "README.md", folder / "interfaces.md"):
            if page.is_file():
                links.update(_links(page))
        for capability, operation in capabilities.items():
            contract_folder = folder / "contracts" / operation
            report(capability not in expected_contracts, "duplicate capability " + capability)
            digest_parts = []
            for filename in FILES:
                file = contract_folder / filename
                report(file.is_file(), "missing contract file " + str(file.relative_to(docs)))
                report(file.resolve() in links, "unlinked capability contract " + str(file.relative_to(docs)))
                if file.is_file():
                    digest_parts.append(filename + " " + hashlib.sha256(file.read_bytes()).hexdigest() + "\n")
                if filename.endswith("schema.json"):
                    draft_schema_files.add(file)
            digest = "sha256:" + hashlib.sha256("".join(digest_parts).encode()).hexdigest()
            matching = [x for x in operations if isinstance(x, dict) and x.get("capability_id") == capability]
            report(len(matching) == 1 and matching[0].get("schema_digest") == digest, capability + " catalog contract digest differs")
            expected_contracts[capability] = {
                "capability_id": capability, "operation": operation,
                "directory": directory, "kind": kind, "status": "planned",
                "version": CONTRACT_VERSION, "folder": str(contract_folder.relative_to(docs)),
                "schema_digest": digest,
            }
            semantics = load(contract_folder / "semantics.json")
            if isinstance(semantics, dict):
                for key, value in (("capability_id", capability), ("kind", kind), ("status", "planned"), ("version", CONTRACT_VERSION)):
                    report(semantics.get(key) == value, capability + " semantics " + key + " differs")
            for file in contract_folder.glob("*.schema.json"):
                draft_schema_files.add(file)
                schema = load(file)
                if isinstance(schema, dict):
                    report(schema.get("x-xelab-status") == "planned", str(file.relative_to(docs)) + " status absent")

    actual_contracts = {}
    for item in rows(registry.get("contracts"), "planned contract registry contracts"):
        if isinstance(item, dict):
            capability = item.get("capability_id")
            report(capability not in actual_contracts, "duplicate registry capability " + str(capability))
            actual_contracts[capability] = item
        else:
            report(False, "registry contract must be an object")
    report(len(expected_contracts) == len(actual_contracts) == 37, "expected 37 planned capability contracts")
    report(actual_contracts == expected_contracts, "planned catalog and contract registry differ")
    stable = load(docs / "preset-plugins/contracts/registry.json")
    if isinstance(stable, dict):
        stable_ids = {x.get("name") for x in stable.get("contracts", []) if isinstance(x, dict)}
        report(not (set(expected_contracts) & stable_ids), "draft capability entered stable contract registry")

    reference_files = set()
    for item in rows(registry.get("reference_documents"), "contract reference_documents"):
        if not isinstance(item, dict) or not isinstance(item.get("file"), str):
            report(False, "malformed reference document")
            continue
        name = item["file"]
        target = (docs / name).resolve()
        report(name not in reference_files, "duplicate reference document " + name)
        reference_files.add(name)
        report(target.is_relative_to(docs) and target.is_file(), "missing/outside reference document " + name)
        if target.is_relative_to(docs) and target.is_file():
            digest = "sha256:" + hashlib.sha256(target.read_bytes()).hexdigest()
            report(item.get("sha256") == digest, "reference document digest differs " + name)
    report(reference_files == {"preset-plugins/planned/shared.schema.json", "contracts/openapi.json"}, "reference document set differs")

    for file in sorted(draft_schema_files):
        schema = load(file)
        for node in _nodes(schema):
            if "$ref" in node:
                try:
                    local_reference(file, node["$ref"])
                except Exception as exc:
                    report(False, str(file.relative_to(docs)) + " invalid local reference: " + str(exc))
    graph = {p.resolve(): list(_links(p)) for p in docs.rglob("*.md")}
    seen, pending = set(), [(docs / "README.md").resolve()]
    while pending:
        path = pending.pop()
        if path not in seen:
            seen.add(path)
            pending.extend(x for x in graph.get(path, []) if x.suffix == ".md")
    for directory in EXPECTED:
        for page in ("README.md", "interfaces.md"):
            file = docs / "preset-plugins" / directory / page
            report(file.resolve() in seen, "unreachable capability page " + str(file.relative_to(docs)))

    fixture_count = 0
    if strict:
        try:
            from jsonschema import Draft202012Validator, FormatChecker
            from referencing import Registry, Resource
            from referencing.jsonschema import DRAFT202012
        except ImportError as exc:
            report(False, "strict dependencies missing: " + str(exc))
            return {"directories": len(by_directory), "capabilities": len(expected_contracts), "fixtures": 0}

        def retrieve(uri):
            parsed = urlparse(uri)
            if parsed.scheme != "file" or parsed.netloc not in ("", "localhost"):
                raise ValueError("non-local schema retrieval forbidden: " + uri)
            file = Path(unquote(parsed.path)).resolve()
            if not file.is_relative_to(docs):
                raise ValueError("schema retrieval outside docs: " + uri)
            return Resource.from_contents(read(file), default_specification=DRAFT202012)

        resolver = Registry(retrieve=retrieve)
        for file in sorted(draft_schema_files):
            try:
                Draft202012Validator.check_schema(read(file))
            except Exception as exc:
                report(False, "invalid draft schema " + str(file.relative_to(docs)) + ": " + str(exc))
        fixture_file = docs / "preset-plugins/planned/validation.json"
        fixtures = load(fixture_file)
        if isinstance(fixtures, dict):
            fixtures = fixtures.get("cases")
        fixtures = rows(fixtures, "validation cases")
        coverage = {capability: set() for capability in expected_contracts}
        names = set()
        for index, item in enumerate(fixtures):
            label = str(item.get("name", index)) if isinstance(item, dict) else str(index)
            if not isinstance(item, dict):
                report(False, "fixture must be an object: " + label)
                continue
            report(label not in names, "duplicate fixture name " + label)
            names.add(label)
            capability = item.get("capability_id")
            report(capability in expected_contracts, "fixture capability unknown: " + label)
            report(isinstance(item.get("valid"), bool), "fixture valid must be boolean: " + label)
            reference = item.get("schema")
            reference = reference.get("$ref") if isinstance(reference, dict) else reference
            try:
                if not isinstance(reference, str):
                    raise ValueError("fixture requires a local schema $ref")
                target, fragment = local_reference(fixture_file, reference)
                contract = expected_contracts[capability]
                folder = (docs / contract["folder"]).resolve()
                report(target.parent == folder and target.name in FILES[:3] and not fragment, "fixture schema does not match capability: " + label)
                validator = Draft202012Validator({"$ref": target.as_uri() + ("#" + fragment if fragment else "")}, registry=resolver, format_checker=FormatChecker())
                accepted = validator.is_valid(item["value"])
                report(accepted == item.get("valid"), "boundary fixture mismatch: " + label)
                if target == folder / "input.schema.json" and not fragment and isinstance(item.get("valid"), bool):
                    coverage[capability].add(item["valid"])
                fixture_count += 1
            except Exception as exc:
                report(False, "fixture validation failed " + label + ": " + str(exc))
        for capability, values in coverage.items():
            report(values == {True, False}, capability + " requires positive and negative input fixtures")
    return {"directories": len(by_directory), "capabilities": len(expected_contracts), "fixtures": fixture_count}
