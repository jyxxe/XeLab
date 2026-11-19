"""Offline checks for Forgejo's repository transport and pagination."""
from email.message import Message
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from forgejo import Forgejo


def response(items, *links):
    body = io.BytesIO(json.dumps(items).encode())
    body.headers = Message()
    for link in links:
        body.headers.add_header("Link", link)
    return body


class ForgejoTests(unittest.TestCase):
    def test_capped_pages_keep_original_origin_and_subpath(self):
        with tempfile.TemporaryDirectory() as directory:
            token_file = Path(directory) / "token"
            token_file.write_text("test-forgejo-token\n")
            api = Forgejo("http://127.0.0.1:3000/git/", "example/project", token_file)
            pages = [response([1], '<ignored>; rel="last"', '<https://elsewhere.invalid/>; rel="next"'),
                     response([2], '<ignored>; rel="prev"')]
            with mock.patch.object(api.opener, "open", side_effect=pages) as opened:
                self.assertEqual(list(api.pages("/issues?state=open")), [1, 2])
        requests = [call.args[0] for call in opened.call_args_list]
        prefix = "http://127.0.0.1:3000/git/api/v1/repos/example/project/issues?state=open&limit=100&page="
        self.assertEqual([req.full_url for req in requests], [prefix + "1", prefix + "2"])
        for req in requests:
            self.assertEqual(req.get_method(), "GET")
            self.assertEqual(req.get_header("Authorization"), "token test-forgejo-token")

    def test_no_implicit_github_credentials(self):
        with mock.patch.dict(os.environ, {"GITHUB_TOKEN": "old-token", "GH_TOKEN": "old-token"}):
            api = Forgejo("https://forge.example", "example/project")
            with mock.patch.object(api.opener, "open", return_value=response([])) as opened:
                self.assertEqual(list(api.pages("/issues/1/comments")), [])
            self.assertIsNone(opened.call_args.args[0].get_header("Authorization"))

    def test_invalid_instance_is_rejected_before_reading_credentials(self):
        for instance in ("", "http://remote.example", "https://user:password@host", "file:///tmp",
                         "https://host/?token=secret", "https://host/#fragment", "https://host:invalid"):
            with self.subTest(instance=instance), self.assertRaises(ValueError):
                Forgejo(instance, "example/project", Path("does-not-exist"))
