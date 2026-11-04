#!/usr/bin/env python3
"""Drive real XeLab OAuth HTTPS pages and CORS with an isolated Chromium context.

The Rust acceptance fixture serves the production api::router directly over TLS
and supplies a private JSON configuration. The separate UI is a real callback
document. No response is mocked, no Cookie is installed, and no code, password
or credential enters the report.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.metadata
import ipaddress
import json
import os
import pathlib
import re
import secrets
import stat
import sys
import time
import urllib.parse


class CheckFailed(Exception):
    """A failed named assertion; deliberately carries no response or secret."""


def origin(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.path not in ("", "/")
        or parsed.query
        or parsed.fragment
        or not re.fullmatch(r"[a-z0-9.-]+", parsed.hostname)
    ):
        raise CheckFailed("fixture_origin")
    # Read the port here as well: malformed/out-of-range ports must fail early.
    parsed.port
    return f"https://{parsed.netloc}"


def url_origin(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    return f"{parsed.scheme}://{parsed.netloc}"


def fixture(path: pathlib.Path) -> dict:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) & 0o077:
        raise CheckFailed("fixture_permissions")
    value = json.loads(path.read_text(encoding="utf-8"))
    required = {
        "core_origin", "ui_origin", "redirect_uri", "client_id", "account_id",
        "password", "api_read_path", "expected_account_id", "tls_spki_sha256",
    }
    optional = {"scope", "connect_ip", "chromium_path", "timeout_ms"}
    if not isinstance(value, dict) or not required <= value.keys() or value.keys() - required - optional:
        raise CheckFailed("fixture_fields")
    if any(not isinstance(value[key], str) or not value[key] for key in required):
        raise CheckFailed("fixture_field_types")
    value["core_origin"] = origin(value["core_origin"])
    value["ui_origin"] = origin(value["ui_origin"])
    if value["core_origin"] == value["ui_origin"]:
        raise CheckFailed("distinct_origins")
    core_host = urllib.parse.urlsplit(value["core_origin"]).hostname
    ui_host = urllib.parse.urlsplit(value["ui_origin"]).hostname
    if core_host == ui_host:
        raise CheckFailed("distinct_cookie_hosts")
    redirect = urllib.parse.urlsplit(value["redirect_uri"])
    if (
        url_origin(value["redirect_uri"]) != value["ui_origin"]
        or redirect.username or redirect.password or redirect.fragment
        or not redirect.path.startswith("/")
    ):
        raise CheckFailed("fixture_redirect")
    if not value["api_read_path"].startswith("/api/platform/v1/") or any(
        character in value["api_read_path"] for character in "?#\\\r\n"
    ):
        raise CheckFailed("fixture_api_path")
    if len(base64.b64decode(value["tls_spki_sha256"], validate=True)) != 32:
        raise CheckFailed("fixture_certificate_pin")
    address = ipaddress.ip_address(value.get("connect_ip", "127.0.0.1"))
    if not address.is_loopback:
        raise CheckFailed("fixture_loopback")
    value["connect_ip"] = str(address)
    value.setdefault("scope", f"client:{value['client_id']}")
    value.setdefault("timeout_ms", 20000)
    if not isinstance(value["scope"], str) or not value["scope"]:
        raise CheckFailed("fixture_scope")
    if not isinstance(value["timeout_ms"], int) or not 1000 <= value["timeout_ms"] <= 60000:
        raise CheckFailed("fixture_timeout")
    return value


FETCH = """async ({url, method, fields, token}) => {
  const options = {method, mode: 'cors', credentials: 'omit', redirect: 'error',
                   cache: 'no-store', headers: {'Accept': 'application/json'}};
  if (fields !== null) {
    options.headers['Content-Type'] = 'application/x-www-form-urlencoded';
    options.body = new URLSearchParams(fields).toString();
  }
  if (token !== null) options.headers.Authorization = 'Bearer ' + token;
  try {
    const response = await fetch(url, options);
    return {status: response.status, type: response.type, body: await response.json()};
  } catch (_) {
    return {status: null, transport_error: true};
  }
}"""


def run(config: dict, report: dict) -> None:
    # Playwright debug traces can include form values and evaluate arguments.
    # Keep this credential-bearing subprocess free of ambient debug logging.
    os.environ.pop("DEBUG", None)
    os.environ.pop("PWDEBUG", None)
    from playwright.sync_api import sync_playwright

    def require(condition: bool, name: str) -> None:
        report["step"] = name
        if not condition:
            raise CheckFailed(name)
        report["checks"][name] = True

    core = config["core_origin"]
    ui = config["ui_origin"]
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    state = secrets.token_urlsafe(32)
    authorize = core + "/oauth/authorize?" + urllib.parse.urlencode({
        "response_type": "code", "client_id": config["client_id"],
        "redirect_uri": config["redirect_uri"], "state": state,
        "code_challenge": challenge, "code_challenge_method": "S256",
        "scope": config["scope"],
    })
    report["core_origin"] = core
    report["ui_origin"] = ui
    report["playwright_version"] = importlib.metadata.version("playwright")
    network_isolation = {"ui_received_password": False, "ui_received_core_cookie": False,
                         "inspection_failed": False}
    # Keep diagnostics in the existing safe HTTP report so an already compiled
    # Rust fixture can preserve them without ever recording a callback URL.
    report["http"].update({
        "consent_submit_status": None,
        "consent_location_is_ui": None,
        "consent_request_origin_class": "missing",
        "consent_request_has_core_cookie": False,
        "consent_form_has_csrf": False,
        "consent_form_csrf_length": None,
        "consent_form_exact_fields": False,
        "consent_error_code": None,
        "native_navigation_status": None,
        "callback_origin_class": "unknown",
        "console_form_action_violation": False,
        "console_csp_violation": False,
        "diagnostic_observer_failed": False,
    })
    core_host = urllib.parse.urlsplit(core).hostname
    ui_host = urllib.parse.urlsplit(ui).hostname
    connect = config["connect_ip"]
    if ":" in connect:
        connect = f"[{connect}]"
    resolver = f"MAP {core_host} {connect},MAP {ui_host} {connect}"
    with sync_playwright() as playwright:
        report["step"] = "browser_launch"
        browser = playwright.chromium.launch(
            executable_path=config.get("chromium_path", "/usr/bin/chromium"),
            headless=True,
            args=["--no-sandbox", "--disable-dev-shm-usage", "--no-proxy-server",
                  f"--host-resolver-rules={resolver}",
                  f"--ignore-certificate-errors-spki-list={config['tls_spki_sha256']}"],
        )
        report["browser_version"] = browser.version
        context = browser.new_context(viewport={"width": 1280, "height": 900}, reduced_motion="reduce")
        context.set_default_timeout(config["timeout_ms"])
        context.set_default_navigation_timeout(config["timeout_ms"])
        page = context.new_page()

        def inspect_request(request) -> None:
            try:
                if (url_origin(request.url) == core
                        and urllib.parse.urlsplit(request.url).path == "/api/platform/v1/auth/consents"
                        and request.method == "POST"):
                    headers = request.all_headers()
                    submitted_origin = headers.get("origin")
                    report["http"]["consent_request_origin_class"] = (
                        "core" if submitted_origin == core else
                        "ui" if submitted_origin == ui else
                        "null" if submitted_origin == "null" else
                        "missing" if submitted_origin is None else "other"
                    )
                    report["http"]["consent_request_has_core_cookie"] = any(
                        part.strip().startswith("__Host-xelab_auth=")
                        for part in headers.get("cookie", "").split(";")
                    )
                    fields = urllib.parse.parse_qsl(
                        request.post_data or "", keep_blank_values=True,
                        strict_parsing=True, encoding="utf-8", errors="strict",
                    )
                    csrf = [value for name, value in fields if name == "X-CSRF-Token"]
                    report["http"]["consent_form_has_csrf"] = len(csrf) == 1 and bool(csrf[0])
                    report["http"]["consent_form_csrf_length"] = len(csrf[0]) if len(csrf) == 1 else None
                    report["http"]["consent_form_exact_fields"] = (
                        len(fields) == 4 and {name for name, _ in fields} == {
                            "authorization_request_id", "approved", "X-CSRF-Token", "Idempotency-Key",
                        }
                    )
                if url_origin(request.url) != ui:
                    return
                payload = request.post_data or ""
                password = config["password"]
                if any(value in request.url or value in payload for value in (
                    password, urllib.parse.quote(password, safe=""), json.dumps(password),
                )):
                    network_isolation["ui_received_password"] = True
                cookie = request.all_headers().get("cookie", "")
                if any(part.strip().startswith("__Host-xelab_auth=") for part in cookie.split(";")):
                    network_isolation["ui_received_core_cookie"] = True
            except Exception:
                # A listener exception must not print a callback URL/credential
                # through Playwright's event emitter; fail the final check.
                network_isolation["inspection_failed"] = True

        def inspect_response(response) -> None:
            try:
                parsed = urllib.parse.urlsplit(response.url)
                if (url_origin(response.url) == core
                        and parsed.path == "/api/platform/v1/auth/consents"
                        and response.request.method == "POST"):
                    report["http"]["consent_submit_status"] = response.status
                    location = response.all_headers().get("location")
                    report["http"]["consent_location_is_ui"] = (
                        location is not None
                        and url_origin(urllib.parse.urljoin(core, location)) == ui
                    )
                    if response.status >= 400:
                        value = response.json()
                        detail = value.get("error") if isinstance(value, dict) else None
                        code = detail.get("code") if isinstance(detail, dict) else None
                        if code in {
                            "PERMISSION_DENIED", "INVALID_CREDENTIAL", "INVALID_ARGUMENT",
                            "RESOURCE_NOT_FOUND", "NOT_READY", "DEADLINE_EXCEEDED",
                            "ONE_TIME_RESPONSE_UNAVAILABLE", "IDEMPOTENCY_CONFLICT",
                            "STATE_CONFLICT", "PAYLOAD_TOO_LARGE", "UNSUPPORTED_MEDIA_TYPE",
                            "RESOURCE_EXHAUSTED",
                        }:
                            report["http"]["consent_error_code"] = code
            except Exception:
                report["http"]["diagnostic_observer_failed"] = True

        def inspect_console(message) -> None:
            try:
                if message.type != "error":
                    return
                # Chromium's message contains complete navigation URLs. Retain
                # only these two diagnostic booleans, never the message itself.
                value = message.text.lower()
                if "form-action" in value:
                    report["http"]["console_form_action_violation"] = True
                if "content security policy" in value or "csp" in value:
                    report["http"]["console_csp_violation"] = True
            except Exception:
                report["http"]["diagnostic_observer_failed"] = True

        page.on("request", inspect_request)
        page.on("response", inspect_response)
        page.on("console", inspect_console)
        report["step"] = "trusted_login_navigation"
        response = page.goto(authorize, wait_until="domcontentloaded")
        require(response is not None and response.status == 200, "trusted_login_http_200")
        require(url_origin(page.url) == core, "password_page_core_origin")
        require(page.evaluate("isSecureContext"), "trusted_login_secure_context")
        headers = response.all_headers()
        require("no-store" in headers.get("cache-control", ""), "trusted_login_no_store")
        require(headers.get("referrer-policy") == "no-referrer", "trusted_login_no_referrer")
        csp = headers.get("content-security-policy", "")
        require("frame-ancestors 'none'" in csp and "'unsafe-inline'" not in csp, "trusted_login_csp")
        require(page.locator("script[src]").count() == 0, "trusted_login_no_plugin_scripts")
        require(page.locator("#login").get_attribute("action") == "/api/platform/v1/auth/login", "trusted_login_fixed_action")
        require(page.locator("#password").get_attribute("type") == "password", "trusted_password_field")
        page.locator("#account").fill(config["account_id"])
        page.locator("#password").fill(config["password"])
        report["step"] = "trusted_login_submit"
        with page.expect_response(lambda item: item.url == core + "/api/platform/v1/auth/login") as login:
            page.locator("#login button[type=submit]").click()
        require(login.value.status == 200, "actual_password_login_http_200")
        page.locator("#consent").wait_for(state="visible")
        require(url_origin(page.url) == core, "trusted_consent_core_origin")
        require(page.locator("#consent").get_attribute("action") == "/api/platform/v1/auth/consents", "native_consent_fixed_action")
        require(page.locator("#consent").get_attribute("method").lower() == "post", "native_consent_post")
        require(page.locator('#consent input[name="X-CSRF-Token"]').count() == 1, "native_consent_request_csrf")
        sessions = [cookie for cookie in context.cookies([core]) if cookie["name"] == "__Host-xelab_auth"]
        require(len(sessions) == 1, "actual_core_session_cookie")
        session = sessions[0]
        require(session["secure"] and session["httpOnly"] and session["sameSite"] == "Lax"
                and session["domain"] == core_host and session["path"] == "/", "core_cookie_host_secure_httponly")
        report["step"] = "native_consent_submit"
        with page.expect_navigation(wait_until="domcontentloaded") as native_navigation:
            page.locator('#consent button[name="approved"][value="true"]').click()
        navigation_response = native_navigation.value
        report["http"]["native_navigation_status"] = (
            navigation_response.status if navigation_response is not None else None
        )
        expected = urllib.parse.urlsplit(config["redirect_uri"])
        callback = urllib.parse.urlsplit(page.url)
        current_origin = url_origin(page.url)
        report["http"]["callback_origin_class"] = (
            "chrome-error" if page.url.startswith("chrome-error:") else
            "core" if current_origin == core else
            "ui" if current_origin == ui else "other"
        )
        require(url_origin(page.url) == ui and callback.path == expected.path, "exact_registered_callback")
        pairs = urllib.parse.parse_qsl(callback.query, keep_blank_values=True, strict_parsing=True)
        require(len(pairs) == len(dict(pairs)), "callback_unique_fields")
        parameters = dict(pairs)
        existing = dict(urllib.parse.parse_qsl(expected.query, keep_blank_values=True, strict_parsing=True))
        require(parameters.keys() == existing.keys() | {"code", "state"}
                and all(parameters[key] == value for key, value in existing.items()), "callback_exact_fields")
        require(parameters.get("state") == state and bool(parameters.get("code")), "callback_state_and_code")
        code = parameters["code"]
        require(page.evaluate("isSecureContext"), "callback_secure_context")
        require(page.evaluate("document.referrer") == "", "callback_no_core_referrer")

        def fetch(fields=None, token=None, path="/oauth/token", method="POST") -> dict:
            require(url_origin(page.url) == ui, "token_client_remains_ui_origin")
            return page.evaluate(FETCH, {"url": core + path, "method": method, "fields": fields, "token": token})

        def rejected(result: dict, name: str) -> None:
            require(result.get("type") == "cors" and result.get("status") == 400
                    and isinstance(result.get("body"), dict)
                    and result["body"].get("error") == "invalid_grant", name)
            report["http"][name] = result["status"]

        fields = {"grant_type": "authorization_code", "client_id": config["client_id"],
                  "code": code, "redirect_uri": config["redirect_uri"], "code_verifier": verifier}
        report["step"] = "wrong_pkce"
        wrong = dict(fields, code_verifier=("A" if verifier[0] != "A" else "B") + verifier[1:])
        rejected(fetch(wrong), "wrong_pkce_rejected")
        report["step"] = "actual_browser_token_exchange"
        token_response = fetch(fields)
        require(token_response.get("type") == "cors" and token_response.get("status") == 200, "pkce_exchange_cors_http_200")
        tokens = token_response.get("body")
        require(isinstance(tokens, dict) and tokens.get("token_type") == "Bearer"
                and isinstance(tokens.get("access_token"), str) and bool(tokens["access_token"])
                and isinstance(tokens.get("refresh_token"), str) and bool(tokens["refresh_token"])
                and isinstance(tokens.get("expires_in"), int) and tokens["expires_in"] > 0,
                "actual_oauth_tokens_issued")
        report["http"]["token_exchange"] = token_response["status"]
        report["step"] = "authorization_code_replay"
        rejected(fetch(fields), "authorization_code_replay_rejected")

        def read_account(access: str, name: str) -> dict:
            result = fetch(token=access, path=config["api_read_path"], method="GET")
            require(result.get("type") == "cors" and result.get("status") == 200, name)
            require(isinstance(result.get("body"), dict)
                    and result["body"].get("id") == config["expected_account_id"], name + "_subject")
            report["http"][name] = result["status"]
            return result

        report["step"] = "actual_authorized_account_read"
        read_account(tokens["access_token"], "actual_account_cors_read")
        report["step"] = "actual_authorized_account_query_read"
        # The Authorization header makes this a native browser preflight to
        # the exact queried URL; a no-query resource read cannot cover it.
        listed = fetch(token=tokens["access_token"], path="/api/platform/v1/accounts?limit=1", method="GET")
        report["http"]["actual_account_query_cors_read"] = listed.get("status")
        body = listed.get("body")
        require(listed.get("type") == "cors" and listed.get("status") == 200
                and isinstance(body, dict) and isinstance(body.get("items"), list)
                and len(body["items"]) == 1 and isinstance(body["items"][0], dict)
                and body["items"][0].get("id") == config["expected_account_id"],
                "actual_account_query_cors_read")
        report["step"] = "refresh_rotation"
        refresh_fields = {"grant_type": "refresh_token", "client_id": config["client_id"],
                          "refresh_token": tokens["refresh_token"]}
        rotated = fetch(refresh_fields)
        require(rotated.get("type") == "cors" and rotated.get("status") == 200, "refresh_rotation_http_200")
        replacement = rotated.get("body")
        require(isinstance(replacement, dict) and replacement.get("token_type") == "Bearer"
                and isinstance(replacement.get("access_token"), str)
                and replacement["access_token"] != tokens["access_token"]
                and isinstance(replacement.get("refresh_token"), str)
                and replacement["refresh_token"] != tokens["refresh_token"], "refresh_credentials_rotated")
        read_account(replacement["access_token"], "rotated_account_cors_read")
        report["step"] = "refresh_reuse"
        rejected(fetch(refresh_fields), "refresh_reuse_rejected")
        rejected(fetch(dict(refresh_fields, refresh_token=replacement["refresh_token"])), "refresh_family_revoked")
        result = fetch(token=replacement["access_token"], path=config["api_read_path"], method="GET")
        require(result.get("status") in (401, 403) or result.get("transport_error") is True,
                "reused_family_access_denied")
        report["http"]["revoked_access"] = result.get("status")
        require(not any(cookie["name"] == "__Host-xelab_auth" for cookie in context.cookies([ui])), "ui_has_no_core_session_cookie")
        require(page.evaluate("!document.cookie.split(';').some(v => v.trim().startsWith('__Host-xelab_auth='))"), "ui_cannot_read_core_cookie")
        require(page.evaluate("localStorage.length === 0 && sessionStorage.length === 0"), "no_credential_browser_storage")
        require(not network_isolation["ui_received_password"], "password_never_sent_to_ui")
        require(not network_isolation["ui_received_core_cookie"], "core_cookie_never_sent_to_ui")
        require(not network_isolation["inspection_failed"], "ui_request_isolation_observed")
        context.close()
        browser.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    arguments = parser.parse_args()
    report = {"status": "failed", "step": "fixture", "checks": {}, "http": {}}
    started = time.monotonic()
    try:
        run(fixture(arguments.fixture), report)
        report["status"] = "passed"
        report["step"] = "complete"
    except Exception as error:
        # Playwright errors can contain callback URLs or form values. Do not
        # print exception messages, stack traces, browser logs or response bodies.
        report["error_type"] = type(error).__name__
    report["elapsed_seconds"] = round(time.monotonic() - started, 3)
    raw = (json.dumps(report, sort_keys=True) + "\n").encode()
    descriptor = os.open(arguments.output, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as result:
        result.write(raw)
    sys.stdout.write(raw.decode())
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
