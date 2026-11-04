#!/usr/bin/env python3
"""Real signed OCI WebUI/iframe browser acceptance against the actual kernel.

The Rust coordinator owns the database, runtime and public API mutations. This
driver mocks no response, installs no Cookie and injects no session. Passwords
are entered only on the trusted core origin. Reports never contain credentials,
callback query strings, form bodies, browser exception text or stack traces.
"""
from __future__ import annotations

import argparse
import base64
import importlib.metadata
import ipaddress
import json
import os
import pathlib
import re
import stat
import sys
import time
import urllib.parse
import uuid


class CheckFailed(Exception):
    pass


def future_features_unavailable(window):
    state = window.locator('[data-kind="unavailable"]')
    return (state.count() == 1
            and state.get_by_role("heading", name="Batch / Workflow / 微插件 / 知识库尚不可用", exact=True).count() == 1
            and state.get_by_text("未安装或实现的后续执行系统没有可展示的运行结果。", exact=True).count() == 1)


def close_windows(page, report):
    report["step"] = "desktop_close_actual_windows"
    for _ in range(16):
        if not page.locator("#windows section.window").count():
            report["checks"]["all_windows_close"] = True
            return
        focused = page.locator('#windows section.window[data-focused="true"]:visible')
        if not focused.count():
            # A minimized window must be restored through its real Dock control.
            page.locator('#dock button[aria-label^="恢复"]').last.click()
        # Background controls may legitimately be covered by a newer window.
        # Close the current foreground window using its actual UI control.
        focused.last.locator('.window-controls button[aria-label^="关闭"]').click()
    if page.locator("#windows section.window").count():
        raise CheckFailed("all_windows_close")
    report["checks"]["all_windows_close"] = True


def url_origin(value):
    parsed = urllib.parse.urlsplit(value)
    return f"{parsed.scheme}://{parsed.netloc}"


def safe_read_summary(status, value):
    """Keep only bounded protocol facts, never response data or error text."""
    result = {"status": status}
    if isinstance(value, dict):
        if 200 <= status < 300 and isinstance(value.get("items"), list):
            result["item_count"] = len(value["items"])
        error = value.get("error")
        code = error.get("code") if isinstance(error, dict) else None
        if status >= 400 and isinstance(code, str) and re.fullmatch(r"[A-Z][A-Z0-9_]{0,63}", code):
            result["error_code"] = code
    return result


