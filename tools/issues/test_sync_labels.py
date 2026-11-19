"""Behavior checks for public-data/privileged-label synchronization boundaries."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock
from urllib.error import HTTPError
from urllib.request import Request

from forgejo import NoRedirect
from sync_labels import LabelClient, load_labels, main, synchronize
from validate import Loader
import yaml


LABEL = {"name": "type:bug", "color": "d73a4a", "description": "Bug reports"}


class FakeAPI(LabelClient):
    def __init__(self, existing=(), fail=False):
        self.labels = copy.deepcopy(list(existing))
        self.calls = []
        self.fail = fail

    def read_page(self, path):
        self.calls.append(("GET", path, None))
        page = int(path.rsplit("=", 1)[1])
        return self.labels[(page - 1) * 100:page * 100], page * 100 < len(self.labels)

    def write_label(self, method, path, data):
        self.calls.append((method, path, data))
        if self.fail:
            raise HTTPError("https://forge.example", 403, "forbidden", {}, None)
        if method == "POST":
            self.labels.append(dict(data, id=101))
        elif method == "PATCH":
            next(item for item in self.labels if path == f"/labels/{item['id']}").update(data)
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

    def test_apply_requires_explicit_credential_file(self):
        with mock.patch.dict(os.environ, {"GH_TOKEN": "old-token"}), mock.patch("sync_labels.LabelClient") as client, contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(main(["--forgejo-url", "http://localhost", "--repo", "example/project", "--apply"]), 1)
            client.assert_not_called()

    def test_wire_request_uses_numeric_path_and_json_without_renaming(self):
        api = LabelClient("https://forge.example/git", "example/project")
        response = io.BytesIO()
        response.status = 200
        payload = {"color": "#d73a4a", "description": "Bug reports"}
        with mock.patch.object(api.opener, "open", return_value=response) as opened:
            api.write_label("PATCH", "/labels/17", payload)
        request = opened.call_args.args[0]
        self.assertEqual(request.full_url, "https://forge.example/git/api/v1/repos/example/project/labels/17")
        self.assertEqual((request.get_method(), request.get_header("Content-type")), ("PATCH", "application/json"))
        self.assertEqual(json.loads(request.data), payload)

    def test_apply_and_rerun_are_idempotent(self):
        api = FakeAPI()
        self.assertEqual(self.run_sync(api, apply=True), 1)
        self.assertEqual(self.run_sync(api, apply=True), 0)
        self.assertEqual([c[0] for c in api.calls], ["GET", "POST", "GET"])

    def test_update_preserves_spelling_and_unmanaged_labels(self):
        existing = dict(LABEL, name="Type:Bug", color="#000000", id=17)
        custom = {"name": "keep-me", "color": "ffffff", "description": "custom"}
        api = FakeAPI([existing, custom])
        self.run_sync(api, apply=True)
        self.assertEqual(api.labels[0]["name"], "Type:Bug")
        self.assertEqual(api.labels[1], custom)
        self.assertEqual(api.calls[-1], ("PATCH", "/labels/17", {"color": "#d73a4a", "description": LABEL["description"]}))
        self.assertEqual(self.run_sync(api, apply=True), 0)

    def test_unicode_casefold_match_is_left_unmanaged(self):
        managed = dict(LABEL, name="status:needs-triage")
        custom = dict(managed, name="\u017ftatus:needs-triage", color="#000000", id=17)
        api = FakeAPI([custom])
        self.assertEqual(self.run_sync(api, [managed], apply=True), 1)
        self.assertEqual(api.labels[0], custom)
        self.assertEqual(api.calls[-1], ("POST", "/labels", dict(managed, color="#" + managed["color"])))
        self.assertEqual(self.run_sync(api, [managed], apply=True), 0)

    def test_conflicting_managed_ids_across_pages_stop_before_any_write(self):
        first = dict(LABEL, name="Type:Bug", id=17)
        second = dict(LABEL, id=18)
        unrelated = [dict(LABEL, name=f"external-{n}") for n in range(99)]
        planned = [dict(LABEL, name="type:feature"), LABEL]
        for apply in (False, True):
            with self.subTest(apply=apply):
                api = FakeAPI([first, *unrelated, second])
                with self.assertRaisesRegex(ValueError, "ambiguous.*type:bug"):
                    self.run_sync(api, planned, apply=apply)
                self.assertEqual([method for method, _, _ in api.calls], ["GET", "GET"])

    def test_rejects_non_numeric_label_id_before_update(self):
        api = FakeAPI([dict(LABEL, color="#000000", id="bad-id")])
        with self.assertRaisesRegex(ValueError, "label ID"):
            self.run_sync(api, apply=True)
        self.assertEqual([call[0] for call in api.calls], ["GET"])

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
        self.assertEqual(api.calls[-1], ("POST", "/labels", dict(LABEL, color="#d73a4a")))

    def test_repository_cannot_supply_host_query_or_traversal(self):
        for repo in ["https://evil.invalid/repo", "a/b?token=1", "a/b/../c", "a/..", "../b"]:
            with self.subTest(repo=repo), self.assertRaises(ValueError):
                LabelClient("https://forge.example", repo)

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

    def test_manifest_rejects_duplicate_json_fields(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "labels.json"
            path.write_text(
                '[{"name": "type:bug", "color": "d73a4a", "color": "ffffff", "description": "Bug reports"}]',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "duplicate JSON field in label manifest: 'color'"):
                load_labels(path)

    def test_yaml_on_and_duplicate_keys(self):
        self.assertEqual(yaml.load("on: {push: null}\nenabled: false", Loader=Loader), {"on": {"push": None}, "enabled": False})
        with self.assertRaises(ValueError):
            yaml.load("permissions: read\npermissions: write", Loader=Loader)


if __name__ == "__main__":
    unittest.main()
