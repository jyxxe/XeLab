#!/usr/bin/env python3
"""Read-only checks for the supported Issue Forms subset and workflow trust boundary."""
from pathlib import Path
import re
import yaml

from sync_labels import load_labels

ROOT = Path(__file__).resolve().parents[2]


class Loader(yaml.SafeLoader):
    pass


# GitHub uses YAML 1.2: `on` is a string, not the YAML 1.1 boolean True.
Loader.yaml_implicit_resolvers = {
    key: [(tag, exp) for tag, exp in rules if tag != "tag:yaml.org,2002:bool"]
    for key, rules in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
Loader.add_implicit_resolver("tag:yaml.org,2002:bool", re.compile(r"^(?:true|false)$"), list("tf"))


def mapping(loader, node):
    result = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node)
        if key in result:
            raise ValueError(f"duplicate YAML key: {key}")
        result[key] = loader.construct_object(value_node)
    return result


Loader.add_constructor("tag:yaml.org,2002:map", mapping)


def read(path):
    return yaml.load(path.read_text(encoding="utf-8"), Loader=Loader)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate(root=ROOT):
    labels = {x["name"] for x in load_labels(root / ".github/labels.json")}
    directory = root / ".github/ISSUE_TEMPLATE"
    config = read(directory / "config.yml")
    require(config["blank_issues_enabled"] is False, "use structured public forms")
    require(any(x["url"].endswith("/SECURITY.md") for x in config["contact_links"]), "missing security policy route")
    names, kinds = set(), set()
    paths = sorted(path for path in directory.glob("*.yml") if path.name != "config.yml")
    require(len(paths) == 5, "expected five proposal forms")
    required_ids = {"provenance", "baseline", "summary", "details", "evidence", "acceptance", "dependencies", "checks"}
    for path in paths:
        form = read(path)
        require(set(form) == {"name", "description", "title", "labels", "body"}, f"unexpected form keys: {path}")
        for key in ("name", "description", "title"):
            require(isinstance(form[key], str) and form[key].strip(), f"missing {key}")
        require(form["name"] not in names, "duplicate form name")
        names.add(form["name"])
        require(set(form["labels"]) <= labels, "form references unknown label")
        types = [name for name in form["labels"] if name.startswith("type:")]
        require(len(types) == 1 and form["labels"] == [types[0], "status:needs-triage"], "unsafe initial labels")
        require(types[0] not in kinds, "duplicate proposal type")
        kinds.add(types[0])
        ids, field_labels = set(), set()
        for field in form["body"]:
            kind, attrs = field["type"], field["attributes"]
            require(kind in {"markdown", "textarea", "input", "dropdown", "checkboxes"}, "unsupported field type")
            if kind == "markdown":
                require(isinstance(attrs.get("value"), str), "markdown requires value")
                continue
            identifier = field["id"]
            require(re.fullmatch(r"[A-Za-z0-9_-]+", identifier) and identifier not in ids, "invalid/duplicate field id")
            require(isinstance(attrs.get("label"), str) and attrs["label"] not in field_labels, "invalid/duplicate field label")
            ids.add(identifier)
            field_labels.add(attrs["label"])
            if kind == "checkboxes":
                require(all(x.get("required") is True and isinstance(x.get("label"), str) for x in attrs["options"]), "checks must be required")
            else:
                require(field.get("validations", {}).get("required") is True, "evidence fields must be required")
            if kind == "dropdown":
                require(all(isinstance(x, str) for x in attrs["options"]), "dropdown values must be strings")
                require(len(set(attrs["options"])) == len(attrs["options"]), "duplicate choices")
        require(required_ids <= ids, "missing common submission fields")
        if types == ["type:security"]:
            require("disclosure" in ids and "SECURITY.md" in form["body"][0]["attributes"]["value"], "missing security disclosure guard")
    require(kinds == {"type:" + kind for kind in ("bug", "feature", "architecture", "security", "research")}, "incorrect proposal categories")
    workflows = root / ".github/workflows"
    checks = read(workflows / "issue-proposals.yml")
    sync = read(workflows / "issue-labels.yml")
    require(set(checks["on"]) == {"push", "pull_request", "workflow_dispatch"}, "unexpected checks trigger")
    require(set(sync["on"]) == {"push", "workflow_dispatch"}, "untrusted event must not trigger label writes")
    for workflow in (checks, sync):
        require(workflow["permissions"] == {"contents": "read"}, "default token must be read-only")
        require(workflow["on"]["push"]["branches"] == ["main"], "push must target main")
        for job in workflow["jobs"].values():
            require(job["runs-on"] == "ubuntu-24.04", "no self-hosted issue runner")
            for step in job["steps"]:
                if "uses" in step:
                    require(re.fullmatch(r"actions/checkout@[a-f0-9]{40}", step["uses"]), "pin checkout to full SHA")
                    require(step["with"]["persist-credentials"] is False, "must not persist token")
                require("${{" not in step.get("run", ""), "no expression interpolation in shell")
    require(set(checks["jobs"]) == {"validate"}, "unexpected check job")
    require("permissions" not in checks["jobs"]["validate"], "PR job must inherit read-only permissions")
    require(set(sync["jobs"]) == {"sync"}, "unexpected write job")
    job = sync["jobs"]["sync"]
    require(job["if"] == "github.ref == 'refs/heads/main'", "manual sync must use main")
    require(job["permissions"] == {"contents": "read", "issues": "write"}, "label sync has excessive permissions")
    require(job["steps"][0]["with"]["ref"] == "main", "only checkout trusted main for writes")
    print(f"Validated {len(paths)} forms, {len(labels)} labels and workflow trust boundaries")


if __name__ == "__main__":
    validate()
