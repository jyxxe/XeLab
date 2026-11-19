#!/usr/bin/env python3
"""Add/update reviewed labels; preview by default, never delete or relabel issues."""
import argparse
import json
from pathlib import Path
import re
import sys
from urllib.error import HTTPError, URLError
from urllib.request import Request

from forgejo import Forgejo, connection_args

ROOT = Path(__file__).resolve().parents[2]


def unique_label_fields(pairs):
    fields = {}
    for name, value in pairs:
        if name in fields:
            raise ValueError(f"duplicate JSON field in label manifest: {name!r}")
        fields[name] = value
    return fields


def load_labels(path):
    labels = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_label_fields)
    if not isinstance(labels, list) or not labels:
        raise ValueError("label manifest must be a non-empty list")
    seen = set()
    for label in labels:
        if not isinstance(label, dict) or set(label) != {"name", "color", "description"}:
            raise ValueError("each label needs exactly name, color and description")
        name, color, description = (label[k] for k in ("name", "color", "description"))
        if not all(isinstance(v, str) for v in (name, color, description)):
            raise ValueError("label values must be strings")
        if not re.fullmatch(r"(?:type|status|priority|area):[a-z0-9-]+", name) or len(name) > 50:
            raise ValueError("invalid managed label name")
        if name.casefold() in seen:
            raise ValueError("duplicate managed label")
        if not re.fullmatch(r"[0-9a-f]{6}", color) or not 1 <= len(description) <= 100:
            raise ValueError("invalid color or description length")
        seen.add(name.casefold())
    return labels


class LabelClient(Forgejo):
    def write_label(self, method, path, data):
        request = Request(self.base + path, data=json.dumps(data).encode(), method=method,
                          headers={**self.headers, "Content-Type": "application/json"})
        with self.opener.open(request, timeout=30) as response:
            if response.status not in {200, 201}:
                raise ValueError("unexpected Forgejo label response; inspect the repository before retrying")


def synchronize(api, labels, apply=False):
    managed_names = {label["name"].lower() for label in labels}
    existing = {}
    for item in api.pages("/labels"):
        name = item["name"]
        if not name.isascii():
            continue
        key = name.lower()
        if key not in managed_names:
            continue
        if key in existing and existing[key].get("id") != item.get("id"):
            raise ValueError(f"ambiguous Forgejo label IDs for {key}")
        existing[key] = item

    changes = 0
    for label in labels:
        current = existing.get(label["name"].lower())
        if current is None:
            method, path = "POST", "/labels"
            payload = dict(label, color="#" + label["color"])
        elif (current["color"].removeprefix("#").lower(), current.get("description") or "") != (label["color"], label["description"]):
            label_id = current.get("id")
            if type(label_id) is not int or label_id <= 0:
                raise ValueError("invalid Forgejo label ID; refusing to update")
            method, path = "PATCH", f"/labels/{label_id}"
            payload = {"color": "#" + label["color"], "description": label["description"]}
        else:
            continue
        print(f"{'apply' if apply else 'preview'} {method} {label['name']}")
        if apply:
            api.write_label(method, path, payload)
        changes += 1
    return changes


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    connection_args(parser)
    parser.add_argument("--apply", action="store_true", help="explicitly create/update managed labels")
    args = parser.parse_args(argv)
    try:
        labels = load_labels(ROOT / ".github/labels.json")
        if args.apply and args.token_file is None:
            raise ValueError("--apply requires an authorized Forgejo --token-file")
        changes = synchronize(LabelClient(args.forgejo_url, args.repo, args.token_file), labels, args.apply)
        print(f"{changes} label changes {'applied' if args.apply else 'planned (no writes)'}")
    except HTTPError as error:
        print(f"Forgejo API returned HTTP {error.code}; stopped. Preview again before retrying.", file=sys.stderr)
        return 1
    except (ValueError, OSError, URLError) as error:
        print(f"Label sync stopped: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
