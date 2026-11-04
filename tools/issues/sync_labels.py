#!/usr/bin/env python3
"""Add/update reviewed labels; preview by default, never delete or relabel issues."""
import argparse
import json
import os
from pathlib import Path
import re
import sys
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, build_opener, HTTPRedirectHandler

ROOT = Path(__file__).resolve().parents[2]


def load_labels(path):
    labels = json.loads(path.read_text(encoding="utf-8"))
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


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Do not forward an authorization header to a redirect target.
        raise ValueError("GitHub redirect refused; verify the current repository name")


class GitHub:
    def __init__(self, repo, token):
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
            raise ValueError("repository must be owner/name")
        if any(part in {".", ".."} for part in repo.split("/")):
            raise ValueError("invalid repository")
        self.base = "https://api.github.com/repos/" + repo
        self.token = token
        self.opener = build_opener(NoRedirect())

    def request(self, method, path, data=None):
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        payload = None if data is None else json.dumps(data).encode("utf-8")
        if payload is not None:
            headers["Content-Type"] = "application/json"
        request = Request(self.base + path, data=payload, headers=headers, method=method)
        with self.opener.open(request, timeout=30) as response:
            return json.load(response)


def synchronize(api, labels, apply=False):
    existing = {}
    page = 1
    while True:
        batch = api.request("GET", f"/labels?per_page=100&page={page}")
        for label in batch:
            existing[label["name"].casefold()] = label
        if len(batch) < 100:
            break
        page += 1
    changes = 0
    for label in labels:
        current = existing.get(label["name"].casefold())
        if current is None:
            method, path = "POST", "/labels"
            payload = label
        elif (current["color"].lower(), current.get("description") or "") != (label["color"], label["description"]):
            method, path = "PATCH", "/labels/" + quote(current["name"], safe="")
            # Preserve spelling and all labels outside the manifest; never rename.
            payload = {key: label[key] for key in ("color", "description")}
        else:
            continue
        print(f"{'apply' if apply else 'preview'} {method} {label['name']}")
        if apply:
            api.request(method, path, payload)
        changes += 1
    return changes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--apply", action="store_true", help="explicitly create/update managed labels")
    args = parser.parse_args()
    try:
        labels = load_labels(ROOT / ".github/labels.json")
        token = os.environ.get("GH_TOKEN", "")
        if args.apply and not token:
            raise ValueError("--apply requires an authorized GH_TOKEN")
        changes = synchronize(GitHub(args.repo, token), labels, args.apply)
        print(f"{changes} label changes {'applied' if args.apply else 'planned (no writes)'}")
    except HTTPError as error:
        print(f"GitHub API returned HTTP {error.code}; stopped. Review permissions/rate limits and retry.", file=sys.stderr)
        return 1
    except (ValueError, OSError, URLError) as error:
        print(f"Label sync stopped: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
