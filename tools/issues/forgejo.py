"""Repository-scoped client for explicitly selected Forgejo instances."""
import json
from pathlib import Path
import re
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


def connection_args(parser):
    parser.add_argument("--forgejo-url", required=True, help="Forgejo instance URL (including any subpath)")
    parser.add_argument("--repo", required=True, help="repository as owner/name")
    parser.add_argument("--token-file", type=Path, help="private file containing a repository-scoped Forgejo token")


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("Forgejo redirect refused; verify the instance and repository")


class Forgejo:
    def __init__(self, instance, repo, token_file=None):
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo) or any(part in {".", ".."} for part in repo.split("/")):
            raise ValueError("repository must be owner/name")
        try:
            parts = urlsplit(instance)
            loopback = parts.hostname in {"127.0.0.1", "::1", "localhost"}
            valid = (parts.hostname and parts.port != 0 and parts.username is None and not parts.query
                     and not parts.fragment and "\\" not in instance and not re.search(r"[\x00-\x20\x7f]", instance)
                     and (parts.scheme == "https" or (parts.scheme == "http" and loopback)))
        except ValueError:
            valid = False
        if not valid:
            raise ValueError("expected an HTTPS Forgejo URL (HTTP only on loopback) without credentials, query or fragment")
        self.repo = repo
        self.base = instance.rstrip("/") + "/api/v1/repos/" + repo
        self.opener = build_opener(ProxyHandler({}) if loopback else ProxyHandler(), NoRedirect())
        self.headers = {"Accept": "application/json"}
        if token_file is not None:
            token = token_file.read_text(encoding="ascii").strip()
            if not re.fullmatch(r"[!-~]+", token):
                raise ValueError("expected one nonempty Forgejo token in the token file")
            self.headers["Authorization"] = "token " + token

    def read_page(self, path):
        request = Request(self.base + path, headers=self.headers, method="GET")
        with self.opener.open(request, timeout=30) as response:
            batch = json.load(response)
            if not isinstance(batch, list):
                raise ValueError("expected a Forgejo list response")
            more = any(re.search(r'<[^>]*>\s*;\s*rel="next"', link)
                       for link in response.headers.get_all("Link", []))
            return batch, more

    def pages(self, path):
        separator = "&" if "?" in path else "?"
        page = 1
        while True:
            batch, more = self.read_page(f"{path}{separator}limit=100&page={page}")
            yield from batch
            if not more:
                return
            page += 1
