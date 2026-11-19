#!/usr/bin/env python3
"""Real Chromium shell interactions with explicit routed API adapters, not Core/OCI."""
import copy
import hashlib
import json
import unittest
from urllib.parse import parse_qs, urlparse

from playwright.sync_api import expect

import test_browser as desktop
from test_bridge import contribution, framing_query


class ShellBrowser(unittest.TestCase):
    # Compose the existing fixture; inheriting it would run its test suite twice.
    @classmethod
    def setUpClass(cls):
        desktop.DesktopBrowser.setUpClass()

    @classmethod
    def tearDownClass(cls):
        desktop.DesktopBrowser.tearDownClass()

    def setUp(self):
        self.fixture = desktop.DesktopBrowser(methodName="runTest")
        self.fixture.setUp()
        self.page = self.fixture.page
        self.custom_calls = []
        self.fail_preferences = 0
        self.launcher_pages = None
        self.key_pages = None
        self.key_denied = False
        self.key_error = None
        self.account_action_error = None
        self.hold_account_ids = set()
        self.held_account_routes = {}
        self.fast_accounts = {}
        self.hold_operation_ids = set()
        self.held_operation_routes = {}
        self.operation_responses = {}
        self.allow_second_refresh = False
        self.arm_hold_widget_after_put = False
        self.hold_widget_lookup = False
        self.held_widget_routes = []
        self.hold_installation = False
        self.held_installation_routes = []
        self.subject = desktop.ACCOUNT
        self.page.route("**/*", self.route)

    def tearDown(self):
        self.fixture.tearDown()

    def reply(self, route, data, status=200, *, request_id=desktop.ACCOUNT):
        route.fulfill(status=status, content_type="application/json", headers={
            "access-control-allow-origin": desktop.SHELL,
            "access-control-allow-headers": "authorization,content-type,if-match,idempotency-key",
            "access-control-allow-methods": "GET,PUT,OPTIONS",
            "access-control-expose-headers": "etag,x-request-id",
            "etag": '"rev-1"', "x-request-id": request_id,
        }, body=json.dumps(data))

    def route(self, route):
        request, url = route.request, urlparse(route.request.url)
        if f"{url.scheme}://{url.netloc}" != desktop.CORE or request.method == "OPTIONS":
            route.fallback()
            return
        if request.method == "PUT" and url.path.endswith("/me/ui-preferences"):
            self.custom_calls.append({"path": url.path, "body": request.post_data, "headers": request.all_headers()})
            if self.fail_preferences:
                self.fail_preferences -= 1
                self.reply(route, {"error": {"code": "UNAVAILABLE", "message": "explicit component outage"}}, 503)
                return
            if self.arm_hold_widget_after_put:
                self.arm_hold_widget_after_put = False
                self.hold_widget_lookup = True
        if self.hold_widget_lookup and request.method == "GET" and "/ui/contributions/" in url.path:
            self.held_widget_routes.append(route)
            return
        if self.hold_installation and url.path.startswith("/api/platform/v1/installations/"):
            self.held_installation_routes.append(route)
            return
        if self.account_action_error and url.path.endswith("/effective-permissions"):
            status, code, message = self.account_action_error
            self.reply(route, {"error": {"code": code, "message": message}}, status)
            return
        if (self.allow_second_refresh and request.method == "POST" and url.path == "/oauth/token"
                and parse_qs(request.post_data or "", keep_blank_values=True) == {
                    "grant_type": ["refresh_token"], "client_id": ["browser-component"], "refresh_token": ["component-rotated-refresh"]}):
            self.custom_calls.append({"path": url.path, "method": request.method, "refresh_token": "component-rotated-refresh"})
            self.reply(route, {"access_token": "component-rotated-access-2", "refresh_token": "component-rotated-refresh-2",
                               "token_type": "Bearer", "expires_in": 900, "scope": "client:browser-component"})
            return
        if url.path.startswith("/api/platform/v1/operations/"):
            op_id = url.path.removeprefix("/api/platform/v1/operations/")
            if "/" not in op_id:
                if op_id in self.hold_operation_ids:
                    self.held_operation_routes[op_id] = route
                    return
                if op_id in self.operation_responses:
                    status, data = self.operation_responses[op_id]
                    self.reply(route, data, status)
                    return
        if url.path.startswith("/api/platform/v1/accounts/"):
            target_id = url.path.removeprefix("/api/platform/v1/accounts/")
            if "/" not in target_id:
                if target_id in self.hold_account_ids:
                    self.held_account_routes[target_id] = route
                    return
                if target_id in self.fast_accounts:
                    self.reply(route, self.fast_accounts[target_id])
                    return
        if url.path.endswith("/sessions") and self.subject != desktop.ACCOUNT:
            self.reply(route, {"items": [{"account_id": self.subject, "status": "active"}], "next_cursor": None})
            return
        if url.path == "/api/platform/v1/ui/contributions" and self.launcher_pages is not None:
            cursor = parse_qs(url.query).get("cursor", [None])[0]
            self.reply(route, self.launcher_pages[cursor])
            return
        if url.path == f"/api/platform/v1/accounts/{desktop.ACCOUNT}":
            self.reply(route, {"id": desktop.ACCOUNT, "display_name": "Component account", "status": "active"})
            return
        if url.path.endswith("/api-keys") or "/api-keys/" in url.path:
            self.custom_calls.append({"path": url.path, "method": request.method, "headers": request.all_headers()})
            value = {"id": desktop.DEPLOYMENT, "owner_id": desktop.ACCOUNT, "revision": 1,
                     "name": "Read-only monitor", "prefix": "xk_safe", "status": "active",
                     "issuer_credential_id": desktop.ACCOUNT, "created_at": desktop.NOW,
                     "updated_at": desktop.NOW, "last_used_at": None,
                     "inheritance": {"mode": "all_snapshot", "secret": "DO-NOT-RENDER-NESTED-SECRET"},
                     "secret": "DO-NOT-RENDER-ISSUANCE-SECRET", "verification_digest": "DO-NOT-RENDER-DIGEST"}
            if self.key_error:
                status, code, message = self.key_error
                self.reply(route, {"error": {"code": code, "message": message}}, status)
            elif self.key_denied:
                self.reply(route, {"error": {"code": "PERMISSION_DENIED", "message": "component ceiling denied"}}, 403)
            elif self.key_pages is not None:
                cursor = parse_qs(url.query).get("cursor", [None])[0]
                self.reply(route, self.key_pages[cursor])
            else:
                self.reply(route, {"items": [value], "next_cursor": None} if url.path.endswith("/api-keys") else value)
            return
        route.fallback()

    def window_contribution(self, identifier="main"):
        value = contribution(identifier)
        value["spec"]["title"] = "Pinned echo " + identifier
        self.fixture.contributions[identifier] = value
        return value

    def open_launcher(self):
        self.page.get_by_role("button", name="打开应用启动器", exact=True).click()

    def open_launcher_with_discovery_held(self):
        endpoint = desktop.CORE + "/api/platform/v1/ui/contributions?"
        pending = []

        def hold_discovery(route):
            if route.request.method != "GET":
                route.fallback()
                return
            pending.append(route)
            self.page.evaluate("() => { globalThis.__launcherDiscoveryHeld = true; }")

        self.page.route(lambda url: url.startswith(endpoint), hold_discovery)
        self.open_launcher()
        self.page.wait_for_function("() => globalThis.__launcherDiscoveryHeld === true")
        return pending.pop()

    def pin_from_launcher(self, value):
        self.open_launcher()
        self.page.locator("#app-search").fill(value["spec"]["title"])
        self.page.get_by_role("button", name="固定 " + value["spec"]["title"], exact=True).click()
        self.page.get_by_role("button", name="取消固定 " + value["spec"]["title"], exact=True).last.wait_for()
        self.page.locator("#launcher-close").click()
        self.page.get_by_role("button", name="打开固定贡献 " + value["spec"]["title"], exact=True).wait_for()

    def reference_widget(self):
        self.fixture.real_reference = True
        for identifier, kind in [("echo_window", "window"), ("echo_widget", "widget")]:
            value = contribution(identifier, kind)
            value["plugin_id"] = "org.xelab.ui-reference"
            value["spec"].update(title="Reference echo" if kind == "window" else "Echo widget", entry_path="/window.html" if kind == "window" else "/widget.html", refresh_interval_seconds=0)
            value["spec"]["skill_bindings"][0].update(name="org.xelab.ui-reference.echo", modes=["call"])
            value["bindings"][0].update(skill_id=desktop.SKILL, modes=["call"])
            value["frame_url"] = desktop.PLUGIN + "/ui/org.xelab.ui-reference" + value["spec"]["entry_path"]
            value["contribution_digest"] = "sha256:" + hashlib.sha256(json.dumps(value["spec"], sort_keys=True, separators=(",", ":")).encode()).hexdigest()
            self.fixture.contributions[identifier] = value
        self.fixture.open()
        self.open_launcher()
        self.page.get_by_role("button", name="启动插件页面", exact=True).click()
        listing = self.page.locator('.window[data-app="contributions"]')
        with self.page.expect_event("framenavigated", predicate=lambda frame: frame.url.startswith(desktop.PLUGIN + "/ui/org.xelab.ui-reference/widget.html")) as mounted:
            listing.get_by_role("button", name="添加桌面组件", exact=True).click()
        widget = mounted.value
        widget.wait_for_function("() => document.querySelector('#call')?.disabled === false")
        listing.get_by_role("button", name="关闭插件页面", exact=True).click()
        widget.evaluate("""() => {window.__windowResponses=[];const m=Object.fromEntries(new URLSearchParams(location.hash.slice(1)));addEventListener('message',event=>{const value=event.data;if(event.source===parent&&event.origin===m.parent_origin&&value?.protocol===m.protocol&&value?.view_id===m.view_id&&value?.channel_nonce===m.channel_nonce&&value.type==='response'&&value.method==='window.open')window.__windowResponses.push({ok:value.ok,at:performance.now(),view_valid:/^[a-f0-9-]{36}$/i.test(value.result?.view_id||'')});});}""")
        return widget

    def test_reference_window_open_avoids_redundant_lookup_with_original_ten_second_deadline(self):
        # Actual app/Host/reference code; only Core HTTP is a routed adapter.
        # Individual legal metadata reads take 3s, never exceeding their 10s
        # budget. The old serial duplicate caused a valid response after 12s.
        self.page.add_init_script("""(() => {const original=window.fetch.bind(window);window.__windowProbe={delay:0,start:0,requests:[]};window.fetch=async(input,options)=>{const url=new URL(typeof input==='string'?input:input.url||String(input),location.href),p=window.__windowProbe;if(p.delay&&url.pathname.startsWith('/api/platform/v1/ui/contributions/')){const row={kind:url.pathname.endsWith('/echo_widget')?'widget':'window',before_window:!document.querySelector('#windows section.window[data-app$=":echo_window"]'),start:Math.round(performance.now()-p.start)};p.requests.push(row);await new Promise(resolve=>setTimeout(resolve,p.delay));const response=await original(input,options);row.end=Math.round(performance.now()-p.start);row.status=response.status;return response;}return original(input,options);};})();""")
        widget = self.reference_widget()
        widget.evaluate("() => {window.__windowStarted=performance.now();}")
        self.page.evaluate("() => {window.__windowProbe.start=performance.now();window.__windowProbe.delay=3000;}")
        with self.page.expect_event("framenavigated", timeout=25000, predicate=lambda frame: frame.url.startswith(desktop.PLUGIN + "/ui/org.xelab.ui-reference/window.html")) as opened:
            widget.locator("#open-window").click()
        widget.get_by_text("The host opened this deployment’s echo window.", exact=True).wait_for(timeout=25000)
        opened.value.wait_for_function("() => document.querySelector('#call')?.disabled === false")
        response = widget.evaluate("() => window.__windowResponses.map(row=>({...row,at:Math.round(row.at-window.__windowStarted)}))")
        self.assertEqual(len(response), 1)
        self.assertTrue(response[0]["ok"] and response[0]["view_valid"])
        self.assertLess(response[0]["at"], 10000, "The existing reference deadline remains unchanged")
        metadata = self.page.evaluate("() => window.__windowProbe.requests")
        before = [row for row in metadata if row["kind"] == "window" and row["before_window"]]
        self.assertEqual(len(before), 1, "Bridge's checked target must not be fetched twice before opening")
        self.assertGreaterEqual(sum(row["kind"] == "window" and not row["before_window"] for row in metadata), 2, "Target mount and handshake still check current authority")
        self.assertTrue(all(row.get("status", 200) == 200 for row in metadata))
        report = {}
        audit = desktop.DRIVER.WindowOpenAudit(desktop.CORE, desktop.DEPLOYMENT, report)
        audit.start()
        audit.snapshot(widget)
        safe = report["window_open_observation"]["widget_snapshot"]
        self.assertEqual(safe["state_class"], "window_open_succeeded")
        # The driver must continue to classify old signed images and must not
        # place arbitrary plugin text in diagnostic reports.
        widget.evaluate("""() => {
          document.querySelector('#window-state').textContent='Waiting for the host to open the window…';
          document.querySelector('#skill-state').textContent='Skill call completed.';
        }""")
        audit.snapshot(widget)
        self.assertEqual(report['window_open_observation']['widget_snapshot']['state_class'], 'previous_skill_completed')
        desktop.DRIVER.wait_for_reference_skill(widget)
        widget.evaluate("""() => {
          document.querySelector('#window-state').textContent='BINDING_UNAVAILABLE: PRIVATE_DIAGNOSTIC_MARKER';
        }""")
        audit.snapshot(widget)
        self.assertEqual(report['window_open_observation']['widget_snapshot']['state_class'], 'BINDING_UNAVAILABLE')
        self.assertNotIn('PRIVATE_DIAGNOSTIC_MARKER', json.dumps(report))
        widget.evaluate("""() => {
          document.querySelector('#window-state').remove();
          document.querySelector('#skill-state').remove();
          document.querySelector('#state').textContent='Skill call completed.';
        }""")
        desktop.DRIVER.wait_for_reference_skill(widget)
        audit.snapshot(widget)
        self.assertEqual(report['window_open_observation']['widget_snapshot']['state_class'], 'previous_skill_completed')
        widget.evaluate("() => {document.querySelector('#state').textContent='The host opened this deployment’s echo window.';}")
        audit.snapshot(widget)
        self.assertEqual(report['window_open_observation']['widget_snapshot']['state_class'], 'window_open_succeeded')
        audit.end()
        print(json.dumps({"component": "actual_window_bridge_core_adapter", "metadata_delay_ms": 3000, "window_response_ms": response[0]["at"], "pre_open_target_lookups": len(before), "target_ready": True}, sort_keys=True))

    def test_closed_widget_cannot_create_late_window_across_reference_digest_await(self):
        widget = self.reference_widget()
        self.page.evaluate("""() => {const original=crypto.subtle.digest.bind(crypto.subtle);window.__hashWaiting=false;window.__hashFinished=false;crypto.subtle.digest=async(algorithm,value)=>{const input=new TextDecoder().decode(value);if(input.startsWith('[')&&input.includes('\"alias\"')){window.__hashWaiting=true;await new Promise(resolve=>{window.__releaseHash=resolve;});}const result=await original(algorithm,value);if(window.__hashWaiting)window.__hashFinished=true;return result;};}""")
        widget.locator("#open-window").click()
        self.page.wait_for_function("() => window.__hashWaiting")
        # Remove the actual source widget while the account stays logged in;
        # sessionEpoch alone cannot protect this asynchronous boundary.
        self.page.locator("#widgets .widget-card").get_by_role("button", name="移除", exact=True).click()
        self.page.wait_for_function("() => document.querySelectorAll('#widgets .widget-card').length === 0")
        self.page.get_by_role("button", name="退出", exact=True).wait_for()
        self.page.evaluate("() => window.__releaseHash()")
        self.page.wait_for_function("() => window.__hashFinished")
        self.page.evaluate("() => Promise.resolve()")
        self.assertEqual(self.page.locator('#windows section.window[data-app$=":echo_window"]').count(), 0)
        self.assertFalse(any(call["method"] == "POST" for call in self.fixture.calls))

    def test_window_open_keeps_current_target_permission_and_fixed_deployment_checks(self):
        widget = self.reference_widget()
        target = self.fixture.contributions["echo_window"]
        path = f"/api/platform/v1/ui/contributions/{target['deployment_id']}/echo_window"
        self.fixture.denied.add(path)
        widget.locator("#open-window").click()
        widget.wait_for_function("() => window.__windowResponses.length === 1")
        self.assertFalse(widget.evaluate("() => window.__windowResponses[0].ok"))
        self.assertEqual(self.page.locator('#windows section.window[data-app$=":echo_window"]').count(), 0)
        self.fixture.denied.remove(path)
        target["deployment_id"] = "00000000-0000-4000-8000-000000000099"
        widget.locator("#open-window").click()
        widget.wait_for_function("() => window.__windowResponses.length === 2")
        self.assertFalse(widget.evaluate("() => window.__windowResponses[1].ok"))
        self.assertEqual(self.page.locator('#windows section.window[data-app$=":echo_window"]').count(), 0)

    def test_target_mount_rechecks_permission_before_loading_business_page(self):
        widget = self.reference_widget()
        target = self.fixture.contributions["echo_window"]
        target_requests, target_navigations = [], []
        self.page.on("framenavigated", lambda frame: target_navigations.append(True) if frame.url.startswith(target["frame_url"]) else None)

        def metadata(route):
            target_requests.append(True)
            if len(target_requests) == 1:
                route.fallback()
            else:
                self.reply(route, {"error": {"code": "PERMISSION_DENIED", "message": "component mount proof denied", "retryable": False}}, 403)

        self.page.route(desktop.CORE + f"/api/platform/v1/ui/contributions/{target['deployment_id']}/echo_window", metadata)
        widget.locator("#open-window").click()
        window = self.page.locator('#windows section.window[data-app$=":echo_window"]')
        window.get_by_role("heading", name="无权限", exact=True).wait_for()
        self.assertEqual(len(target_requests), 2, "The checked target is independently reauthorized at mount")
        self.assertEqual(window.locator("iframe").count(), 0)
        self.assertEqual(target_navigations, [], "No target business document loads after the current mount proof is denied")

    def test_launcher_discovery_keeps_focused_builtin_app(self):
        value = self.window_contribution()
        self.fixture.open()
        discovery = self.open_launcher_with_discovery_held()
        launcher = self.page.locator("#launcher")
        self.assertTrue(self.page.locator("#app-search").evaluate("el => el === document.activeElement"))
        settings = launcher.get_by_role("button", name="启动设置", exact=True)
        settings.focus()
        self.assertTrue(settings.evaluate("el => el === document.activeElement"))

        discovery.fallback()
        launcher.get_by_role("button", name="启动贡献 " + value["spec"]["title"], exact=True).wait_for()
        self.assertTrue(settings.evaluate("el => el === document.activeElement"))

        self.page.keyboard.press("Enter")
        launcher.wait_for(state="hidden")
        title = self.page.locator('.window[data-app="settings"] .window-title')
        self.assertTrue(title.evaluate("el => el === document.activeElement"))

    def test_fixed_contribution_window_keeps_only_available_shortcuts(self):
        value = self.window_contribution("fixed")
        value["spec"]["window"]["resizable"] = False
        raw_spec = json.dumps(value["spec"], sort_keys=True, separators=(",", ":")).encode()
        value["contribution_digest"] = "sha256:" + hashlib.sha256(raw_spec).hexdigest()
        self.fixture.open()
        self.open_launcher()
        self.page.get_by_role("button", name=f"启动贡献 {value['spec']['title']}", exact=True).click()

        window = self.page.locator('.window[data-app$=":fixed"]')
        window.locator("iframe").wait_for()
        title = window.locator(".window-title")
        name = f"{value['spec']['title']} · {value['plugin_id']}"
        expect(title).to_have_attribute("aria-label", f"{name}窗口。Alt加方向键移动。", timeout=2000)
        expect(window.locator(".window-resize")).to_be_hidden()
        window.get_by_role("button", name=f"最大化{name}", exact=True).press("Enter")
        expect(title).to_have_attribute("aria-label", f"{name}窗口。")
        window.get_by_role("button", name=f"恢复{name}窗口大小", exact=True).press("Enter")
        expect(title).to_have_attribute("aria-label", f"{name}窗口。Alt加方向键移动。")
        expect(window.locator(".window-resize")).to_be_hidden()

    def test_launcher_discovery_keeps_focused_minimized_window_switcher(self):
        value = self.window_contribution()
        self.fixture.open()
        self.fixture.app("系统状态")
        window = self.fixture.app("系统状态")
        window.get_by_role("button", name="最小化系统状态", exact=True).click()
        window.wait_for(state="hidden")

        discovery = self.open_launcher_with_discovery_held()
        launcher = self.page.locator("#launcher")
        switcher = launcher.get_by_role("button", name="聚焦窗口2 系统状态", exact=True)
        launcher.get_by_role("button", name="启动设置", exact=True).focus()
        self.page.keyboard.press("Tab")
        self.page.keyboard.press("Tab")
        self.assertTrue(switcher.evaluate("el => el === document.activeElement"))

        discovery.fallback()
        launcher.get_by_role("button", name="启动贡献 " + value["spec"]["title"], exact=True).wait_for()
        self.assertTrue(switcher.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))

        self.page.keyboard.press("Enter")
        launcher.wait_for(state="hidden")
        window.wait_for()
        self.assertTrue(window.locator(".window-title").evaluate("el => el === document.activeElement"))

    def test_delayed_preferences_keep_dock_launcher_focus(self):
        self.fixture.hold_preferences = True
        self.fixture.saved["spec"]["theme"]["mode"] = "dark"
        self.fixture.open()
        dock = self.page.locator("#dock")
        launcher = dock.get_by_role("button", name="打开应用启动器", exact=True)
        dock.get_by_role("button", name="打开设置", exact=True).focus()
        self.page.keyboard.press("Tab")
        self.assertTrue(launcher.evaluate("el => el === document.activeElement"))

        self.assertEqual(len(self.fixture.pending_preferences), 1)
        self.fixture.hold_preferences = False
        self.fixture.route(self.fixture.pending_preferences.pop())
        self.page.wait_for_function("() => document.documentElement.dataset.theme === 'dark'")
        self.assertTrue(launcher.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))

        self.page.keyboard.press("Enter")
        self.page.locator("#launcher").wait_for()
        self.assertTrue(self.page.locator("#app-search").evaluate("el => el === document.activeElement"))

    def test_narrow_dock_keeps_focused_launcher_visible_when_pins_load(self):
        self.page.set_viewport_size({"width": 390, "height": 760})
        value = self.window_contribution()
        self.fixture.saved["spec"]["dock"]["pinned"] = [
            {"deployment_id": value["deployment_id"], "contribution_id": value["spec"]["id"]},
        ]
        self.fixture.hold_preferences = True
        self.fixture.open()
        dock = self.page.locator("#dock")
        launcher = dock.get_by_role("button", name="打开应用启动器", exact=True)
        dock.get_by_role("button", name="打开设置", exact=True).focus()
        self.page.keyboard.press("Tab")
        expect(launcher).to_be_focused()
        expect(launcher).to_be_in_viewport(ratio=1)

        self.assertEqual(len(self.fixture.pending_preferences), 1)
        self.fixture.hold_preferences = False
        self.fixture.route(self.fixture.pending_preferences.pop())
        dock.get_by_role("button", name=f"打开固定贡献 {value['spec']['title']}", exact=True).wait_for()
        expect(launcher).to_be_focused()
        expect(launcher).to_be_in_viewport(ratio=1)
        self.assertTrue(launcher.evaluate("""el => {
            const style = getComputedStyle(el);
            const outline = Number.parseFloat(style.outlineWidth) + Number.parseFloat(style.outlineOffset);
            const button = el.getBoundingClientRect(), dock = el.parentElement.getBoundingClientRect();
            return el.matches(':focus-visible') && style.outlineStyle !== 'none'
                && button.left - outline >= dock.left && button.right + outline <= dock.right
                && button.top - outline >= dock.top && button.bottom + outline <= dock.bottom;
        }"""))
        self.page.keyboard.press("Enter")
        expect(self.page.locator("#app-search")).to_be_focused()

    def test_resolving_a_pin_keeps_dock_unpin_focus_when_its_label_changes(self):
        identifier = "delayed-pin"
        value = self.window_contribution(identifier)
        key = {"deployment_id": value["deployment_id"], "contribution_id": identifier}
        self.fixture.saved["spec"]["dock"]["pinned"] = [key]
        pending = []

        def hold_pin(route):
            if route.request.method != "GET":
                route.fallback()
                return
            pending.append(route)
            self.page.evaluate("() => { globalThis.__dockPinHeld = true; }")

        self.page.route(desktop.CORE + f"/api/platform/v1/ui/contributions/{key['deployment_id']}/{identifier}", hold_pin)
        self.fixture.open()
        self.page.wait_for_function("() => globalThis.__dockPinHeld === true")
        dock = self.page.locator("#dock")
        unpin = dock.get_by_role("button", name=f"取消固定 {identifier}", exact=True)
        dock.get_by_role("button", name="打开设置", exact=True).focus()
        self.page.keyboard.press("Tab")
        self.assertTrue(unpin.evaluate("el => el === document.activeElement"))

        pending.pop().fallback()
        resolved_unpin = dock.get_by_role("button", name=f"取消固定 {value['spec']['title']}", exact=True)
        resolved_unpin.wait_for()
        self.assertTrue(resolved_unpin.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))

    def test_unpin_moves_focus_to_the_next_enabled_dock_control(self):
        first, second = (self.window_contribution(identifier) for identifier in ("first", "second"))
        self.fixture.contributions["unavailable"] = contribution("unavailable", kind="widget")
        deployment_id = first["deployment_id"]
        self.fixture.saved["spec"]["dock"]["pinned"] = [
            {"deployment_id": deployment_id, "contribution_id": identifier}
            for identifier in ("first", "second", "unavailable")
        ]
        self.fixture.open()
        dock = self.page.locator("#dock")
        first_open = dock.get_by_role("button", name=f"打开固定贡献 {first['spec']['title']}", exact=True)
        first_unpin = dock.get_by_role("button", name=f"取消固定 {first['spec']['title']}", exact=True)
        second_open = dock.get_by_role("button", name=f"打开固定贡献 {second['spec']['title']}", exact=True)
        second_unpin = dock.get_by_role("button", name=f"取消固定 {second['spec']['title']}", exact=True)
        unavailable_open = dock.get_by_role("button", name="打开固定贡献 unavailable", exact=True)
        unavailable_unpin = dock.get_by_role("button", name="取消固定 unavailable", exact=True)
        second_open.wait_for()
        self.assertTrue(unavailable_open.is_disabled())

        first_open.focus()
        self.page.keyboard.press("Tab")
        self.assertTrue(first_unpin.evaluate("el => el === document.activeElement"))
        self.page.keyboard.press("Enter")
        first_unpin.wait_for(state="detached")
        self.assertTrue(second_open.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))

        self.page.keyboard.press("Tab")
        self.assertTrue(second_unpin.evaluate("el => el === document.activeElement"))
        self.page.keyboard.press("Enter")
        second_unpin.wait_for(state="detached")
        self.assertTrue(unavailable_unpin.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [
            {"deployment_id": deployment_id, "contribution_id": "unavailable"},
        ])

    def test_unpin_leaves_focus_moved_to_a_window_while_saving(self):
        value = self.window_contribution()
        self.fixture.saved["spec"]["dock"]["pinned"] = [
            {"deployment_id": value["deployment_id"], "contribution_id": value["spec"]["id"]},
        ]
        pending = []

        def hold_save(route):
            if route.request.method != "PUT":
                route.fallback()
                return
            pending.append(route)
            self.page.evaluate("() => { globalThis.__unpinSaveHeld = true; }")

        self.page.route(desktop.CORE + "/api/platform/v1/me/ui-preferences", hold_save)
        self.fixture.open()
        window = self.fixture.app("系统状态")
        unpin = self.page.locator("#dock").get_by_role("button", name=f"取消固定 {value['spec']['title']}", exact=True)
        unpin.press("Enter")
        self.page.wait_for_function("() => globalThis.__unpinSaveHeld === true")

        window.locator(".window-title").focus()
        self.page.keyboard.press("Tab")
        close = window.get_by_role("button", name="关闭系统状态", exact=True)
        self.assertTrue(close.evaluate("el => el === document.activeElement"))
        pending.pop().fallback()
        unpin.wait_for(state="detached")
        self.assertTrue(close.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))

    def test_launcher_pagination_keeps_focus_through_loading_and_the_last_page(self):
        first, second, last = (self.window_contribution(identifier) for identifier in ("first", "second", "last"))
        self.launcher_pages = {None: {"items": [first], "next_cursor": "second"}}
        pending = {}
        reads = []

        def hold_page(route):
            cursor = parse_qs(urlparse(route.request.url).query).get("cursor", [None])[0]
            if route.request.method != "GET" or cursor is None:
                return route.fallback()
            reads.append(cursor)
            pending[cursor] = route
            self.page.evaluate("cursor => { globalThis.__launcherCursorHeld = cursor; }", cursor)

        endpoint = desktop.CORE + "/api/platform/v1/ui/contributions?"
        self.page.route(lambda url: url.startswith(endpoint), hold_page)
        self.fixture.open()
        self.open_launcher()
        self.page.locator("#app-search").fill("Pinned echo")
        launcher = self.page.locator("#launcher")
        more = launcher.get_by_role("button", name="加载更多贡献", exact=True)
        launcher.get_by_role("button", name=f"固定 {first['spec']['title']}", exact=True).focus()
        self.page.keyboard.press("Tab")
        expect(more).to_be_focused()

        try:
            self.page.keyboard.press("Enter")
            self.page.wait_for_function("() => globalThis.__launcherCursorHeld === 'second'")
            expect(more).to_be_focused(timeout=2000)
            expect(more).to_be_disabled()
            self.assertTrue(more.evaluate("el => el.matches(':focus-visible')"))
            self.page.keyboard.press("Space")
            self.reply(pending.pop("second"), {"items": [second], "next_cursor": "last"})
            launcher.get_by_role("heading", name=second["spec"]["title"], exact=True).wait_for()
            expect(more).to_be_focused()
            expect(more).to_be_enabled()

            self.page.keyboard.press("Enter")
            self.page.wait_for_function("() => globalThis.__launcherCursorHeld === 'last'")
            self.reply(pending.pop("last"), {"items": [last], "next_cursor": None})
            launch = launcher.get_by_role("button", name=f"启动贡献 {last['spec']['title']}", exact=True)
            expect(launch).to_be_focused()
            self.assertTrue(launch.evaluate("el => el.matches(':focus-visible')"))
            expect(more).to_have_count(0)
            self.assertEqual(reads, ["second", "last"])
        finally:
            for route in pending.values():
                route.abort()

    def test_launcher_search_pages_and_pin_open_unpin_fixed_contribution(self):
        first, second = self.window_contribution(), self.window_contribution("second")
        self.launcher_pages = {None: {"items": [first], "next_cursor": "component-next"},
                               "component-next": {"items": [second], "next_cursor": None}}
        self.fixture.open()
        self.open_launcher()
        self.page.get_by_role("button", name="加载更多贡献", exact=True).click()
        self.page.locator("#app-search").fill(second["spec"]["title"])
        self.page.get_by_role("button", name="固定 " + second["spec"]["title"], exact=True).click()
        self.page.get_by_role("button", name="取消固定 " + second["spec"]["title"], exact=True).last.wait_for()
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [{"deployment_id": second["deployment_id"], "contribution_id": "second"}])
        self.assertNotIn("digest", json.dumps(self.fixture.saved["spec"]["dock"]["pinned"]))
        self.page.locator("#launcher-close").click()
        self.page.get_by_role("button", name="打开固定贡献 " + second["spec"]["title"], exact=True).click()
        window = self.page.locator('.window[data-app="plugin:' + second["deployment_id"] + ':second"]')
        expected_src = second["frame_url"] + "#protocol=xelab.ui%2F1"
        # The mount verifies current permissions asynchronously after creating
        # the iframe. Wait for the authorized target under the same timeout.
        frame = window.locator("iframe[src^=" + json.dumps(expected_src) + "]")
        frame.wait_for()
        self.assertEqual(frame.get_attribute("sandbox"), "allow-scripts allow-same-origin")
        self.assertTrue(frame.get_attribute("src").startswith(expected_src))
        self.page.get_by_role("button", name="取消固定 " + second["spec"]["title"], exact=True).click()
        self.page.wait_for_function("() => !document.querySelector('.dock-pin')")
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [])
        self.assertEqual(len(self.custom_calls), 2)
        self.assertEqual(self.custom_calls[0]["headers"]["if-match"], '"rev-1"')
        self.assertEqual(self.custom_calls[1]["headers"]["if-match"], '"rev-2"')
        self.assertNotEqual(self.custom_calls[0]["headers"]["idempotency-key"], self.custom_calls[1]["headers"]["idempotency-key"])

    def test_stale_pin_control_does_not_remove_a_pin_added_in_another_view(self):
        value = self.window_contribution("stale-pin")
        title = value["spec"]["title"]
        key = {"deployment_id": value["deployment_id"], "contribution_id": value["spec"]["id"]}
        self.fixture.open()
        self.open_launcher()
        self.page.get_by_role("button", name="启动插件页面", exact=True).click()
        window = self.page.locator('.window[data-app="contributions"]')
        control = window.locator("article .toolbar button:last-child")
        control.wait_for()
        self.assertEqual(control.get_attribute("aria-label"), f"固定 {title}")

        self.open_launcher()
        launcher = self.page.locator("#launcher")
        launcher.get_by_role("button", name=f"固定 {title}", exact=True).click()
        launcher.get_by_role("button", name=f"取消固定 {title}", exact=True).wait_for()
        self.page.locator("#launcher-close").click()
        self.page.locator("#dock .dock-pin").wait_for()
        self.assertEqual(control.get_attribute("aria-label"), f"固定 {title}")

        control.click()
        window.locator("article .toolbar button:last-child:enabled").wait_for()
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [key])
        self.assertEqual(control.get_attribute("aria-label"), f"取消固定 {title}")

        control.click()
        window.locator("article .toolbar button:last-child:enabled").wait_for()
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [])
        self.assertEqual(control.get_attribute("aria-label"), f"固定 {title}")
        writes = [json.loads(call["body"])["dock"]["pinned"] for call in self.custom_calls
                  if call["path"].endswith("/me/ui-preferences")]
        self.assertEqual(writes, [[key], []])

    def test_launcher_pin_retry_after_etag_mismatch_reads_current_preferences(self):
        value = self.window_contribution()
        self.fixture.open()
        endpoint = desktop.CORE + "/api/platform/v1/me/ui-preferences"
        writes = []

        def enforce_preference_etag(route):
            if route.request.method != "PUT":
                route.fallback()
                return
            if not writes:
                # A second client saves after this tab reads preferences.
                self.fixture.saved["revision"] += 1
                self.fixture.saved["spec"]["theme"]["mode"] = "dark"
            headers = route.request.all_headers()
            writes.append(headers)
            revision = self.fixture.saved["revision"]
            if headers["if-match"] == f'"rev-{revision}"':
                route.fallback()
            else:
                self.reply(route, {"error": {"code": "ETAG_MISMATCH", "message": "Resource revision changed"}}, 412)

        self.page.route(endpoint, enforce_preference_etag)
        self.open_launcher()
        title = value["spec"]["title"]
        pin = self.page.get_by_role("button", name="固定 " + title, exact=True)
        pin.click()
        self.page.locator("#toast").get_by_text("ETAG_MISMATCH", exact=False).wait_for()
        self.assertEqual(len(writes), 1)
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [])

        with self.page.expect_response(lambda response: response.url == endpoint and response.request.method == "PUT") as retry:
            pin.click()
        self.assertEqual(retry.value.status, 200)
        self.assertEqual([write["if-match"] for write in writes], ['"rev-1"', '"rev-2"'])
        self.assertNotEqual(writes[0]["idempotency-key"], writes[1]["idempotency-key"])
        spec = self.fixture.saved["spec"]
        self.assertEqual(spec["dock"]["pinned"], [{"deployment_id": value["deployment_id"], "contribution_id": "main"}])
        self.assertEqual(spec["theme"]["mode"], "dark")

    def test_widget_retry_keeps_unknown_intent_after_etag_mismatch(self):
        value = contribution("conflict_widget", "widget")
        self.fixture.contributions["conflict_widget"] = value
        self.page.clock.set_fixed_time("2026-11-01T12:00:00Z")
        self.fixture.refresh_enabled = True
        self.fixture.open(expires_ms=45000, refresh_token="component-refresh")
        endpoint = desktop.CORE + "/api/platform/v1/me/ui-preferences"
        writes = []

        def handle_preference_write(route):
            request = route.request
            if request.method != "PUT":
                route.fallback()
                return
            headers = request.all_headers()
            writes.append((headers, request.post_data))
            if len(writes) == 1:
                # The original credential's write commits, but its response is lost.
                self.fixture.saved["spec"] = json.loads(request.post_data)
                self.fixture.saved["revision"] += 1
                route.abort("failed")
            elif headers["if-match"] == f'"rev-{self.fixture.saved["revision"]}"':
                route.fallback()
            else:
                self.reply(route, {"error": {"code": "ETAG_MISMATCH", "message": "Resource revision changed"}}, 412)

        self.page.route(endpoint, handle_preference_write)
        self.open_launcher()
        launch = self.page.get_by_role("button", name="启动贡献 " + value["spec"]["title"], exact=True)
        launch.click()
        self.page.locator("#toast").get_by_text("CONNECTION_FAILED", exact=False).wait_for()
        self.assertEqual(len(writes), 1)

        # Credential rotation means the new credential cannot replay the old write.
        self.page.clock.set_fixed_time("2026-11-01T12:00:16Z")
        for _ in range(2):
            with self.page.expect_response(lambda response: response.url == endpoint and response.request.method == "PUT") as retry:
                launch.click()
            self.assertEqual(retry.value.status, 412)
        self.assertEqual(len(writes), 3)
        first_headers, first_body = writes[0]
        for headers, body in writes[1:]:
            self.assertNotEqual(headers["authorization"], first_headers["authorization"])
            self.assertEqual(headers["if-match"], first_headers["if-match"])
            self.assertEqual(headers["idempotency-key"], first_headers["idempotency-key"])
            self.assertEqual(body, first_body)
        self.assertEqual(len(self.fixture.saved["spec"]["widgets"]), 1)

    def test_closed_launcher_cannot_open_window_after_reopening(self):
        value = self.window_contribution()
        self.fixture.open()
        self.open_launcher()
        launcher = self.page.locator("#launcher")
        launch = launcher.get_by_role("button", name="启动贡献 " + value["spec"]["title"], exact=True)
        original_button = launch.element_handle()
        self.hold_widget_lookup = True
        original_button.click()
        for _ in range(250):
            if self.held_widget_routes:
                break
            self.page.wait_for_timeout(20)
        self.assertEqual(len(self.held_widget_routes), 1)
        self.assertTrue(original_button.is_disabled())

        self.page.keyboard.press("Escape")
        launcher.wait_for(state="hidden")
        active = self.fixture.app("系统状态")
        active.get_by_text('"component-only"', exact=False).wait_for()
        self.open_launcher()
        search = launcher.locator("#app-search")
        search.fill("settings")
        self.hold_widget_lookup = False
        self.held_widget_routes.pop().fallback()
        # The detached button is re-enabled when its old handler has returned.
        self.page.wait_for_function("button => !button.disabled", arg=original_button)
        self.assertEqual(self.page.locator('.window[data-app^="plugin:"]').count(), 0)
        self.assertTrue(launcher.is_visible())
        self.assertEqual(search.input_value(), "settings")
        self.assertTrue(search.evaluate("element => element === document.activeElement"))
        self.assertEqual(self.page.locator("#toast").inner_text(), "")

        search.fill(value["spec"]["title"])
        launch.click()
        launcher.wait_for(state="hidden")
        window = self.page.locator('.window[data-app^="plugin:"]')
        window.locator("iframe[src]").wait_for()
        self.assertEqual(window.count(), 1)
        self.assertTrue(window.locator(".window-title").evaluate("element => element === document.activeElement"))

    def test_closed_launcher_cannot_submit_widget_after_preference_read(self):
        value = contribution("late_widget", "widget")
        self.fixture.contributions["late_widget"] = value
        self.fixture.open()
        self.open_launcher()
        launcher = self.page.locator("#launcher")
        launch = launcher.get_by_role("button", name="启动贡献 " + value["spec"]["title"], exact=True)
        original_button = launch.element_handle()
        self.fixture.hold_preferences = True
        original_button.click()
        for _ in range(250):
            if self.fixture.pending_preferences:
                break
            self.page.wait_for_timeout(20)
        self.assertEqual(len(self.fixture.pending_preferences), 1)
        self.assertTrue(original_button.is_disabled())

        self.page.get_by_role("button", name="关闭启动器", exact=True).click()
        launcher.wait_for(state="hidden")
        self.open_launcher()
        self.fixture.hold_preferences = False
        self.fixture.route(self.fixture.pending_preferences.pop())
        self.page.wait_for_function("button => !button.disabled", arg=original_button)
        self.assertEqual(self.fixture.saved["spec"]["widgets"], [])
        self.assertEqual(self.custom_calls, [])
        self.assertTrue(launcher.is_visible())
        self.assertEqual(self.page.locator("#widgets iframe").count(), 0)
        self.assertEqual(self.page.locator("#toast").inner_text(), "")

        # An explicit new launch can submit once. Let its already-sent write
        # finish after closing the dialog, without dismissing the next opening.
        pending_writes = []

        def hold_write(route):
            if route.request.method == "PUT":
                pending_writes.append(route)
            else:
                route.fallback()

        self.page.route(desktop.CORE + "/api/platform/v1/me/ui-preferences", hold_write)
        retry_button = launch.element_handle()
        retry_button.click()
        for _ in range(250):
            if pending_writes:
                break
            self.page.wait_for_timeout(20)
        self.assertEqual(len(pending_writes), 1)
        self.assertTrue(retry_button.is_disabled())
        self.page.get_by_role("button", name="关闭启动器", exact=True).click()
        launcher.wait_for(state="hidden")
        self.open_launcher()
        pending_writes.pop().fallback()
        self.page.wait_for_function("button => !button.disabled", arg=retry_button)
        self.assertTrue(launcher.is_visible())
        self.assertEqual(len(self.custom_calls), 1)
        widgets = self.fixture.saved["spec"]["widgets"]
        self.assertEqual([widget["contribution"]["contribution_id"] for widget in widgets], ["late_widget"])
        self.page.locator("#widgets iframe[src]").wait_for()

    def test_closed_launcher_cannot_send_widget_after_credential_refresh(self):
        value = contribution("late_refresh", "widget")
        self.fixture.contributions["late_refresh"] = value
        self.page.clock.set_fixed_time("2026-11-01T12:00:00Z")
        self.fixture.refresh_enabled = True
        self.fixture.open(expires_ms=45000, refresh_token="component-refresh")
        self.open_launcher()
        launcher = self.page.locator("#launcher")
        launch = launcher.get_by_role("button", name="启动贡献 " + value["spec"]["title"], exact=True)
        original_button = launch.element_handle()
        self.fixture.hold_preferences = True
        original_button.click()
        for _ in range(250):
            if self.fixture.pending_preferences:
                break
            self.page.wait_for_timeout(20)
        self.assertEqual(len(self.fixture.pending_preferences), 1)
        self.assertTrue(original_button.is_disabled())

        # The preflight already holds a response, but the PUT now needs a
        # credential refresh before ApiClient can start the request.
        self.page.clock.set_fixed_time("2026-11-01T12:00:16Z")
        self.fixture.hold_preferences = False
        self.fixture.hold_refresh = True
        self.fixture.route(self.fixture.pending_preferences.pop())
        for _ in range(250):
            if self.fixture.pending_refresh:
                break
            self.page.wait_for_timeout(20)
        self.assertEqual(len(self.fixture.pending_refresh), 1)
        self.page.keyboard.press("Escape")
        launcher.wait_for(state="hidden")
        self.open_launcher()
        self.fixture.hold_refresh = False
        self.fixture.route(self.fixture.pending_refresh.pop())
        self.page.wait_for_function("button => !button.disabled", arg=original_button)
        self.assertEqual(len(self.custom_calls), 0)
        self.assertEqual(self.fixture.saved["spec"]["widgets"], [])
        self.assertTrue(launcher.is_visible())
        self.assertTrue(self.page.get_by_role("button", name="退出", exact=True).is_visible())
        launch.click()
        launcher.wait_for(state="hidden")
        self.assertEqual(len(self.custom_calls), 1)
        widgets = self.fixture.saved["spec"]["widgets"]
        self.assertEqual([widget["contribution"]["contribution_id"] for widget in widgets], ["late_refresh"])

    def test_pinned_digest_change_cannot_silently_open_new_version(self):
        value = self.window_contribution()
        self.fixture.open()
        self.pin_from_launcher(value)
        self.fixture.contributions["main"]["contribution_digest"] = "sha256:" + "e" * 64
        self.page.get_by_role("button", name="打开固定贡献 " + value["spec"]["title"], exact=True).click()
        self.page.locator("#toast").get_by_text("贡献或权限已改变", exact=False).wait_for()
        self.assertEqual(self.page.locator(".bridge-frame").count(), 0)
        self.assertTrue(self.page.get_by_role("button", name="打开固定贡献 " + value["spec"]["title"], exact=True).is_disabled())
        self.assertEqual(len(self.custom_calls), 1)

    def test_pinned_denied_can_be_removed_without_loading_iframe(self):
        value = self.window_contribution()
        self.fixture.saved["spec"]["dock"]["pinned"] = [{"deployment_id": value["deployment_id"], "contribution_id": "main"}]
        self.fixture.open()
        self.fixture.denied.add(f'/api/platform/v1/ui/contributions/{value["deployment_id"]}/main')
        self.page.reload()
        self.page.wait_for_function("document.querySelector('#connection').textContent === '当前请求无权限'")
        self.assertTrue(self.page.get_by_role("button", name="打开固定贡献 main", exact=True).is_disabled())
        self.page.get_by_role("button", name="取消固定 main", exact=True).click()
        self.page.wait_for_function("() => !document.querySelector('.dock-pin')")
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [])
        self.assertEqual(self.page.locator(".bridge-frame").count(), 0)

    def widget(self):
        value = contribution("widget", "widget")
        value["spec"]["title"] = "Layout widget"
        value["spec"]["widget"].update(min_columns=2, max_columns=4, min_rows=2, max_rows=3)
        self.fixture.contributions["widget"] = value
        placement = {"instance_id": desktop.ACCOUNT, "contribution": {"deployment_id": value["deployment_id"], "contribution_id": "widget"},
                     "column": 0, "row": 0, "columns": 3, "rows": 2}
        self.fixture.saved["spec"]["widgets"] = [placement]
        return value, copy.deepcopy(placement)

    def layout_window(self):
        self.page.get_by_role("button", name="调整组件布局 Layout widget", exact=True).click()
        return self.page.locator('.window[data-app^="widget-layout:"]')

    def restore_record(self, value):
        bindings = [{"alias": item["alias"], "skill_id": item["skill_id"], "definition_digest": item["definition_digest"],
                     "modes": sorted(item["modes"])} for item in sorted(value["bindings"], key=lambda item: item["alias"])]
        return {"app": "contribution", "contribution": {
            "deployment_id": value["deployment_id"], "contribution_id": value["spec"]["id"],
            "installation_id": value["installation_id"], "manifest_digest": value["manifest_digest"],
            "contribution_digest": value["contribution_digest"],
            "bindings_digest": "sha256:" + hashlib.sha256(json.dumps(bindings, separators=(",", ":")).encode()).hexdigest()},
            "rect": {"x": 100, "y": 80, "width": 600, "height": 360}, "minimized": False, "maximized": False}

    def seed_restore(self, value, account=desktop.ACCOUNT):
        key = f"xelab:windows:{desktop.CORE}:browser-component:{account}"
        stored = json.dumps([self.restore_record(value)])
        self.fixture.context.add_init_script(f"if(location.origin === {json.dumps(desktop.SHELL)}) localStorage.setItem({json.dumps(key)}, {json.dumps(stored)});")
        return key

    def test_contribution_restore_uses_new_channel_and_never_stores_business_or_credentials(self):
        value = self.window_contribution()
        value["frame_url"] += framing_query("main", "browser-component")
        self.fixture.open()
        skills = self.fixture.app("Skill")
        skills.get_by_role("button", name="调用测试", exact=True).click()
        editor = self.page.locator(".window").last
        editor.get_by_label("请求 JSON", exact=True).fill(json.dumps({"input": {"private": "BUSINESS-RESULT-NOT-LAYOUT"}, "idempotency_key": "private-business-intent"}))
        editor.get_by_role("button", name="提交", exact=True).click()
        editor.get_by_text('"private": "BUSINESS-RESULT-NOT-LAYOUT"', exact=False).wait_for()
        desktop.DRIVER.close_windows(self.page, {"checks": {}})
        self.pin_from_launcher(value)
        self.page.get_by_role("button", name="打开固定贡献 " + value["spec"]["title"], exact=True).click()
        window = self.page.locator('.window[data-app^="plugin:"]')
        frame = window.locator("iframe")
        frame.wait_for()
        before_view, before_source = window.get_attribute("data-view-id"), frame.get_attribute("src")
        window.locator(".window-title").focus()
        self.page.keyboard.press("Alt+ArrowRight")
        key = f"xelab:windows:{desktop.CORE}:browser-component:{desktop.ACCOUNT}"
        stored = self.page.evaluate("key => localStorage.getItem(key)", key)
        record = json.loads(stored)[0]
        self.assertEqual(set(record["contribution"]), {"deployment_id", "contribution_id", "installation_id", "manifest_digest", "contribution_digest", "bindings_digest"})
        for private in ("BUSINESS-RESULT-NOT-LAYOUT", "private-business-intent", "component-token", "channel_nonce", "bindings\"", "frame_url"):
            self.assertNotIn(private, stored)
        self.page.reload()
        restored = self.page.locator('.window[data-app^="plugin:"]')
        restored.locator("iframe").wait_for()
        self.assertNotEqual(restored.get_attribute("data-view-id"), before_view)
        self.assertNotEqual(restored.locator("iframe").get_attribute("src"), before_source)
        self.assertEqual(restored.evaluate("el => [el.style.left, el.style.top, el.style.width, el.style.height]"),
                         [f'{record["rect"][field]}px' for field in ("x", "y", "width", "height")])
        title = f'{value["spec"]["title"]} · {value["plugin_id"]}'
        expect(restored.locator(".window-title h2")).to_have_text(title)
        expect(self.page.locator("#active-app")).to_have_text(title, timeout=2000)
        maximize = restored.get_by_role("button", name=f"最大化{title}", exact=True)
        maximize.press("Enter")
        restored.get_by_role("button", name=f"恢复{title}窗口大小", exact=True).press("Enter")
        maximize.wait_for()
        self.assertEqual(len([call for call in self.fixture.calls if call["method"] == "POST"]), 1)
        self.assertNotIn("BUSINESS-RESULT-NOT-LAYOUT", self.page.locator("body").inner_text())

    def test_background_contribution_restore_preserves_active_window_title(self):
        value = self.window_contribution()
        self.seed_restore(value)
        endpoint = f'{desktop.CORE}/api/platform/v1/ui/contributions/{value["deployment_id"]}/main'
        pending = []
        captured = False

        def hold_restore(route):
            nonlocal captured
            if route.request.method != "GET" or captured:
                return route.fallback()
            captured = True
            pending.append(route)
            self.page.evaluate("() => { globalThis.__contributionRestoreHeld = true; }")

        self.page.route(endpoint, hold_restore)
        try:
            self.fixture.open()
            self.page.wait_for_function("() => globalThis.__contributionRestoreHeld === true")
            expect(self.page.locator("#active-app")).to_have_text("插件窗口 · main")
            active = self.fixture.app("系统状态")
            expect(active.locator(".window-title")).to_be_focused()

            pending.pop().fallback()
            restored = self.page.locator('.window[data-app^="plugin:"]')
            title = f'{value["spec"]["title"]} · {value["plugin_id"]}'
            expect(restored.locator(".window-title h2")).to_have_text(title)
            expect(self.page.locator("#active-app")).to_have_text("系统状态")
            expect(active.locator(".window-title")).to_be_focused()
        finally:
            for route in pending:
                route.abort()

    def test_changed_restore_is_inert_until_explicit_current_version_discovery(self):
        value = self.window_contribution()
        key = self.seed_restore(value)
        value["manifest_digest"] = "sha256:" + "f" * 64
        self.fixture.open()
        window = self.page.locator('.window[data-app^="plugin:"]')
        window.get_by_role("heading", name="插件窗口等待确认", exact=True).wait_for()
        self.assertEqual(window.locator("iframe").count(), 0)
        self.assertEqual(json.loads(self.page.evaluate("key => localStorage.getItem(key)", key))[0]["contribution"]["manifest_digest"], "sha256:" + "b" * 64)
        window.get_by_role("button", name="明确重新发现并加载", exact=True).click()
        window.locator("iframe").wait_for()
        self.assertEqual(json.loads(self.page.evaluate("key => localStorage.getItem(key)", key))[0]["contribution"]["manifest_digest"], value["manifest_digest"])
        self.assertEqual([call for call in self.fixture.calls if call["method"] == "POST"], [])

    def test_changed_whitelist_or_denied_restore_never_mounts_automatically(self):
        value = self.window_contribution()
        self.seed_restore(value)
        value["bindings"] = []
        self.fixture.open()
        window = self.page.locator('.window[data-app^="plugin:"]')
        window.get_by_role("heading", name="插件窗口等待确认", exact=True).wait_for()
        self.assertEqual(window.locator("iframe").count(), 0)
        path = f'/api/platform/v1/ui/contributions/{value["deployment_id"]}/main'
        self.fixture.denied.add(path)
        window.get_by_role("button", name="明确重新发现并加载", exact=True).click()
        window.get_by_role("heading", name="无权限", exact=True).wait_for()
        self.assertEqual(window.locator("iframe").count(), 0)
        self.fixture.denied.clear()
        window.get_by_role("button", name="明确重新发现并加载", exact=True).click()
        window.locator("iframe").wait_for()
        self.assertEqual([call for call in self.fixture.calls if call["method"] == "POST"], [])

    def test_delayed_restore_cannot_replace_new_interactive_window(self):
        value = self.window_contribution()
        self.seed_restore(value)
        self.fixture.hold_preferences = True
        self.fixture.open()
        active = self.fixture.app("系统状态")
        active.get_by_text('"component-only"', exact=False).wait_for()
        identity = active.get_attribute("data-view-id")
        self.fixture.hold_preferences = False
        for route in self.fixture.pending_preferences:
            self.fixture.route(route)
        self.page.wait_for_function("() => document.querySelector('#connection').textContent === '已连接核心'")
        self.assertEqual(self.page.locator(".window").count(), 1)
        self.assertEqual(active.get_attribute("data-view-id"), identity)
        self.assertEqual(self.page.locator("iframe").count(), 0)

    def test_restore_is_account_scoped_and_logout_removes_current_layout(self):
        value = self.window_contribution()
        key = self.seed_restore(value)
        self.fixture.open()
        self.page.locator('.window[data-app^="plugin:"] iframe').wait_for()
        self.page.get_by_role("button", name="退出", exact=True).click()
        self.assertEqual(self.page.locator("iframe").count(), 0)
        self.assertIsNone(self.page.evaluate("key => localStorage.getItem(key)", key))
        self.subject = desktop.DEPLOYMENT
        self.fixture.saved = desktop.preferences()
        self.fixture.saved["owner_id"] = self.subject
        # The init script intentionally leaves a former account's non-sensitive
        # layout on reload; the new subject must never select that namespace.
        self.page.reload()
        self.page.get_by_role("button", name="退出", exact=True).wait_for()
        self.page.wait_for_function("() => document.querySelector('#connection').textContent === '已连接核心'")
        self.assertEqual(self.page.locator(".window").count(), 0)
        self.assertEqual(self.page.locator("iframe").count(), 0)

    def test_widget_layout_constraints_and_frozen_retry_preserve_preferences(self):
        _, placement = self.widget()
        self.fixture.saved["spec"]["theme"]["mode"] = "dark"
        self.fixture.open()
        window = self.layout_window()
        width = window.get_by_label("宽度格数", exact=True)
        self.assertEqual((width.get_attribute("min"), width.get_attribute("max")), ("2", "4"))
        width.fill("5")
        window.get_by_role("button", name="保存组件布局", exact=True).click()
        self.assertFalse(width.evaluate("el => el.checkValidity()"))
        self.assertEqual(self.custom_calls, [])
        width.fill("4")
        window.get_by_label("列坐标", exact=True).fill("2")
        window.get_by_label("行坐标", exact=True).fill("3")
        window.get_by_label("高度格数", exact=True).fill("3")
        self.fail_preferences = 1
        window.get_by_role("button", name="保存组件布局", exact=True).click()
        window.get_by_role("heading", name="当前不可用", exact=True).wait_for()
        window.get_by_role("button", name="保存组件布局", exact=True).click()
        window.get_by_role("heading", name="组件布局已保存", exact=True).wait_for()
        first, second = self.custom_calls
        self.assertEqual(first["body"], second["body"])
        self.assertEqual(first["headers"]["idempotency-key"], second["headers"]["idempotency-key"])
        self.assertEqual(first["headers"]["if-match"], second["headers"]["if-match"])
        expected = {**placement, "column": 2, "row": 3, "columns": 4, "rows": 3}
        self.assertEqual(self.fixture.saved["spec"]["widgets"], [expected])
        self.assertEqual(self.fixture.saved["spec"]["theme"]["mode"], "dark")
        self.assertEqual(self.page.locator(".widget-card").evaluate("el => [el.style.gridColumn, el.style.gridRow]"), ["3 / span 4", "4 / span 3"])
        window.get_by_label("行坐标", exact=True).fill("4")
        window.get_by_role("button", name="保存组件布局", exact=True).click()
        self.page.wait_for_function("() => document.querySelector('.widget-card').style.gridRow === '5 / span 3'")
        self.assertEqual(self.fixture.saved["spec"]["widgets"][0]["row"], 4)
        self.assertEqual(self.custom_calls[2]["headers"]["if-match"], '"rev-2"')
        self.assertNotEqual(first["headers"]["idempotency-key"], self.custom_calls[2]["headers"]["idempotency-key"])

    def test_widget_moved_offscreen_can_be_reached_by_wheel(self):
        self.widget()
        self.fixture.open()
        layout = self.layout_window()
        layout.get_by_label("行坐标", exact=True).fill("18")
        layout.get_by_role("button", name="保存组件布局", exact=True).click()
        layout.get_by_text("组件布局已保存", exact=True).wait_for()
        layout.get_by_role("button", name="关闭组件布局 · Layout widget", exact=True).click()
        self.assertEqual(self.fixture.saved["spec"]["widgets"][0]["row"], 18)

        grid = self.page.locator("#widgets")
        card = grid.locator(".widget-card")
        height = self.page.viewport_size["height"]
        self.assertGreater(card.bounding_box()["y"], height)
        self.assertTrue(grid.evaluate("el => el.scrollHeight > el.clientHeight && el.scrollTop === 0"))
        bounds = grid.bounding_box()
        x, y = bounds["x"] + bounds["width"] / 2, bounds["y"] + bounds["height"] / 2
        self.page.mouse.move(x, y)
        self.page.mouse.wheel(0, 2500)
        self.page.wait_for_function("() => document.querySelector('#widgets').scrollTop > 0", timeout=2500)
        self.assertLess(card.bounding_box()["y"], height)

        card.get_by_role("button", name="移除", exact=True).click()
        card.wait_for(state="detached")
        self.assertEqual(self.fixture.saved["spec"]["widgets"], [])
        self.assertEqual(self.page.evaluate("([x,y]) => document.elementFromPoint(x,y).id", [x, y]), "desktop")

    def test_widget_overlap_stale_and_denied_never_write_preferences(self):
        value, placement = self.widget()
        self.fixture.saved["spec"]["widgets"].append({**placement, "instance_id": desktop.DEPLOYMENT, "column": 4})
        self.fixture.open()
        self.page.get_by_role("button", name="调整组件布局 Layout widget", exact=True).first.click()
        window = self.page.locator('.window[data-app^="widget-layout:"]')
        window.get_by_label("列坐标", exact=True).fill("4")
        window.get_by_role("button", name="保存组件布局", exact=True).click()
        window.get_by_text("此位置与其他组件重叠", exact=False).wait_for()
        self.assertEqual(self.custom_calls, [])
        self.fixture.denied.add(f'/api/platform/v1/ui/contributions/{value["deployment_id"]}/widget')
        window.get_by_label("列坐标", exact=True).fill("0")
        window.get_by_role("button", name="保存组件布局", exact=True).click()
        window.get_by_role("heading", name="无权限", exact=True).wait_for()
        self.assertEqual(self.custom_calls, [])
        self.fixture.denied.clear()
        self.fixture.contributions["widget"]["contribution_digest"] = "sha256:" + "f" * 64
        window.get_by_role("button", name="保存组件布局", exact=True).click()
        window.get_by_text("固定贡献或权限已改变", exact=False).wait_for()
        self.assertEqual(self.custom_calls, [])

    def test_api_key_metadata_reads_are_authorized_fixed_gets_and_never_show_secret(self):
        self.fixture.open()
        account = self.fixture.app("账户与权限")
        account.get_by_role("button", name="API Key 元数据", exact=True).click()
        keys = self.page.locator('.window[data-app^="api-keys:"]')
        keys.get_by_role("heading", name="Read-only monitor", exact=True).wait_for()
        keys.get_by_role("button", name="读取密钥元数据", exact=True).click()
        self.page.get_by_role("heading", name="API Key 元数据详情", exact=True).wait_for()
        body = self.page.locator("body").inner_text()
        self.assertIn("xk_safe", body)
        self.assertIn("all_snapshot", body)
        self.assertNotIn("DO-NOT-RENDER", body)
        self.assertNotIn("verification_digest", body)
        self.assertEqual([(call["method"], call["path"]) for call in self.custom_calls], [
            ("GET", f"/api/platform/v1/accounts/{desktop.ACCOUNT}/api-keys"),
            ("GET", f"/api/platform/v1/api-keys/{desktop.DEPLOYMENT}")])
        self.assertTrue(all("cookie" not in call["headers"] for call in self.custom_calls))

    def test_api_key_denied_has_real_denial_state_without_key_metadata(self):
        self.key_denied = True
        self.fixture.open()
        self.fixture.app("账户与权限").get_by_role("button", name="API Key 元数据", exact=True).click()
        self.page.locator('.window[data-app^="api-keys:"]').get_by_role("heading", name="无权限", exact=True).wait_for()
        self.assertNotIn("xk_safe", self.page.locator("body").inner_text())
        self.assertNotIn("DO-NOT-RENDER", self.page.locator("body").inner_text())

    def test_desktop_failure_redacts_server_error_messages(self):
        secret = "TOKEN=do-not-render-account-secret"
        self.key_error = (500, "INTERNAL_ERROR", secret)
        self.fixture.open()
        self.fixture.app("账户与权限").get_by_role("button", name="API Key 元数据", exact=True).click()
        window = self.page.locator('.window[data-app^="api-keys:"]')
        window.get_by_role("heading", name="请求失败", exact=True).wait_for()
        body = self.page.locator("body").inner_text()
        self.assertNotIn(secret, body)
        self.assertIn("INTERNAL_ERROR", window.inner_text())

    def test_list_section_pagination_retry_preserves_items_and_clears_error(self):
        key_first = {"id": desktop.DEPLOYMENT, "owner_id": desktop.ACCOUNT, "revision": 1,
                     "name": "First monitor", "prefix": "xk_first", "status": "active",
                     "issuer_credential_id": desktop.ACCOUNT, "created_at": desktop.NOW,
                     "updated_at": desktop.NOW, "last_used_at": None,
                     "inheritance": {"mode": "all_snapshot"}}
        key_second = {"id": "second-key-id", "owner_id": desktop.ACCOUNT, "revision": 1,
                      "name": "Second monitor", "prefix": "xk_second", "status": "active",
                      "issuer_credential_id": desktop.ACCOUNT, "created_at": desktop.NOW,
                      "updated_at": desktop.NOW, "last_used_at": None,
                      "inheritance": {"mode": "all_snapshot"}}
        self.key_pages = {
            None: {"items": [key_first], "next_cursor": "cursor-2"},
            "cursor-2": {"items": [key_second], "next_cursor": "cursor-end"},
            "cursor-end": {"items": [], "next_cursor": None},
        }
        self.fixture.open()
        self.fixture.app("账户与权限").get_by_role("button", name="API Key 元数据", exact=True).click()
        keys_window = self.page.locator('.window[data-app^="api-keys:"]')
        keys_window.get_by_role("heading", name="First monitor", exact=True).wait_for()
        more = keys_window.get_by_role("button", name="加载下一页API Key 元数据", exact=True)
        self.assertTrue(more.is_visible())

        # 1. Trigger failure on pagination
        self.key_error = (503, "UNAVAILABLE", "key database unavailable")
        more.press("Enter")
        keys_window.locator('.state[data-kind="unavailable"]').wait_for()
        self.assertEqual(keys_window.locator(".card").count(), 1)
        expect(more).to_be_enabled()
        expect(more).to_be_focused(timeout=2000)

        # 2. Trigger another failure on retry: error state should be replaced, not accumulated
        self.key_error = (500, "INTERNAL_ERROR", "internal failure")
        self.page.keyboard.press("Enter")
        keys_window.locator('.state[data-kind="error"]').wait_for()
        self.assertEqual(keys_window.locator(".state[data-kind='error']").count(), 1)
        self.assertEqual(keys_window.locator(".card").count(), 1)
        expect(more).to_be_enabled()
        expect(more).to_be_focused()

        # 3. Successful retry: loads the second page and moves focus to its first item.
        self.key_error = None
        self.page.keyboard.press("Enter")
        expect(keys_window.get_by_role("heading", name="Second monitor", exact=True)).to_be_focused()
        self.assertEqual(keys_window.locator(".card").count(), 2)
        self.assertEqual(keys_window.locator(".state").count(), 0)

        # 4. The empty final page still has a focus destination after the button is hidden.
        more.press("Enter")
        expect(keys_window.get_by_text("已加载全部可见项目。", exact=True)).to_be_focused()
        expect(more).to_be_hidden()

    def test_m3_catalog_waits_for_independently_loaded_installations(self):
        titles = ("M3 official WebUI", "M3 independent UI reference")
        plugin_ids = ("org.xelab.webui", "org.xelab.ui-reference")
        deployments = [
            {"id": f"00000000-0000-4000-8000-{index:012d}", "revision": 1, "phase": "ready", "conditions": [],
             "spec": {"display_name": title, "enabled": True}}
            for index, title in enumerate(titles, 1)
        ]
        installations = [
            {"id": f"00000000-0000-4001-8000-{index:012d}", "plugin_id": plugin_id,
             "version": "1.0.0", "state": "installed", "image_digest": "sha256:" + "b" * 64}
            for index, plugin_id in enumerate(plugin_ids, 1)
        ]
        held = []

        def deployment_list(route):
            if route.request.method != "GET":
                return route.fallback()
            self.reply(route, {"items": deployments, "next_cursor": None})

        def hold_installations(route):
            if route.request.method != "GET":
                return route.fallback()
            held.append(route)
            self.page.evaluate("() => { globalThis.__catalogInstallationsHeld = true; }")

        def release_installations(message):
            if message.text == "release-catalog-installations-fixture":
                self.reply(held.pop(), {"items": installations, "next_cursor": None})

        self.page.route(desktop.CORE + "/api/platform/v1/deployments?*", deployment_list)
        self.page.route(desktop.CORE + "/api/platform/v1/installations?*", hold_installations)
        self.fixture.open()
        catalog = self.fixture.app("插件中心")
        for title in titles:
            catalog.get_by_role("heading", name=title, exact=True).wait_for()
        self.page.wait_for_function("() => globalThis.__catalogInstallationsHeld === true")
        for plugin_id in plugin_ids:
            self.assertEqual(catalog.get_by_role("heading", name=plugin_id, exact=True).count(), 0)

        report = {"checks": {}}
        self.page.on("console", release_installations)
        try:
            self.page.evaluate("() => { setTimeout(() => console.debug('release-catalog-installations-fixture'), 250); }")
            desktop.DRIVER.wait_for_plugin_catalog(catalog, report)
            self.assertEqual(report["checks"], {"desktop_reads_actual_installed_plugins": True})
            self.assertEqual(held, [])
        finally:
            self.page.remove_listener("console", release_installations)
            if held:
                held.pop().abort()

    def test_plugin_catalog_pages_both_collections_and_allows_later_deployment_actions(self):
        deployments = [
            {"id": f"00000000-0000-4000-8000-{index:012d}", "revision": 1,
             "created_at": desktop.NOW, "updated_at": desktop.NOW, "observed_generation": 1,
             "phase": "disabled", "conditions": [],
             "spec": {"display_name": f"Deployment {index}", "installation_id": desktop.DEPLOYMENT,
                      "enabled": False, "replicas": 1, "config": {}, "volume_ids": [],
                      "limits": {"cpu_millis": 500, "memory_bytes": 536870912,
                                 "ephemeral_bytes": 268435456, "pids": 128}}}
            for index in range(1, 202)
        ]
        installations = [
            {"id": f"00000000-0000-4001-8000-{index:012d}", "plugin_id": f"org.xelab.fixture.installation-{index}",
             "version": "1.0.0", "state": "installed", "image_digest": "sha256:" + "b" * 64}
            for index in range(1, 202)
        ]
        collections = {"deployments": deployments, "installations": installations}
        endpoint = desktop.CORE + "/api/platform/v1"
        reads = []
        held_deployments = []

        def catalog_page(route):
            if route.request.method != "GET":
                route.fallback()
                return
            url = urlparse(route.request.url)
            collection = url.path.rsplit("/", 1)[-1]
            query = parse_qs(url.query)
            cursor = query.get("cursor", [None])[0]
            reads.append((collection, cursor, query.get("limit")))
            items = collections[collection]
            next_cursor = f"{collection}-next"
            if cursor is None:
                self.reply(route, {"items": items[:200], "next_cursor": next_cursor})
            elif collection == "deployments" and cursor == next_cursor:
                held_deployments.append(route)
                self.page.evaluate("() => { globalThis.__deploymentPageHeld = true; }")
            elif cursor == next_cursor:
                self.reply(route, {"items": items[200:], "next_cursor": None})
            else:
                self.reply(route, {"error": {"code": "INVALID_ARGUMENT", "message": "Wrong catalog cursor"}}, 400)

        self.page.route(lambda url: any(url.startswith(f"{endpoint}/{name}?") for name in collections), catalog_page)
        target = deployments[-1]
        enable_url = f"{endpoint}/deployments/{target['id']}/enabled"
        operation = {"id": desktop.ACCOUNT, "revision": 1, "created_at": desktop.NOW, "updated_at": desktop.NOW,
                     "kind": "update_deployment", "state": "pending", "progress_percent": 0,
                     "resource_id": target["id"], "result": None, "error": None}
        self.page.route(enable_url, lambda route: self.reply(route, operation, 202))

        self.fixture.open()
        self.fixture.app("插件中心")
        catalog = self.page.locator('.window[data-app="plugins"]')
        catalog.get_by_role("heading", name="Deployment 200", exact=True).wait_for()
        more_deployments = catalog.get_by_role("button", name="加载下一页部署", exact=True)
        more_installations = catalog.get_by_role("button", name="加载下一页安装制品", exact=True)
        more_deployments.wait_for()
        more_installations.wait_for()
        initial_reads = [("deployments", None, ["200"]), ("installations", None, ["200"])]
        self.assertCountEqual(reads, initial_reads)

        try:
            more_deployments.press("Enter")
            self.page.wait_for_function("() => globalThis.__deploymentPageHeld === true")
            expect(more_deployments).to_be_focused(timeout=2000)
            expect(more_deployments).to_be_disabled()
            self.page.keyboard.press("Space")
            self.reply(held_deployments.pop(0), {"items": deployments[200:], "next_cursor": None})

            target_card = catalog.locator("article").filter(has_text=target["spec"]["display_name"])
            expect(target_card.get_by_role("heading", name="Deployment 201", exact=True)).to_be_focused()
            self.page.keyboard.press("Tab")
            expect(target_card.get_by_role("button", name="详情", exact=True)).to_be_focused()
            self.page.keyboard.press("Tab")
            expect(target_card.get_by_role("button", name="启用", exact=True)).to_be_focused()
            self.page.keyboard.press("Enter")
        finally:
            for route in held_deployments:
                route.abort()
        editor = self.page.locator('.window[data-app="editor-setDeploymentEnabled"]')
        with self.page.expect_request(lambda request: request.url == enable_url and request.method == "PUT") as enabled:
            editor.get_by_role("button", name="提交", exact=True).click()
        self.assertEqual(json.loads(enabled.value.post_data), {"enabled": True})
        self.assertEqual(enabled.value.all_headers()["if-match"], '"rev-1"')
        editor.get_by_text("pending", exact=False).wait_for()
        editor.get_by_role("button", name="关闭改变部署启停", exact=True).click()
        self.assertTrue(more_deployments.is_hidden())
        self.assertEqual(reads[2:], [("deployments", "deployments-next", ["200"])])

        more_installations.click()
        catalog.get_by_role("heading", name=installations[-1]["plugin_id"], exact=True).wait_for()
        self.assertTrue(more_installations.is_hidden())
        self.assertEqual(reads[2:], [("deployments", "deployments-next", ["200"]),
                                     ("installations", "installations-next", ["200"])])

    def test_plugin_catalog_keeps_deployment_actions_during_an_installation_error(self):
        unavailable = True
        installation = {"id": desktop.ACCOUNT, "plugin_id": "org.xelab.comp-test", "version": "1.0.0",
                        "state": "installed", "image_digest": "sha256:" + "a" * 64}
        statuses = []

        def installations(route):
            if route.request.method != "GET":
                return route.fallback()
            status = 503 if unavailable else 200
            statuses.append(status)
            data = ({"error": {"code": "UNAVAILABLE", "message": "installation dependency unavailable"}}
                    if unavailable else {"items": [installation], "next_cursor": None})
            self.reply(route, data, status)

        self.page.route(desktop.CORE + "/api/platform/v1/installations?*", installations)
        self.fixture.open()
        catalog = self.fixture.app("插件中心")
        catalog.get_by_role("heading", name="当前不可用", exact=True).wait_for()
        installs = catalog.get_by_role("region", name="安装制品", exact=True)
        self.assertTrue(installs.get_by_role("heading", name="当前不可用", exact=True).is_visible())
        deployments = catalog.get_by_role("region", name="部署", exact=True)
        deployment = deployments.locator("article").filter(has_text="fixture deployment")
        deployment.get_by_role("button", name="配置", exact=True).click()
        editor = self.page.locator('.window[data-app="editor-updateDeployment"]')
        self.assertEqual(json.loads(editor.get_by_label("请求 JSON", exact=True).input_value())["spec"]["display_name"],
                         "fixture deployment")
        editor.get_by_role("button", name="关闭修改部署", exact=True).click()

        unavailable = False
        catalog.get_by_role("button", name="刷新插件中心", exact=True).click()
        installs.get_by_role("heading", name=installation["plugin_id"], exact=True).wait_for()
        deployment.get_by_role("button", name="配置", exact=True).wait_for()
        self.assertEqual(installs.get_by_role("heading", name="当前不可用", exact=True).count(), 0)
        self.assertEqual(statuses, [503, 200])

    def test_skill_pagination_reports_empty_only_after_exhausting_all_pages(self):
        self.fixture.open()
        endpoint = desktop.CORE + "/api/platform/v1/skills?"

        def skill_pages(route):
            if route.request.method != "GET":
                route.fallback()
                return
            cursor = parse_qs(urlparse(route.request.url).query).get("cursor", [None])[0]
            if cursor is None:
                self.reply(route, {"items": [], "next_cursor": "next-skills"})
            elif terminal_empty:
                self.reply(route, {"items": [], "next_cursor": None})
            else:
                route.fallback()

        self.page.route(lambda url: url.startswith(endpoint), skill_pages)
        for terminal_empty in (False, True):
            with self.subTest(terminal_empty=terminal_empty):
                window = self.fixture.app("Skill")
                more = window.get_by_role("button", name="加载下一页Skill 浏览器", exact=True)
                empty = window.get_by_text("暂无可见项目", exact=True)
                more.wait_for()
                empty_on_first_page = empty.is_visible()

                more.click()
                more.wait_for(state="hidden")
                self.assertEqual(window.get_by_role("heading", name="component.echo", exact=True).is_visible(),
                                 not terminal_empty)
                self.assertEqual((empty_on_first_page, empty.is_visible()), (False, terminal_empty))
            window.get_by_role("button", name="关闭Skill", exact=True).click()

    def test_toast_redacts_server_error_messages_across_actions(self):
        value = self.window_contribution("toast-redact")
        self.fixture.open()

        # 1. apiKeys detail button (getApiKey 500) redacts server error.message in #toast
        self.fixture.app("账户与权限").get_by_role("button", name="API Key 元数据", exact=True).click()
        keys_window = self.page.locator('.window[data-app^="api-keys:"]')
        keys_window.get_by_role("heading", name="Read-only monitor", exact=True).wait_for()
        secret_key = "TOKEN=toast-secret-api-key"
        self.key_error = (500, "INTERNAL_ERROR", secret_key)
        keys_window.get_by_role("button", name="读取密钥元数据", exact=True).click()
        self.page.locator("#toast").get_by_text("INTERNAL_ERROR", exact=False).wait_for()
        self.assertNotIn(secret_key, self.page.locator("body").inner_text())
        self.key_error = None
        keys_window.get_by_role("button", name="关闭API Key 元数据", exact=True).click()

        # 2. accounts toolbar action (getEffectivePermissions 403) redacts server error.message
        secret_perm = "TOKEN=toast-secret-effective-perms"
        self.account_action_error = (403, "PERMISSION_DENIED", secret_perm)
        self.fixture.app("账户与权限").get_by_role("button", name="有效权限与阻断祖先", exact=True).click()
        self.page.locator("#toast").get_by_text("PERMISSION_DENIED", exact=False).wait_for()
        self.assertNotIn(secret_perm, self.page.locator("body").inner_text())
        self.account_action_error = None

        # 3. launcher pin failure (503 UNAVAILABLE) redacts explicit server error message
        self.fail_preferences = 1
        self.open_launcher()
        self.page.locator("#app-search").fill(value["spec"]["title"])
        self.page.get_by_role("button", name="固定 " + value["spec"]["title"], exact=True).click()
        self.page.locator("#toast").get_by_text("UNAVAILABLE", exact=False).wait_for()
        self.assertNotIn("explicit component outage", self.page.locator("body").inner_text())
        self.page.locator("#launcher-close").click()

        # 4. Closing settings mid-flight aborts initial preference read without spurious toast
        self.page.evaluate("document.querySelector('#toast').textContent = ''")
        self.fixture.hold_preferences = True
        settings_win = self.fixture.app("设置")
        while not self.fixture.pending_preferences:
            self.page.wait_for_timeout(10)
        settings_win.get_by_role("button", name="关闭设置", exact=True).click()
        self.fixture.hold_preferences = False
        pending = self.fixture.pending_preferences.pop()
        self.reply(pending, {"error": {"code": "UNAVAILABLE", "message": "secret-after-settings-close"}}, 503)
        self.page.wait_for_timeout(120)
        self.assertEqual(self.page.locator("#toast").inner_text(), "")
        self.assertNotIn("secret-after-settings-close", self.page.locator("body").inner_text())

    def test_widget_removal_during_load_prevents_stale_mount_and_polling(self):
        value = contribution("stale_widget", "widget")
        value["spec"]["refresh_interval_seconds"] = 30
        value["contribution_digest"] = "sha256:" + hashlib.sha256(
            json.dumps(value["spec"], sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        self.fixture.contributions["stale_widget"] = value
        self.fixture.open()

        self.open_launcher()
        self.page.get_by_role("button", name="启动插件页面", exact=True).click()
        listing = self.page.locator('.window[data-app="contributions"]')
        listing.get_by_role("heading", name="Independent fixture", exact=True).wait_for()

        # Part 1: Hold getUiContribution during renderWidgets(), then remove the widget before mountContribution runs.
        self.arm_hold_widget_after_put = True
        listing.get_by_role("button", name="添加桌面组件", exact=True).click()
        listing.get_by_role("button", name="关闭插件页面", exact=True).click()
        card = self.page.locator("#widgets .widget-card")
        card.wait_for(state="visible")
        while not self.held_widget_routes:
            self.page.wait_for_timeout(10)
        card.get_by_role("button", name="移除", exact=True).click()
        self.page.wait_for_function("() => document.querySelectorAll('#widgets .widget-card').length === 0")

        self.hold_widget_lookup = False
        for held in self.held_widget_routes:
            try:
                held.fallback()
            except Exception:
                pass
        self.held_widget_routes.clear()
        self.page.wait_for_timeout(150)

        self.assertEqual(self.page.locator("#widgets .widget-card").count(), 0)
        self.assertEqual(self.page.locator("#widgets iframe").count(), 0)
        self.assertFalse(any("/installations/" in call["path"] for call in self.fixture.calls))
        self.assertEqual(self.page.locator("#toast").inner_text(), "")

        # Part 2: Mount the widget with refresh_interval_seconds=30, remove it while verifyPureWidget fetches
        # getInstallation, and confirm no getSkill polling verification or DOM mutation continues afterward.
        self.open_launcher()
        self.page.get_by_role("button", name="启动插件页面", exact=True).click()
        listing = self.page.locator('.window[data-app="contributions"]')
        listing.get_by_role("heading", name="Independent fixture", exact=True).wait_for()
        self.hold_installation = True
        listing.get_by_role("button", name="添加桌面组件", exact=True).click()
        listing.get_by_role("button", name="关闭插件页面", exact=True).click()
        card = self.page.locator("#widgets .widget-card")
        card.wait_for(state="visible")
        while not self.held_installation_routes:
            self.page.wait_for_timeout(10)
        card.get_by_role("button", name="移除", exact=True).click()
        self.page.wait_for_function("() => document.querySelectorAll('#widgets .widget-card').length === 0")

        self.hold_installation = False
        for held in self.held_installation_routes:
            try:
                self.reply(held, {"error": {"code": "UNAVAILABLE", "message": "late"}}, 503)
            except Exception:
                pass
        self.held_installation_routes.clear()
        self.page.wait_for_timeout(150)

        self.assertEqual(self.page.locator("#widgets .widget-card").count(), 0)
        self.assertEqual(self.page.locator("#widgets iframe").count(), 0)
        self.assertFalse(any("/skills/" in call["path"] for call in self.fixture.calls))
        self.assertEqual(self.page.locator("#toast").inner_text(), "")

    def test_account_lookup_ignores_stale_response_and_api_keys_window_not_persisted(self):
        slow_id = "00000000-0000-4000-8000-00000000000a"
        fast_id = "00000000-0000-4000-8000-00000000000b"
        self.hold_account_ids.add(slow_id)
        self.fast_accounts[fast_id] = {"id": fast_id, "display_name": "Fresh Fast Account", "status": "active"}

        self.fixture.open()
        accounts_win = self.fixture.app("账户与权限")
        accounts_win.get_by_role("heading", name="Component account · active", exact=True).wait_for()
        uuid_input = accounts_win.get_by_label("账户 UUID")
        read_btn = accounts_win.get_by_role("button", name="读取", exact=True)

        uuid_input.fill(slow_id)
        read_btn.click()
        while slow_id not in self.held_account_routes:
            self.page.wait_for_timeout(10)

        uuid_input.fill(fast_id)
        read_btn.click()
        accounts_win.get_by_text("Fresh Fast Account · active", exact=True).wait_for()

        self.hold_account_ids.discard(slow_id)
        self.reply(self.held_account_routes.pop(slow_id), {"id": slow_id, "display_name": "Stale Slow Account", "status": "active"})
        self.page.wait_for_timeout(120)

        self.assertIn("Fresh Fast Account", accounts_win.inner_text())
        self.assertNotIn("Stale Slow Account", accounts_win.inner_text())

        # Opening API Key 元数据 creates an ephemeral window excluded from persisted localStorage layout.
        accounts_win.get_by_role("button", name="API Key 元数据", exact=True).click()
        self.page.locator('.window[data-app^="api-keys:"]').wait_for()
        saved_apps = self.page.evaluate("""() => {
          const layouts = [];
          for (let i = 0; i < localStorage.length; i++) {
            const k = localStorage.key(i);
            if (k && k.startsWith('xelab:windows:')) layouts.push(...JSON.parse(localStorage.getItem(k) || '[]'));
          }
          return layouts.map(item => item.app);
        }""")
        self.assertFalse(any(app.startswith("api-keys:") for app in saved_apps), saved_apps)

    def test_account_follow_up_reads_belong_to_the_selected_account(self):
        other = "00000000-0000-4000-8000-000000000099"
        old_request_id = "00000000-0000-4000-8000-000000000079"
        self.fast_accounts[other] = {"id": other, "display_name": "Second Account", "status": "active"}
        self.page.add_init_script("""(() => {
          const original = Response.prototype.arrayBuffer;
          globalThis.__staleFollowUpsRead = 0;
          Response.prototype.arrayBuffer = async function(...args) {
            const bytes = await original.apply(this, args);
            if (this.headers.get('x-request-id') === REQUEST_ID) ++globalThis.__staleFollowUpsRead;
            return bytes;
          };
        })()""".replace("REQUEST_ID", json.dumps(old_request_id)))

        def quota(account_id):
            return {"id": account_id, "account_id": account_id, "revision": 1,
                    "created_at": desktop.NOW, "updated_at": desktop.NOW,
                    "spec": {"maximum_depth": 8, "maximum_descendants": 30, "maximum_api_keys": 5,
                             "maximum_concurrent_calls": 3, "daily_calls": 50},
                    "descendants_used": 0, "api_keys_used": 0, "concurrent_calls_used": 0, "daily_calls_used": 0}

        endpoint = desktop.CORE + "/api/platform/v1/accounts"
        held = {}

        def hold_follow_up(route):
            if route.request.method != "GET":
                route.fallback()
                return
            key = "quota" if urlparse(route.request.url).path.endswith("/quota") else "children"
            held[key] = route
            self.page.evaluate("count => { globalThis.__heldFollowUps = count; }", len(held))

        self.page.route(f"{endpoint}/{desktop.ACCOUNT}/quota", hold_follow_up)
        self.page.route(lambda url: url.startswith(endpoint + "?") and
                        parse_qs(urlparse(url).query).get("parent_id") == [desktop.ACCOUNT], hold_follow_up)
        self.page.route(f"{endpoint}/{other}/quota", lambda route: self.reply(route, quota(other)))
        self.fixture.open()
        self.fixture.app("账户与权限")
        accounts = self.page.locator('.window[data-app="accounts"]')
        accounts.get_by_role("button", name="资源配额", exact=True).click()
        accounts.get_by_role("button", name="子账户", exact=True).click()
        self.page.wait_for_function("() => globalThis.__heldFollowUps === 2")

        selected = accounts.get_by_label("账户 UUID", exact=True)
        selected.fill(other)
        selected.press("Enter")
        accounts.get_by_role("heading", name="Second Account · active", exact=True).wait_for()
        self.reply(held.pop("quota"), quota(desktop.ACCOUNT), request_id=old_request_id)
        self.page.wait_for_function("() => globalThis.__staleFollowUpsRead === 1")
        self.reply(held.pop("children"), {"error": {"code": "PERMISSION_DENIED", "message": "Fixture state"}},
                   403, request_id=old_request_id)
        self.page.wait_for_function("() => globalThis.__staleFollowUpsRead === 2")

        details = self.page.locator('.window[data-app="details"]')
        self.assertEqual((details.count(), self.page.locator("#toast").inner_text(),
                          self.page.locator("#connection").inner_text()), (0, "", "已连接核心"))
        accounts.get_by_role("button", name="资源配额", exact=True).click()
        details.get_by_text(other, exact=False).wait_for()
        self.assertEqual(details.count(), 1)

    def test_invalid_operation_uuid_shows_local_validation_without_a_core_request(self):
        self.fixture.open()
        self.open_launcher()
        self.page.get_by_role("button", name="启动任务与知识库", exact=True).click()
        window = self.page.locator('.window[data-app="tasks"]')
        window.get_by_role("textbox", name="Operation UUID", exact=True).fill("not-a-valid-uuid")
        window.get_by_role("button", name="查询 Operation", exact=True).click()
        error = window.locator('.state[data-kind="error"]')
        error.get_by_role("heading", name="请求失败", exact=True).wait_for()
        self.assertIn("INVALID_ARGUMENT · 无效的请求参数。", error.inner_text())
        self.assertFalse(any(call["path"].startswith("/api/platform/v1/operations/")
                             for call in self.fixture.calls))

    def test_task_toolbar_refresh_reads_the_last_operation_in_that_window(self):
        first_id = desktop.DEPLOYMENT
        other_id = "00000000-0000-4000-8000-000000000109"
        pending = {"id": first_id, "kind": "installPlugin", "state": "pending", "progress_percent": 0,
                   "revision": 1, "created_at": desktop.NOW, "updated_at": desktop.NOW,
                   "resource_id": None, "result": None, "error": None}
        self.operation_responses[first_id] = (200, pending)
        self.operation_responses[other_id] = (200, {**pending, "id": other_id, "state": "running"})
        reads = []
        operation_url = desktop.CORE + "/api/platform/v1/operations/"

        def record_read(request):
            if request.method == "GET" and request.url.startswith(operation_url):
                reads.append(request.url.removeprefix(operation_url))

        self.page.on("request", record_read)
        self.fixture.open()
        windows = self.page.locator('.window[data-app="tasks"]')
        for index, (operation_id, state) in enumerate(((first_id, "pending"), (other_id, "running"))):
            self.open_launcher()
            self.page.get_by_role("button", name="启动任务与知识库", exact=True).click()
            window = windows.nth(index)
            field = window.get_by_role("textbox", name="Operation UUID", exact=True)
            self.assertEqual(field.input_value(), "")
            self.assertEqual(len(reads), index)
            field.fill(operation_id)
            field.press("Enter")
            window.get_by_text(f'"state": "{state}"', exact=False).wait_for()

        self.operation_responses[first_id] = (200, {**pending, "state": "succeeded",
                                                   "revision": 2, "progress_percent": 100})
        self.open_launcher()
        self.page.get_by_role("button", name="聚焦窗口1 任务与知识库", exact=True).click()
        first = windows.first
        first.get_by_role("button", name="刷新任务与能力", exact=True).click()
        self.assertEqual(first.get_by_role("textbox", name="Operation UUID", exact=True).input_value(), first_id)
        first.get_by_text('"state": "succeeded"', exact=False).wait_for()
        other = windows.nth(1)
        self.assertEqual(other.get_by_role("textbox", name="Operation UUID", exact=True).input_value(), other_id)
        other.get_by_text('"state": "running"', exact=False).wait_for()
        self.assertEqual(reads, [first_id, other_id, first_id])

        self.page.reload()
        self.page.wait_for_function("""document.querySelectorAll(
            '.window[data-app="tasks"] input[aria-label="Operation UUID"]').length === 2""")
        for window in windows.all():
            self.assertEqual(window.get_by_role("textbox", name="Operation UUID", exact=True).input_value(), "")
        self.assertEqual(reads, [first_id, other_id, first_id])

    def test_settings_save_error_renders_valid_live_status_and_recovers(self):
        self.fail_preferences = 1
        self.fixture.open()
        settings_win = self.fixture.app("设置")
        settings_win.get_by_role("button", name="保存到当前账户", exact=True).click()
        status = settings_win.locator('[role="status"][aria-live="polite"]')
        error = status.locator('.state[data-kind="unavailable"]')
        error.wait_for(state="visible")
        self.assertEqual(self.page.evaluate(
            "() => document.querySelectorAll('.window[data-app=\"settings\"] p .state, .window[data-app=\"settings\"] .theme-preview-note .state').length"
        ), 0)

        settings_win.get_by_label("主题模式", exact=True).select_option("dark")
        status.locator("p.theme-preview-note").get_by_text("当前为未保存预览。", exact=True).wait_for()
        self.assertEqual(status.locator(".state").count(), 0)

        settings_win.get_by_role("button", name="保存到当前账户", exact=True).click()
        status.locator("p.theme-preview-note").get_by_text("已保存到核心，重新登录后仍可读取。", exact=True).wait_for()
        self.assertEqual(len(self.custom_calls), 2)

    def test_settings_windows_save_their_own_edits_and_keep_other_preferences(self):
        _, placement = self.widget()
        contribution = self.window_contribution()
        pin = {"deployment_id": contribution["deployment_id"], "contribution_id": contribution["spec"]["id"]}
        self.fixture.saved["spec"]["theme"]["mode"] = "light"
        self.fixture.saved["spec"]["dock"]["pinned"] = [pin]
        self.fixture.open()

        first_id = self.fixture.app("设置").get_attribute("data-view-id")
        first = self.page.locator(f'.window[data-view-id="{first_id}"]')
        first.get_by_label("主题模式", exact=True).select_option("custom")

        second = self.fixture.app("设置")
        second.get_by_label("Dock 位置", exact=True).select_option("right")
        second.get_by_label("减少动画", exact=True).check()
        second.get_by_role("button", name="保存到当前账户", exact=True).click()
        second.get_by_text("已保存到核心，重新登录后仍可读取。", exact=True).wait_for()
        second.get_by_role("button", name="关闭设置", exact=True).click()

        self.assertEqual(first.get_by_label("主题模式", exact=True).input_value(), "custom")
        first.get_by_label("自定义 accent", exact=True).fill("#123456")
        first.get_by_role("button", name="保存到当前账户", exact=True).click()
        first.get_by_text("已保存到核心，重新登录后仍可读取。", exact=True).wait_for()

        first_write, second_write = self.custom_calls
        self.assertEqual(first_write["headers"]["if-match"], '"rev-1"')
        self.assertEqual(second_write["headers"]["if-match"], '"rev-2"')
        saved = json.loads(second_write["body"])
        self.assertEqual(saved, self.fixture.saved["spec"])
        self.assertEqual(saved["theme"]["mode"], "custom")
        self.assertEqual(saved["theme"]["custom_tokens"]["accent"], "#123456")
        self.assertTrue(saved["theme"]["reduce_motion"])
        self.assertEqual(saved["dock"]["position"], "right")
        self.assertEqual(saved["dock"]["pinned"], [pin])
        self.assertEqual(saved["widgets"], [placement])

    def test_settings_delayed_save_ack_keeps_newest_revision_and_pending_edits(self):
        self.fixture.open()
        endpoint = desktop.CORE + "/api/platform/v1/me/ui-preferences"
        held = []

        def commit_first_and_hold_response(route):
            request = route.request
            if request.method != "PUT":
                route.fallback()
                return
            revision = self.fixture.saved["revision"]
            if request.all_headers()["if-match"] != f'"rev-{revision}"':
                self.reply(route, {"error": {"code": "ETAG_MISMATCH", "message": "Resource revision changed"}}, 412)
            elif revision == 1:
                self.fixture.saved["spec"] = json.loads(request.post_data)
                self.fixture.saved["revision"] = 2
                held.append((route, copy.deepcopy(self.fixture.saved)))
                self.page.evaluate("() => { globalThis.__preferenceResponseHeld = true; }")
            else:
                route.fallback()

        self.page.route(endpoint, commit_first_and_hold_response)
        first_id = self.fixture.app("设置").get_attribute("data-view-id")
        first = self.page.locator(f'.window[data-view-id="{first_id}"]')
        first.get_by_label("主题模式", exact=True).select_option("dark")
        save = first.get_by_role("button", name="保存到当前账户", exact=True)
        save.click()
        self.page.wait_for_function("() => globalThis.__preferenceResponseHeld === true")

        second = self.fixture.app("设置")
        second.get_by_label("主题模式", exact=True).select_option("light")
        second.get_by_label("减少动画", exact=True).check()
        second.get_by_role("button", name="保存到当前账户", exact=True).click()
        second.get_by_text("已保存到核心，重新登录后仍可读取。", exact=True).wait_for()
        second.get_by_role("button", name="关闭设置", exact=True).click()
        self.assertEqual(self.fixture.saved["revision"], 3)

        first.get_by_label("自动隐藏 Dock", exact=True).check()
        delayed_route, response = held.pop()
        delayed_route.fulfill(status=200, content_type="application/json", headers={
            "access-control-allow-origin": desktop.SHELL,
            "access-control-expose-headers": "etag",
            "etag": '"rev-2"',
        }, body=json.dumps(response))
        self.page.wait_for_function("button => !button.disabled", arg=save.element_handle())
        self.assertTrue(first.get_by_text("当前为未保存预览。", exact=True).is_visible())

        with self.page.expect_response(lambda response: response.url == endpoint and response.request.method == "PUT") as next_save:
            save.click()
        self.assertEqual(next_save.value.status, 200)
        first.get_by_text("已保存到核心，重新登录后仍可读取。", exact=True).wait_for()
        self.assertEqual(self.custom_calls[-1]["headers"]["if-match"], '"rev-3"')
        self.assertEqual(self.fixture.saved["spec"]["theme"]["mode"], "light")
        self.assertTrue(self.fixture.saved["spec"]["theme"]["reduce_motion"])
        self.assertTrue(self.fixture.saved["spec"]["dock"]["auto_hide"])

    def test_accounts_and_tasks_submit_on_enter_and_tasks_ignores_stale_response(self):
        acct_enter_id = "00000000-0000-4000-8000-000000000011"
        op_slow = "00000000-0000-4000-8000-000000000021"
        op_fast = "00000000-0000-4000-8000-000000000022"
        op_err_slow = "00000000-0000-4000-8000-000000000023"
        self.fast_accounts[acct_enter_id] = {"id": acct_enter_id, "display_name": "Enter Account", "status": "active"}
        self.operation_responses[op_fast] = (200, {"id": op_fast, "status": "succeeded", "marker": "fresh-fast-op"})

        self.fixture.open()
        accounts_win = self.fixture.app("账户与权限")
        accounts_win.get_by_role("heading", name="Component account · active", exact=True).wait_for()
        uuid_input = accounts_win.get_by_label("账户 UUID")
        uuid_input.fill(acct_enter_id)
        uuid_input.press("Enter")
        accounts_win.get_by_role("heading", name="Enter Account · active", exact=True).wait_for()

        self.open_launcher()
        self.page.get_by_role("button", name="启动任务与知识库", exact=True).click()
        tasks_win = self.page.locator('.window[data-app="tasks"]')
        op_input = tasks_win.get_by_role("textbox", name="Operation UUID", exact=True)

        self.hold_operation_ids.add(op_slow)
        op_input.fill(op_slow)
        op_input.press("Enter")
        while op_slow not in self.held_operation_routes:
            self.page.wait_for_timeout(10)

        op_input.fill(op_fast)
        op_input.press("Enter")
        tasks_win.get_by_text("fresh-fast-op", exact=False).wait_for()

        self.hold_operation_ids.discard(op_slow)
        self.reply(self.held_operation_routes.pop(op_slow), {"id": op_slow, "status": "succeeded", "marker": "stale-slow-op"})
        self.page.wait_for_timeout(120)
        self.assertIn("fresh-fast-op", tasks_win.inner_text())
        self.assertNotIn("stale-slow-op", tasks_win.inner_text())

        self.hold_operation_ids.add(op_err_slow)
        op_input.fill(op_err_slow)
        op_input.press("Enter")
        while op_err_slow not in self.held_operation_routes:
            self.page.wait_for_timeout(10)

        op_input.fill(op_fast)
        op_input.press("Enter")
        tasks_win.get_by_text("fresh-fast-op", exact=False).wait_for()

        self.hold_operation_ids.discard(op_err_slow)
        self.reply(self.held_operation_routes.pop(op_err_slow), {"error": {"code": "UNAVAILABLE", "message": "stale-error-op"}}, 503)
        self.page.wait_for_timeout(120)
        self.assertIn("fresh-fast-op", tasks_win.inner_text())
        self.assertEqual(tasks_win.get_by_role("heading", name="当前不可用", exact=True).count(), 0)

    def test_repeated_credential_rotations_do_not_duplicate_editor_lock_banner(self):
        self.page.clock.set_fixed_time("2026-11-01T12:00:00Z")
        self.fixture.refresh_enabled = True
        self.allow_second_refresh = True
        self.fixture.open(expires_ms=45000, refresh_token="component-refresh")

        skills = self.fixture.app("Skill")
        skills.get_by_role("button", name="调用测试", exact=True).click()
        editor = self.page.locator('.window[data-app="editor-invokeSkill"]')
        editor.wait_for(state="visible")

        self.page.clock.set_fixed_time("2026-11-01T12:00:16Z")
        resources = self.fixture.app("系统状态")
        resources.get_by_text('"component-only"', exact=False).wait_for()
        editor.get_by_text("原请求已锁定", exact=True).wait_for()
        self.assertEqual(editor.locator('.state[data-kind="unavailable"]').count(), 1)

        self.page.clock.set_fixed_time("2026-11-01T12:15:00Z")
        accounts_win = self.fixture.app("账户与权限")
        accounts_win.get_by_role("heading", name="Component account · active", exact=True).wait_for()
        self.assertEqual(len(self.custom_calls), 1)
        self.assertEqual(editor.get_by_role("heading", name="原请求已锁定", exact=True).count(), 1)
        self.assertEqual(editor.locator('.state[data-kind="unavailable"]').count(), 1)
        self.assertTrue(editor.get_by_role("button", name="提交", exact=True).is_disabled())


if __name__ == "__main__":
    unittest.main()
