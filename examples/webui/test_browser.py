#!/usr/bin/env python3
"""Real Chromium component tests; API fixtures are not core/OCI acceptance."""
import json
import os
import base64
import hashlib
import pathlib
import unittest
from urllib.parse import parse_qs, urlencode, urlparse

from playwright.sync_api import sync_playwright
from test_bridge import contribution, FRAME, PLUGIN

PUBLIC = pathlib.Path(__file__).parent / "public"
SHELL = "https://shell.example.test"
CORE = "https://core.example.test"
BASE = "/ui/org.xelab.webui/"
ACCOUNT = "00000000-0000-4000-8000-000000000001"
SKILL = "00000000-0000-4000-8000-000000000002"
DEPLOYMENT = "00000000-0000-4000-8000-000000000003"
NOW = "2026-11-04T00:00:00Z"


def preferences():
    return {"id": ACCOUNT, "owner_id": ACCOUNT, "revision": 1,
            "created_at": NOW, "updated_at": NOW,
            "spec": {"theme": {"mode": "system", "reduce_motion": False, "reduce_transparency": False},
                     "dock": {"position": "bottom", "auto_hide": False, "pinned": []},
                     "widgets": [], "restore_windows": True}}


class DesktopBrowser(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.playwright = sync_playwright().start()
        cls.browser = cls.playwright.chromium.launch(
            executable_path=os.environ.get("XELAB_OAUTH_BROWSER_CHROMIUM", "/usr/bin/chromium"), headless=True,
            args=["--no-sandbox", "--disable-dev-shm-usage"])

    @classmethod
    def tearDownClass(cls):
        cls.browser.close()
        cls.playwright.stop()

    def setUp(self):
        self.context = self.browser.new_context(viewport={"width": 1280, "height": 900})
        self.page = self.context.new_page()
        self.calls = []
        self.saved = preferences()
        self.denied = set()
        self.contributions = {}
        self.call_result = None
        self.status_overrides = {}
        self.refresh_enabled = False
        self.abort_business = False
        self.errors = []
        self.expected_challenge = None
        self.page.on("pageerror", lambda error: self.errors.append(str(error)))
        self.page.route("**/*", self.route)

    def tearDown(self):
        self.assertEqual(self.errors, [], "browser JavaScript errors")
        self.context.close()

    def route(self, route):
        request = route.request
        url = urlparse(request.url)
        if f"{url.scheme}://{url.netloc}" == SHELL:
            file = url.path.removeprefix(BASE)
            if file in ("", "callback"):
                file = "index.html"
            if file == "runtime-config.js":
                route.fulfill(status=200, content_type="text/javascript", body="export const runtimeConfig=" + json.dumps({"core_origin": CORE, "client_id": "browser-component", "max_open_windows": 16}) + ";")
                return
            if file not in {"index.html", "app.js", "styles.css", "api.js", "oauth.js", "bridge.js"}:
                route.fulfill(status=404, body="not found")
                return
            types = {".html": "text/html", ".css": "text/css", ".js": "text/javascript"}
            route.fulfill(status=200, content_type=types[pathlib.Path(file).suffix], body=(PUBLIC / file).read_bytes())
            return
        if f"{url.scheme}://{url.netloc}" == PLUGIN:
            route.fulfill(status=200, content_type="text/html", headers={"content-security-policy": f"default-src 'none'; script-src 'unsafe-inline'; connect-src 'none'; frame-ancestors {SHELL}"}, body=FRAME)
            return
        if f"{url.scheme}://{url.netloc}" != CORE:
            route.abort()
            return
        headers = {"access-control-allow-origin": SHELL,
                   "access-control-allow-headers": "authorization,content-type,if-match,idempotency-key",
                   "access-control-allow-methods": "GET,POST,PUT,DELETE,PATCH,OPTIONS",
                   "access-control-expose-headers": "etag,x-request-id",
                   "etag": '"rev-1"', "x-request-id": ACCOUNT}
        if request.method == "OPTIONS":
            route.fulfill(status=204, headers=headers)
            return
        self.calls.append({"method": request.method, "path": url.path,
                           "headers": request.all_headers(), "body": request.post_data})
        status = 200
        if url.path == "/oauth/token":
            fields = parse_qs(request.post_data or "", keep_blank_values=True)
            if self.refresh_enabled and fields == {"grant_type": ["refresh_token"], "client_id": ["browser-component"], "refresh_token": ["component-refresh"]}:
                route.fulfill(status=200, headers=headers, content_type="application/json", body=json.dumps({"access_token": "component-rotated-access", "refresh_token": "component-rotated-refresh", "token_type": "Bearer", "expires_in": 900, "scope": "client:browser-component"}))
                return
            verifier = fields.get("code_verifier", [""])[0]
            challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
            if (request.method != "POST" or set(fields) != {"grant_type", "client_id", "code", "redirect_uri", "code_verifier"}
                    or any(len(values) != 1 for values in fields.values())
                    or fields["grant_type"] != ["authorization_code"] or fields["client_id"] != ["browser-component"]
                    or fields["code"] != ["component-authorization-code"] or fields["redirect_uri"] != [SHELL + BASE + "callback"]
                    or not self.expected_challenge or challenge != self.expected_challenge):
                status, data = 400, {"error": "invalid_grant"}
            else:
                data = {"access_token": "component-exchanged-token", "token_type": "Bearer", "expires_in": 900,
                        "scope": "client:browser-component"}
        elif url.path in self.status_overrides:
            status, data = self.status_overrides[url.path], {"error": {"code": "INVALID_CREDENTIAL", "message": "fixture credential rejected", "retryable": False}}
        elif url.path in self.denied:
            status, data = 403, {"error": {"code": "PERMISSION_DENIED", "message": "fixture denied", "retryable": False}}
        elif url.path.endswith("/sessions"):
            data = {"items": [{"account_id": ACCOUNT, "status": "active"}], "next_cursor": None}
        elif url.path.endswith("/me/ui-preferences"):
            if request.method == "PUT":
                self.saved["spec"] = json.loads(request.post_data)
                self.saved["revision"] += 1
            headers["etag"] = f'"rev-{self.saved["revision"]}"'
            data = self.saved
        elif url.path == "/api/platform/v1/ui/contributions":
            data = {"items": list(self.contributions.values()), "next_cursor": None}
        elif "/ui/contributions/" in url.path and url.path.split("/")[-1] in self.contributions:
            data = self.contributions[url.path.split("/")[-1]]
        elif url.path.endswith("/skills"):
            data = {"items": [{"id": SKILL, "revision": 1, "enabled": True,
                              "definition_digest": "sha256:" + "a" * 64,
                              "availability": "ready", "reason_codes": [],
                              "definition": {"name": "component.echo", "version": "1.0.0"}}], "next_cursor": None}
        elif url.path.endswith(f"/skills/{SKILL}/calls"):
            if self.abort_business:
                route.abort("failed")
                return
            data = {"skill_id": SKILL, "skill_version": "1.0.0", "definition_digest": "sha256:" + "a" * 64,
                    "call": {"call_id": DEPLOYMENT, "contract": {"name": "component.echo", "version": "1.0.0",
                             "schema_digest": "sha256:" + "b" * 64}, "provider_instance_id": ACCOUNT,
                             "output": json.loads(request.post_data)["input"], "completed_at": NOW,
                             "skill_id": SKILL, "skill_definition_digest": "sha256:" + "a" * 64}}
            self.call_result = data["call"]
        elif url.path == f"/api/platform/v1/calls/{DEPLOYMENT}":
            data = {"call_id": DEPLOYMENT, "state": "succeeded", "result": self.call_result, "error": None}
        elif url.path.endswith("/deployments"):
            data = {"items": [{"id": DEPLOYMENT, "revision": 1, "phase": "disabled", "conditions": [],
                              "spec": {"display_name": "fixture deployment", "enabled": False}}], "next_cursor": None}
        elif url.path == f"/api/platform/v1/deployments/{DEPLOYMENT}" and request.method == "PUT":
            body = json.loads(request.post_data)
            if set(body) != {"spec"}:
                status, data = 400, {"error": {"code": "INVALID_ARGUMENT", "message": "DeploymentPatch requires spec", "retryable": False}}
            else:
                status, data = 202, {"id": ACCOUNT, "state": "pending", "kind": "update_deployment"}
        elif url.path.endswith("/installations"):
            data = {"items": [{"id": ACCOUNT, "plugin_id": '<img src=x onerror="alert(1)">',
                              "version": "1.0.0", "state": "installed", "image_digest": "sha256:" + "b" * 64}], "next_cursor": None}
        elif url.path.endswith("/node"):
            data = {"fixture": "component-only", "state": "ready"}
        else:
            status, data = 503, {"error": {"code": "NOT_READY", "message": "component fixture has no implementation", "retryable": False}}
        route.fulfill(status=status, headers=headers, content_type="application/json", body=json.dumps(data))

    def open(self, authenticated=True, expires_ms=900000, refresh_token=None):
        if authenticated:
            self.context.add_init_script("""if (location.origin === 'https://shell.example.test') {
              sessionStorage.setItem('xelab.oauth:https://core.example.test:browser-component', JSON.stringify({
                core_origin:'https://core.example.test',client_id:'browser-component',
                access_token:'component-token',refresh_token:REFRESH_VALUE,expires_at:Date.now()+EXPIRY_VALUE,
                scope:'client:browser-component'}));
            }""".replace("REFRESH_VALUE", json.dumps(refresh_token)).replace("EXPIRY_VALUE", str(expires_ms)))
        self.page.goto(SHELL + BASE)
        self.page.locator(".dock button").first.wait_for()
        if authenticated:
            self.page.get_by_role("button", name="退出", exact=True).wait_for()
            self.page.wait_for_function("document.querySelector('#connection').textContent === '已连接核心'")

    def app(self, name):
        self.page.get_by_role("button", name=f"打开{name}", exact=True).click()
        window = self.page.locator(".window").last
        window.wait_for()
        return window

    def test_trusted_login_pkce_and_no_password(self):
        self.open(False)
        self.assertEqual(self.page.locator('input[type="password"]').count(), 0)
        self.page.get_by_role("button", name="前往可信登录 →", exact=True).click()
        self.page.wait_for_url(CORE + "/oauth/authorize?*")
        params = parse_qs(urlparse(self.page.url).query)
        self.assertEqual(params["code_challenge_method"], ["S256"])
        self.assertEqual(params["redirect_uri"], [SHELL + BASE + "callback"])
        self.assertEqual(len(params["state"][0]), 43)
        self.assertNotIn("client_secret", params)

    def test_callback_exchanges_native_browser_fetch_with_exact_pkce(self):
        self.open(False)
        self.page.get_by_role("button", name="前往可信登录 →", exact=True).click()
        self.page.wait_for_url(CORE + "/oauth/authorize?*")
        params = parse_qs(urlparse(self.page.url).query)
        self.expected_challenge = params["code_challenge"][0]
        self.page.goto(SHELL + BASE + "callback?" + urlencode({
            "code": "component-authorization-code", "state": params["state"][0]}))
        self.page.wait_for_function("document.querySelector('#connection').textContent === '已连接核心'")
        self.assertEqual(self.page.url, SHELL + BASE)
        exchanged = [call for call in self.calls if call["path"] == "/oauth/token"]
        self.assertEqual(len(exchanged), 1)
        self.assertNotIn("authorization", exchanged[0]["headers"])
        self.assertNotIn("cookie", exchanged[0]["headers"])
        self.assertEqual(exchanged[0]["headers"]["origin"], SHELL)
        self.assertEqual(self.page.evaluate("Object.keys(sessionStorage).filter(key=>key.endsWith(':pending')).length"), 0)
        self.assertNotIn("component-authorization-code", self.page.locator("body").text_content())

    def test_callback_wrong_state_never_exchanges_or_reads_private_api(self):
        self.open(False)
        self.page.get_by_role("button", name="前往可信登录 →", exact=True).click()
        self.page.wait_for_url(CORE + "/oauth/authorize?*")
        self.page.goto(SHELL + BASE + "callback?code=component-authorization-code&state=wrong-state")
        self.page.get_by_text("登录未完成：INVALID_STATE", exact=True).wait_for()
        self.assertEqual([(call["method"], call["path"]) for call in self.calls], [("GET", "/oauth/authorize")])
        self.assertEqual(self.page.evaluate("Object.keys(sessionStorage).filter(key=>key.startsWith('xelab.oauth:')).length"), 0)
        self.assertEqual(self.page.locator('input[type="password"]').count(), 0)

    def test_window_drag_resize_minimize_maximize_and_keyboard(self):
        self.open()
        window = self.app("系统状态")
        window.get_by_text('"component-only"', exact=False).wait_for()
        title = window.locator(".window-title")
        initial = window.bounding_box()
        box = title.bounding_box()
        self.page.mouse.move(box["x"] + box["width"] - 50, box["y"] + 20)
        self.page.mouse.down()
        self.page.mouse.move(box["x"] + box["width"] + 50, box["y"] + 90, steps=5)
        self.page.mouse.up()
        moved = window.bounding_box()
        self.assertGreater(moved["x"], initial["x"] + 40)
        handle = window.get_by_role("button", name="调整系统状态大小")
        handle.focus()
        self.page.keyboard.press("ArrowRight")
        self.assertGreater(window.bounding_box()["width"], moved["width"])
        title.focus()
        x = window.bounding_box()["x"]
        self.page.keyboard.press("Alt+ArrowLeft")
        self.assertLess(window.bounding_box()["x"], x)
        window.get_by_role("button", name="最大化或恢复系统状态").click()
        self.assertGreater(window.bounding_box()["width"], 1100)
        window.get_by_role("button", name="最大化或恢复系统状态").click()
        self.assertLess(window.bounding_box()["width"], 1000)
        window.get_by_role("button", name="最小化系统状态").click()
        self.assertFalse(window.is_visible())
        self.page.get_by_role("button", name="恢复系统状态", exact=True).click()
        self.assertTrue(window.is_visible())
        window.get_by_role("button", name="关闭系统状态").click()
        self.assertEqual(self.page.locator(".window").count(), 0)

    def test_skill_fixed_uuid_real_request_shape_and_stable_manual_retry(self):
        self.open()
        skills = self.app("Skill")
        skills.get_by_text("component.echo", exact=True).wait_for()
        skills.get_by_role("button", name="调用测试", exact=True).click()
        editor = self.page.locator(".window").last
        editor.get_by_label("请求 JSON", exact=True).fill(json.dumps({"input": {"message": "actual browser fixture"}, "idempotency_key": "browser-business-key"}))
        editor.get_by_role("button", name="提交", exact=True).click()
        editor.get_by_text('"skill_version": "1.0.0"', exact=False).wait_for()
        editor.get_by_text('"message": "actual browser fixture"', exact=False).wait_for()
        editor.get_by_role("button", name="提交", exact=True).click()
        editor.get_by_text('"skill_version": "1.0.0"', exact=False).wait_for()
        calls = [call for call in self.calls if call["method"] == "POST"]
        self.assertEqual(len(calls), 2)
        self.assertEqual({call["path"] for call in calls}, {f"/api/platform/v1/skills/{SKILL}/calls"})
        self.assertEqual(calls[0]["headers"]["idempotency-key"], calls[1]["headers"]["idempotency-key"])
        self.assertEqual(calls[0]["body"], calls[1]["body"])
        self.assertNotIn("cookie", calls[0]["headers"])

    def test_theme_saved_preferences_reload_and_reduced_effects(self):
        self.open()
        settings = self.app("设置")
        settings.get_by_label("主题模式", exact=True).select_option("dark")
        settings.get_by_label("减少动画", exact=True).check()
        settings.get_by_label("减少透明度", exact=True).check()
        settings.get_by_role("button", name="保存到当前账户", exact=True).click()
        settings.get_by_text("已保存到核心，重新登录后仍可读取。", exact=True).wait_for()
        self.assertEqual(self.saved["spec"]["theme"]["mode"], "dark")
        self.assertTrue(self.saved["spec"]["theme"]["reduce_motion"])
        self.assertTrue(self.page.locator("html").evaluate("el => el.classList.contains('reduced-transparency')"))
        puts = [call for call in self.calls if call["method"] == "PUT"]
        self.assertEqual(puts[0]["headers"]["if-match"], '"rev-1"')
        self.page.reload()
        self.page.wait_for_function("document.documentElement.dataset.theme === 'dark'")
        self.assertEqual(self.page.locator("html").get_attribute("data-theme"), "dark")

    def test_disabled_deployment_configuration_uses_public_patch_shape(self):
        self.open()
        plugins = self.app("插件中心")
        plugins.get_by_role("button", name="配置", exact=True).click()
        editor = self.page.locator('.window[data-app="editor-updateDeployment"]')
        editor.get_by_text("先停用部署并等待实例排空", exact=False).wait_for()
        body = json.loads(editor.get_by_label("请求 JSON", exact=True).input_value())
        self.assertEqual(body, {"spec": {"display_name": "fixture deployment", "enabled": False}})
        body["spec"]["config"] = {"core_origin": CORE, "client_id": "browser-component"}
        editor.get_by_label("请求 JSON", exact=True).fill(json.dumps(body))
        editor.get_by_role("button", name="提交", exact=True).click()
        editor.get_by_text('"kind": "update_deployment"', exact=False).wait_for()
        writes = [call for call in self.calls if call["method"] == "PUT"]
        self.assertEqual(len(writes), 1)
        self.assertEqual(writes[0]["path"], f"/api/platform/v1/deployments/{DEPLOYMENT}")
        self.assertEqual(json.loads(writes[0]["body"]), body)
        self.assertEqual(writes[0]["headers"]["if-match"], '"rev-1"')
        self.assertTrue(writes[0]["headers"]["idempotency-key"])

    def test_data_is_text_denied_unavailable_logout_and_mobile_controls(self):
        self.open()
        plugins = self.app("插件中心")
        plugins.get_by_text('<img src=x onerror="alert(1)">', exact=True).wait_for()
        self.assertEqual(plugins.locator("img").count(), 0)
        self.denied.add("/api/platform/v1/clients")
        self.page.get_by_role("button", name="打开应用启动器", exact=True).click()
        self.page.get_by_role("button", name="启动UI 客户端", exact=True).click()
        self.page.locator(".window").last.get_by_text("无权限", exact=True).wait_for()
        self.page.get_by_role("button", name="打开应用启动器", exact=True).click()
        self.page.get_by_role("button", name="启动任务与知识库", exact=True).click()
        self.page.get_by_text("Batch / Workflow / 微插件 / 知识库尚不可用", exact=True).wait_for()
        self.page.set_viewport_size({"width": 390, "height": 760})
        self.page.get_by_role("button", name="打开设置", exact=True).click()
        window = self.page.locator(".window").last
        close = window.get_by_role("button", name="关闭设置")
        box = close.bounding_box()
        self.assertGreaterEqual(box["x"], 0)
        self.assertLessEqual(box["x"] + box["width"], 390)
        self.assertLessEqual(self.page.evaluate("document.documentElement.scrollWidth"), 390)
        self.page.get_by_role("button", name="退出", exact=True).click()
        self.assertEqual(self.page.locator(".window").count(), 0)
        self.assertEqual(self.page.locator("#widgets").text_content(), "")
        self.assertEqual(self.page.evaluate("Object.keys(sessionStorage).filter(k=>k.startsWith('xelab.oauth:')).length"), 0)

    def test_actual_desktop_opens_fixed_window_and_widget_bridge(self):
        for identifier, kind in [("main", "window"), ("summary", "widget")]:
            value = contribution(identifier, kind)
            value["bindings"][0]["skill_id"] = SKILL
            self.contributions[identifier] = value
        self.open()
        self.page.get_by_role("button", name="打开应用启动器", exact=True).click()
        self.page.get_by_role("button", name="启动插件页面", exact=True).click()
        listing = self.page.locator('.window[data-app="contributions"]')
        with self.page.expect_event("framenavigated", predicate=lambda frame: frame.url.startswith(PLUGIN)) as navigated:
            listing.get_by_role("button", name="打开隔离窗口", exact=True).click()
        plugin_window = self.page.locator('.window[data-app^="plugin:"]')
        plugin_window.locator("iframe").wait_for()
        frame = navigated.value
        frame.wait_for_function("messages.some(message=>message.type==='host.init')")
        frame.evaluate("emit('skill.call',{binding_alias:'echo',input:{message:'actual desktop frame'},idempotency_key:'window-intent'},1)")
        frame.wait_for_function("()=>responses().some(message=>message.ok===true)")
        self.assertEqual(frame.evaluate("responses()[0].result.call.output.message"), "actual desktop frame")
        plugin_window.get_by_role("button", name="最小化Independent fixture · org.xelab.fixture", exact=True).click()
        frame.wait_for_function("messages.some(message=>message.event==='visibility.changed'&&message.params.visible===false)")
        self.page.get_by_role("button", name="恢复Independent fixture · org.xelab.fixture", exact=True).click()
        plugin_window.get_by_role("button", name="关闭Independent fixture · org.xelab.fixture", exact=True).click()
        self.assertEqual(self.page.locator('.window[data-app^="plugin:"]').count(), 0)
        with self.page.expect_event("framenavigated", predicate=lambda frame: frame.url.startswith(PLUGIN)) as navigated:
            listing.get_by_role("button", name="添加桌面组件", exact=True).click()
        self.page.locator("#widgets iframe").wait_for()
        listing.get_by_role("button", name="关闭插件页面", exact=True).click()
        widget = navigated.value
        widget.wait_for_function("messages.some(message=>message.type==='host.init')")
        widget.evaluate("emit('skill.call',{binding_alias:'echo',input:{message:'actual desktop widget'},idempotency_key:'widget-intent'},1)")
        widget.wait_for_function("()=>responses().some(message=>message.ok===true)")
        self.assertEqual(widget.evaluate("responses()[0].result.call.output.message"), "actual desktop widget")
        self.assertEqual(len(self.saved["spec"]["widgets"]), 1)
        self.assertEqual(self.saved["spec"]["widgets"][0]["contribution"]["contribution_id"], "summary")
        self.page.locator('.widget-card').evaluate("element=>element.style.transform='translateY(3000px)'")
        widget.locator('html[data-visible="false"]').wait_for(state="attached")
        self.page.locator('.widget-card').evaluate("element=>element.style.transform=''")
        widget.locator('html[data-visible="true"]').wait_for(state="attached")
        self.assertEqual(widget.evaluate("messages.filter(message=>message.event==='widget.refresh').length"), 0)
        self.page.get_by_role("button", name="打开设置", exact=True).click()
        settings = self.page.locator('.window[data-app="settings"]')
        count = len([call for call in self.calls if call["method"] == "POST"])
        settings.get_by_label("主题模式", exact=True).select_option("dark")
        widget.wait_for_function("messages.some(message=>message.event==='theme.changed')")
        self.assertEqual(len([call for call in self.calls if call["method"] == "POST"]), count)
        self.denied.add(f'/api/platform/v1/ui/contributions/{self.contributions["summary"]["deployment_id"]}/summary')
        widget.evaluate("emit('skill.call',{binding_alias:'echo',input:{message:'revoked'},idempotency_key:'revoked-intent'},2)")
        self.page.wait_for_function("document.querySelectorAll('#widgets iframe').length===0")
        self.assertEqual(len([call for call in self.calls if call["method"] == "POST"]), count)
        self.page.get_by_role("button", name="退出", exact=True).click()
        self.assertEqual(self.page.locator("#widgets").text_content(), "")

    def test_mutating_401_never_refreshes_or_retries_business_request(self):
        self.open()
        self.status_overrides[f"/api/platform/v1/skills/{SKILL}/calls"] = 401
        skills = self.app("Skill")
        skills.get_by_role("button", name="调用测试", exact=True).click()
        editor = self.page.locator(".window").last
        editor.get_by_label("请求 JSON", exact=True).fill(json.dumps({"input": {"message": "once"}, "idempotency_key": "once"}))
        editor.get_by_role("button", name="提交", exact=True).click()
        self.page.get_by_role("button", name="登录", exact=True).wait_for()
        self.assertEqual(len([call for call in self.calls if call["method"] == "POST" and call["path"].endswith("/calls")]), 1)
        self.assertEqual(len([call for call in self.calls if call["path"] == "/oauth/token"]), 0)
        self.assertEqual(self.page.locator(".window").count(), 0)

    def test_unknown_mutation_editor_is_locked_after_preflight_credential_rotation(self):
        self.page.clock.install()
        self.refresh_enabled = True
        self.abort_business = True
        self.open(expires_ms=45000, refresh_token="component-refresh")
        skills = self.app("Skill")
        skills.get_by_role("button", name="调用测试", exact=True).click()
        editor = self.page.locator('.window[data-app="editor-invokeSkill"]')
        original_key = editor.locator('.code').text_content()
        editor.get_by_role("button", name="提交", exact=True).click()
        editor.get_by_text("请求失败", exact=True).wait_for()
        self.assertEqual(len([call for call in self.calls if call["method"] == "POST"]), 1)
        self.page.clock.fast_forward(16000)
        resources = self.app("系统状态")
        resources.get_by_text('"component-only"', exact=False).wait_for()
        editor.get_by_text("原请求已锁定", exact=True).wait_for()
        self.assertTrue(editor.get_by_role("button", name="提交", exact=True).is_disabled())
        self.assertEqual(len([call for call in self.calls if call["path"].endswith("/calls") and call["method"] == "POST"]), 1)
        self.assertEqual(len([call for call in self.calls if call["path"] == "/oauth/token"]), 1)
        self.assertEqual(editor.locator('.code').text_content(), original_key)
        self.assertTrue(self.page.get_by_role("button", name="退出", exact=True).is_visible())


if __name__ == "__main__":
    unittest.main(verbosity=2)