def fixture(path):
    metadata = path.lstat()
    if (not stat.S_ISREG(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o600
            or metadata.st_uid != os.geteuid() or metadata.st_nlink != 1):
        raise CheckFailed("private_fixture_required")
    value = json.loads(path.read_text())
    required = {"core_origin", "ui_origin", "reference_origin", "client_id", "owner_id", "owner_password", "child_id", "child_password", "skill_id", "reference_deployment_id", "tls_spki_sha256", "connect_ip", "chromium_path", "progress_file", "control_file"}
    if not isinstance(value, dict) or value.keys() != required or any(not isinstance(item, str) or not item for item in value.values()):
        raise CheckFailed("fixture_fields")
    for name in ("core_origin", "ui_origin", "reference_origin"):
        parsed = urllib.parse.urlsplit(value[name])
        if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment or url_origin(value[name]) != value[name]:
            raise CheckFailed("exact_https_origin_required")
        parsed.port
    if len({urllib.parse.urlsplit(value[name]).hostname for name in ("core_origin", "ui_origin", "reference_origin")}) != 3:
        raise CheckFailed("isolated_hosts_required")
    for name in ("owner_id", "child_id", "skill_id", "reference_deployment_id"):
        if str(uuid.UUID(value[name])) != value[name]:
            raise CheckFailed("fixture_uuid")
    if value["owner_id"] == value["child_id"] or not re.fullmatch(r"[A-Za-z0-9._~-]{1,128}", value["client_id"]):
        raise CheckFailed("fixture_identity")
    if any(not 12 <= len(value[name]) <= 128 for name in ("owner_password", "child_password")):
        raise CheckFailed("fixture_password_length")
    if len(base64.b64decode(value["tls_spki_sha256"], validate=True)) != 32 or not ipaddress.ip_address(value["connect_ip"]).is_loopback:
        raise CheckFailed("fixture_local_certificate_pin")
    for name in ("progress_file", "control_file"):
        candidate = pathlib.Path(value[name])
        if candidate.parent.resolve() != path.parent.resolve() or candidate.is_symlink():
            raise CheckFailed("private_control_directory_required")
    return value


def private_json(path, value):
    path = pathlib.Path(path)
    temporary = path.with_name(path.name + ".tmp-" + uuid.uuid4().hex)
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(value, output, sort_keys=True)
    os.replace(temporary, path)


FETCH = """async ({core,client,path,method,body,key}) => {
  const session=JSON.parse(sessionStorage.getItem(`xelab.oauth:${core}:${client}`));
  if(!session?.access_token)return {status:null,code:'NO_SESSION'};
  const options={method,credentials:'omit',cache:'no-store',redirect:'error',headers:{Authorization:'Bearer '+session.access_token,Accept:'application/json'}};
  if(body!==null){options.headers['Content-Type']='application/json';options.headers['Idempotency-Key']=key;options.body=JSON.stringify(body);}
  const response=await fetch(core+path,options);const value=await response.json();
  return {status:response.status,code:value?.error?.code??null};
}"""


def run(config, report):
    os.environ.pop("DEBUG", None)
    os.environ.pop("PWDEBUG", None)
    from playwright.sync_api import sync_playwright

    def require(condition, name):
        report["step"] = name
        if not condition:
            raise CheckFailed(name)
        report["checks"][name] = True

    core, ui, reference = (config[name] for name in ("core_origin", "ui_origin", "reference_origin"))
    entry = ui + "/ui/org.xelab.webui/"
    reference_base = reference + "/ui/org.xelab.ui-reference/"
    hosts = [urllib.parse.urlsplit(value).hostname for value in (core, ui, reference)]
    resolver = ",".join(f"MAP {host} {config['connect_ip']}" for host in hosts)
    report["playwright_version"] = importlib.metadata.version("playwright")
    report["http"] = {"oauth_token_successes": 0, "callback_successes": 0, "actual_skill_successes": 0, "skill_post_attempts": 0, "node_reads": 0, "preference_writes": 0, "contribution_reads": 0, "read_statuses": {}, "preflight_statuses": {}}
    reads = {"/api/platform/v1/installations": "installation_list",
             "/api/platform/v1/deployments": "deployment_list",
             "/api/platform/v1/skills": "skill_list",
             "/api/platform/v1/sessions": "current_sessions",
             "/api/platform/v1/me/ui-preferences": "current_preferences",
             "/api/platform/v1/ui/contributions": "contribution_list",
             f"/api/platform/v1/accounts/{config['owner_id']}": "owner_account",
             f"/api/platform/v1/accounts/{config['child_id']}": "child_account"}
    observed = {"password_outside_core": False, "core_cookie_on_plugin": False, "observer_failure": False, "page_errors": 0}
    calls = []

    def observe_request(request):
        try:
            location = url_origin(request.url)
            headers = request.all_headers()
            if location == core and urllib.parse.urlsplit(request.url).path == f"/api/platform/v1/skills/{config['skill_id']}/calls" and request.method == "POST":
                # Count admission attempts even if they fail, remain pending,
                # time out, or never receive a successful HTTP response.
                report["http"]["skill_post_attempts"] += 1
            if location in (ui, reference) and "__Host-xelab_auth=" in headers.get("cookie", ""):
                observed["core_cookie_on_plugin"] = True
            if request.post_data and "password" in request.post_data and location != core:
                observed["password_outside_core"] = True
        except Exception:
            observed["observer_failure"] = True

    def observe_response(response):
        try:
            parsed = urllib.parse.urlsplit(response.url)
            if url_origin(response.url) == ui and parsed.path == "/ui/org.xelab.webui/callback" and response.status == 200:
                report["http"]["callback_successes"] += 1
            if url_origin(response.url) != core:
                return
            if response.request.method in ("GET", "OPTIONS") and parsed.path in reads:
                try:
                    value = response.json()
                except Exception:
                    value = None
                bucket = "preflight_statuses" if response.request.method == "OPTIONS" else "read_statuses"
                entries = report["http"][bucket].setdefault(reads[parsed.path], [])
                entries.append(safe_read_summary(response.status, value))
                del entries[:-8]
            if parsed.path == "/oauth/token" and response.status == 200:
                report["http"]["oauth_token_successes"] += 1
            elif parsed.path == "/api/platform/v1/node" and response.status == 200:
                report["http"]["node_reads"] += 1
            elif parsed.path == "/api/platform/v1/me/ui-preferences" and response.request.method == "PUT" and response.status == 200:
                report["http"]["preference_writes"] += 1
            elif "/ui/contributions" in parsed.path and response.status == 200:
                report["http"]["contribution_reads"] += 1
            elif parsed.path == f"/api/platform/v1/skills/{config['skill_id']}/calls" and response.request.method == "POST" and response.status == 200:
                value = response.json()
                call = value.get("call", {})
                if value.get("skill_id") == config["skill_id"] and isinstance(call.get("call_id"), str):
                    calls.append(call["call_id"])
                    report["http"]["actual_skill_successes"] += 1
        except Exception:
            observed["observer_failure"] = True

    def context(browser):
        result = browser.new_context(viewport={"width": 1440, "height": 1000})
        result.on("request", observe_request)
        result.on("response", observe_response)
        result.on("page", lambda page: page.on("pageerror", lambda _: observed.__setitem__("page_errors", observed["page_errors"] + 1)))
        return result

    def login(browser, account, password, label):
        report["step"] = label + "_default_ui_navigation"
        ctx = context(browser)
        page = ctx.new_page()
        page.set_default_timeout(25000)
        response = page.goto(core + "/", wait_until="domcontentloaded")
        require(response is not None and response.status == 200 and url_origin(page.url) == ui, label + "_root_redirects_to_actual_oci_ui")
        page.locator("#welcome-login").click()
        page.locator("#account").wait_for()
        require(url_origin(page.url) == core and urllib.parse.urlsplit(page.url).path == "/auth/ui/login", label + "_password_only_on_trusted_core")
        page.locator("#account").fill(account)
        page.locator("#password").fill(password)
        page.locator("#submit").click()
        page.locator("#consent").wait_for()
        require(url_origin(page.url) == core, label + "_trusted_native_consent")
        require(page.locator('#consent input[name="X-CSRF-Token"]').count() == 1, label + "_request_specific_consent_csrf")
        page.locator('#consent button[name="approved"][value="true"]').click()
        page.wait_for_url(lambda url: url_origin(str(url)) == ui and urllib.parse.urlsplit(str(url)).path == "/ui/org.xelab.webui/")
        page.wait_for_function("() => document.querySelector('#login')?.textContent === '退出'")
        page.wait_for_function("() => document.querySelector('#connection')?.textContent === '已连接核心'")
        require(page.locator("#welcome").is_hidden(), label + "_actual_pkce_login_completed")
        cookies = ctx.cookies([core])
        session = next((cookie for cookie in cookies if cookie["name"] == "__Host-xelab_auth"), None)
        require(session is not None and session["httpOnly"] and session["secure"] and session["sameSite"] == "Lax" and session["path"] == "/" and session["domain"] == hosts[0], label + "_host_only_core_session_cookie")
        require(not any(cookie["name"] == "__Host-xelab_auth" for cookie in ctx.cookies([ui, reference])), label + "_no_core_cookie_on_plugin_hosts")
        return ctx, page

    def open_app(page, identifier, title, dock=True):
        report["step"] = "open_actual_" + identifier
        if dock:
            page.get_by_role("button", name="打开" + title, exact=True).click()
        else:
            page.get_by_role("button", name="打开应用启动器", exact=True).click()
            page.get_by_role("button", name="启动" + title, exact=True).click()
        result = page.locator(f'section.window[data-app="{identifier}"]').last
        result.wait_for()
        return result

    def contribution_frame(page, identifier="echo_window"):
        report["step"] = "mount_actual_" + identifier
        dialog = open_app(page, "contributions", "插件页面", dock=False)
        title = "Reference echo" if identifier == "echo_window" else "Echo widget"
        card = dialog.locator("article.card").filter(has=page.get_by_role("heading", name=title, exact=True))
        card.wait_for()
        filename = "window.html" if identifier == "echo_window" else "widget.html"
        with page.expect_event("framenavigated", predicate=lambda frame: frame.url.startswith(reference_base + filename)) as navigation:
            card.get_by_role("button", name="打开隔离窗口" if identifier == "echo_window" else "添加桌面组件", exact=True).click()
        frame = navigation.value
        frame.locator("#call").wait_for(state="visible")
        frame.wait_for_function("() => document.querySelector('#call')?.disabled === false")
        if identifier == "echo_widget":
            close_windows(page, report)
        require(url_origin(frame.url) == reference, identifier + "_actual_isolated_origin")
        require(frame.evaluate("() => { try { parent.document.body; return false; } catch { return true; } }"), identifier + "_parent_dom_blocked")
        require(frame.evaluate("() => { try { parent.sessionStorage.length; return false; } catch { return true; } }"), identifier + "_parent_credential_storage_blocked")
        return frame

    def echo(frame, message, label):
        report["step"] = label
        frame.locator("#message").fill(message)
        frame.locator("#call").click()
        frame.wait_for_function("() => document.querySelector('#state')?.textContent === 'Skill call completed.'")
        require(frame.locator("#result").inner_text() == message, label)
        require(frame.locator("img").count() == 0, label + "_text_not_html")

    def fetch(page, path, method="GET", body=None):
        return page.evaluate(FETCH, {"core": core, "client": config["client_id"], "path": path, "method": method, "body": body, "key": str(uuid.uuid4())})

    def coordinator_barrier(page, phase, expected):
        private_json(config["progress_file"], {"phase": phase})
        report["step"] = phase
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            path = pathlib.Path(config["control_file"])
            if path.exists():
                metadata = path.lstat()
                require(stat.S_ISREG(metadata.st_mode) and stat.S_IMODE(metadata.st_mode) == 0o600
                        and metadata.st_uid == os.geteuid() and metadata.st_nlink == 1, "private_coordinator_control")
                control = json.loads(path.read_text())
                if control.get("phase") == expected:
                    return control
            page.wait_for_timeout(50)
        raise CheckFailed("coordinator_barrier_not_completed")

    with sync_playwright() as playwright:
        report["step"] = "launch_actual_chromium"
        browser = playwright.chromium.launch(executable_path=config["chromium_path"], headless=True, args=["--no-sandbox", "--disable-dev-shm-usage", "--no-proxy-server", "--host-resolver-rules=" + resolver, "--ignore-certificate-errors-spki-list=" + config["tls_spki_sha256"]])
        report["browser_version"] = browser.version
        owner_context, page = login(browser, config["owner_id"], config["owner_password"], "owner")
        resources = open_app(page, "resources", "系统状态")
        resources.locator("pre.result").wait_for()
        require('"kernel_cpu_millis": 4000' in resources.inner_text() and '"kernel_memory_bytes": 17179869184' in resources.inner_text(), "desktop_reads_actual_node_reservations")
        close_windows(page, report)
        plugins = open_app(page, "plugins", "插件中心")
        report["step"] = "desktop_wait_actual_plugin_catalog"
        plugins.get_by_role("heading", name="M3 official WebUI", exact=True).wait_for()
        plugins.get_by_role("heading", name="M3 independent UI reference", exact=True).wait_for()
        require("org.xelab.webui" in plugins.inner_text() and "org.xelab.ui-reference" in plugins.inner_text(), "desktop_reads_actual_installed_plugins")
        close_windows(page, report)
        accounts = open_app(page, "accounts", "账户与权限")
        accounts.locator("pre.result").wait_for()
        require(config["owner_id"] in accounts.inner_text(), "desktop_reads_actual_owner_identity")
        close_windows(page, report)
        skills = open_app(page, "skills", "Skill")
        card = skills.locator("article.card").filter(has=page.get_by_role("heading", name="org.xelab.ui-reference.echo", exact=True))
        card.get_by_role("button", name="调用测试", exact=True).click()
        editor = page.locator('section.window[data-app="editor-invokeSkill"]').last
        intent = str(uuid.uuid4())
        editor.get_by_role("textbox", name="请求 JSON", exact=True).fill(json.dumps({"input": {"message": "Actual management UI Skill call"}, "provider_deployment_id": config["reference_deployment_id"], "idempotency_key": intent, "timeout_ms": 10000}))
        editor.get_by_role("button", name="提交", exact=True).click()
        editor.locator("pre.result").wait_for()
        require("Actual management UI Skill call" in editor.inner_text(), "desktop_invokes_actual_fixed_skill")
        close_windows(page, report)
        settings = open_app(page, "settings", "设置")
        settings.get_by_role("combobox", name="主题模式", exact=True).select_option("dark")
        settings.get_by_role("checkbox", name="减少动画", exact=True).check()
        settings.get_by_role("button", name="保存到当前账户", exact=True).click()
        page.wait_for_function("() => [...document.querySelectorAll('.theme-preview-note')].some(value => value.textContent.includes('已保存到核心'))")
        close_windows(page, report)
        settings = open_app(page, "settings", "设置")
        require(settings.get_by_role("combobox", name="主题模式", exact=True).input_value() == "dark" and settings.get_by_role("checkbox", name="减少动画", exact=True).is_checked(), "actual_theme_preferences_survive_reread")
        close_windows(page, report)
        unavailable = open_app(page, "tasks", "任务与知识库", dock=False)
        require(future_features_unavailable(unavailable), "future_execution_features_remain_unavailable")
        close_windows(page, report)
        window = contribution_frame(page)
        echo(window, "Actual OCI iframe <img src=x onerror=alert(1)>", "window_bridge_calls_real_http_skill")
        report["step"] = "real_channel_nonce_replay_and_source_rejection"
        coordinator_barrier(page, "await_attack_snapshot", "attack_snapshot_ready")
        before = report["http"]["skill_post_attempts"]
        channel = window.evaluate("() => Object.fromEntries(new URLSearchParams(location.hash.slice(1)))")
        window.evaluate("""() => { const meta=Object.fromEntries(new URLSearchParams(location.hash.slice(1)));
          const envelope={protocol:meta.protocol,view_id:meta.view_id,channel_nonce:meta.channel_nonce,type:'request',request_id:2,method:'skill.call',params:{binding_alias:'echo',input:{message:'Must not execute channel replay'},idempotency_key:'forged-channel-intent',timeout_ms:10000}};
          parent.postMessage({...envelope,seq:3,channel_nonce:(meta.channel_nonce[0]==='0'?'1':'0')+meta.channel_nonce.slice(1)},meta.parent_origin);
          parent.postMessage({...envelope,seq:2},meta.parent_origin);
          parent.postMessage({...envelope,seq:3,protocol:'xelab.ui/2'},meta.parent_origin);
        }""")
        page.evaluate("""meta => window.postMessage({protocol:meta.protocol,view_id:meta.view_id,channel_nonce:meta.channel_nonce,seq:3,type:'request',request_id:2,method:'skill.call',params:{binding_alias:'echo',input:{message:'Wrong source must not execute'},idempotency_key:'forged-host-source',timeout_ms:10000}},location.origin)""", channel)
        # An actual sibling on the SAME plugin origin isolates event.source
        # validation from the separate wrong-origin check above.
        with page.expect_event("framenavigated", predicate=lambda frame: frame.url.startswith(reference_base + "widget.html")) as sibling_navigation:
            page.evaluate("""({metadata,url}) => {const frame=document.createElement('iframe');frame.id='source-probe';frame.hidden=true;frame.setAttribute('sandbox','allow-scripts allow-same-origin');frame.src=url+'#'+new URLSearchParams(metadata);document.body.append(frame);}""", {"metadata": channel, "url": reference_base + "widget.html"})
        sibling = sibling_navigation.value
        sibling.locator("#state").wait_for(state="attached")
        sibling.evaluate("""meta => parent.postMessage({protocol:meta.protocol,view_id:meta.view_id,channel_nonce:meta.channel_nonce,seq:3,type:'request',request_id:2,method:'skill.call',params:{binding_alias:'echo',input:{message:'Same origin wrong source must not execute'},idempotency_key:'forged-sibling-source',timeout_ms:10000}},meta.parent_origin)""", channel)
        # A subsequent legitimate request/response is the event and network
        # barrier; elapsed silence and successful-only counters are insufficient.
        echo(window, "Actual channel survives rejected messages", "valid_call_after_replay_rejection")
        page.locator("#source-probe").evaluate("element => element.remove()")
        require(report["http"]["skill_post_attempts"] == before + 1, "channel_attacks_create_no_skill_post_attempt")
        proof = coordinator_barrier(page, "await_attack_verification", "attack_verified")
        require(proof.get("calls_delta") == 1 and proof.get("daily_delta") == 1 and proof.get("concurrent") == 0,
                "channel_attacks_create_no_call_or_quota_charge")
        require(window.evaluate("async core => { try { await fetch(core+'/api/platform/v1/node'); return false; } catch { return true; } }", core), "third_party_connect_src_none")
        close_windows(page, report)
        widget = contribution_frame(page, "echo_widget")
        echo(widget, "Actual OCI widget Skill call", "widget_bridge_calls_real_http_skill")
        with page.expect_event("framenavigated", predicate=lambda frame: frame.url.startswith(reference_base + "window.html")) as opened:
            widget.locator("#open-window").click()
        require(url_origin(opened.value.url) == reference, "widget_opens_same_fixed_deployment_window")
        require(report["http"]["actual_skill_successes"] >= 3, "owner_actual_skill_requests_observed")
        require(report["http"]["preference_writes"] >= 2, "core_preferences_and_widget_layout_persisted")
        owner_call = calls[0]
        owner_context.close()

        child_context, page = login(browser, config["child_id"], config["child_password"], "child")
        require(fetch(page, f"/api/platform/v1/accounts/{config['owner_id']}")["status"] in (403, 404), "child_cannot_read_owner_account")
        require(fetch(page, "/api/platform/v1/calls/" + owner_call)["status"] in (403, 404), "child_cannot_read_owner_call_result")
        frame = contribution_frame(page)
        echo(frame, "Actual limited child Skill call", "limited_child_bridge_call_before_withdrawal")
        require(coordinator_barrier(page, "await_revoke", "revoked").get("revoked") is True, "actual_inheritance_withdrawal_acknowledged")
        report["step"] = "actual_revocation_closes_old_channel"
        page.wait_for_function("() => document.querySelectorAll('#windows iframe').length === 0", timeout=15000)
        require(page.get_by_text("页面通道已关闭", exact=True).count() >= 1, "withdrawal_invalidates_old_iframe_channel")
        before = report["http"]["actual_skill_successes"]
        denied = fetch(page, f"/api/platform/v1/skills/{config['skill_id']}/calls", "POST", {"input": {"message": "Must be denied after real withdrawal"}, "provider_deployment_id": config["reference_deployment_id"], "idempotency_key": str(uuid.uuid4()), "timeout_ms": 10000})
        require(denied["status"] == 403 and denied["code"] == "PERMISSION_DENIED", "withdrawn_child_new_skill_call_denied")
        require(report["http"]["actual_skill_successes"] == before, "withdrawal_produces_no_success_result")
        child_context.close()
        require(report["http"]["oauth_token_successes"] >= 2 and report["http"]["callback_successes"] >= 2, "actual_owner_and_child_code_pkce_exchanges")
        require(not observed["password_outside_core"] and not observed["core_cookie_on_plugin"] and not observed["observer_failure"], "password_cookie_and_network_observation_boundaries")
        require(observed["page_errors"] == 0, "no_uncaught_browser_execution_error")
        browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    report = {"status": "failed", "step": "private_fixture", "checks": {}, "http": {}}
    started = time.monotonic()
    try:
        run(fixture(args.fixture), report)
        report["status"], report["step"] = "passed", "complete"
    except Exception as error:
        report["error_type"] = type(error).__name__
    report["elapsed_seconds"] = round(time.monotonic() - started, 3)
    private_json(args.output, report)
    print(json.dumps(report, sort_keys=True))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
