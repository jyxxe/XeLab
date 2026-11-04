"""Behavior checks for public-data/privileged-label synchronization boundaries."""
import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest
from urllib.error import HTTPError
from urllib.request import Request

from sync_labels import GitHub, NoRedirect, load_labels, synchronize
from validate import Loader
import yaml


LABEL = {"name": "type:bug", "color": "d73a4a", "description": "Bug reports"}


class FakeAPI:
    def __init__(self, existing=(), fail=False):
        self.labels = copy.deepcopy(list(existing))
        self.calls = []
        self.fail = fail

    def request(self, method, path, data=None):
        self.calls.append((method, path, data))
        if method == "GET":
            page = int(path.rsplit("=", 1)[1])
            return self.labels[(page - 1) * 100:page * 100]
        if self.fail:
            raise HTTPError("https://api.github.com", 403, "forbidden", {}, None)
        if method == "POST":
            self.labels.append(copy.deepcopy(data))
        elif method == "PATCH":
            self.labels[0].update(data)
        else:
            raise AssertionError("unexpected destructive method")


class LabelSyncTests(unittest.TestCase):
    def run_sync(self, api, labels=None, apply=False):
        with contextlib.redirect_stdout(io.StringIO()):
            return synchronize(api, [LABEL] if labels is None else labels, apply)

    def test_preview_never_writes(self):
        api = FakeAPI()
        self.assertEqual(self.run_sync(api), 1)
        self.assertEqual([c[0] for c in api.calls], ["GET"])

    def test_apply_and_rerun_are_idempotent(self):
        api = FakeAPI()
        self.assertEqual(self.run_sync(api, apply=True), 1)
        self.assertEqual(self.run_sync(api, apply=True), 0)
        self.assertEqual([c[0] for c in api.calls], ["GET", "POST", "GET"])

    def test_update_preserves_spelling_and_unmanaged_labels(self):
        existing = dict(LABEL, name="Type:Bug", color="000000")
        custom = {"name": "keep-me", "color": "ffffff", "description": "custom"}
        api = FakeAPI([existing, custom])
        self.run_sync(api, apply=True)
        self.assertEqual(api.labels[0]["name"], "Type:Bug")
        self.assertEqual(api.labels[1], custom)
        self.assertEqual(api.calls[-1], ("PATCH", "/labels/Type%3ABug", {"color": LABEL["color"], "description": LABEL["description"]}))

    def test_pagination_finds_existing_label(self):
        api = FakeAPI([dict(LABEL, name=f"external-{n}") for n in range(100)] + [LABEL])
        self.assertEqual(self.run_sync(api, apply=True), 0)
        self.assertEqual(len(api.calls), 2)

    def test_permission_error_stops_before_next_write(self):
        api = FakeAPI(fail=True)
        with self.assertRaises(HTTPError):
            self.run_sync(api, [LABEL, dict(LABEL, name="type:feature")], apply=True)
        self.assertEqual(len(api.calls), 2)

    def test_malicious_existing_label_is_only_data(self):
        api = FakeAPI([dict(LABEL, name="$(touch /tmp/never); ${{ secrets.TOKEN }}")])
        self.run_sync(api, apply=True)
        self.assertEqual(api.calls[-1], ("POST", "/labels", LABEL))

    def test_repository_cannot_supply_host_query_or_traversal(self):
        for repo in ["https://evil.invalid/repo", "a/b?token=1", "a/b/../c", "a/..", "../b"]:
            with self.subTest(repo=repo), self.assertRaises(ValueError):
                GitHub(repo, "not-a-real-token")

    def test_redirect_cannot_forward_authorization(self):
        with self.assertRaises(ValueError):
            NoRedirect().redirect_request(Request("https://api.github.com"), None, 302, "", {}, "https://evil.invalid")

    def test_manifest_rejects_duplicates_or_executable_names(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "labels.json"
            for labels in [[LABEL, LABEL], [dict(LABEL, name="type:$(id)")], [dict(LABEL, color="red")]]:
                path.write_text(json.dumps(labels))
                with self.assertRaises(ValueError):
                    load_labels(path)

    def test_yaml_on_and_duplicate_keys(self):
        self.assertEqual(yaml.load("on: {push: null}\nenabled: false", Loader=Loader), {"on": {"push": None}, "enabled": False})
        with self.assertRaises(ValueError):
            yaml.load("permissions: read\npermissions: write", Loader=Loader)


if __name__ == "__main__":
    unittest.main()
