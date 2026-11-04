#!/usr/bin/env python3
"""Real Chromium shell interactions with explicit routed API adapters, not Core/OCI."""
import copy
import hashlib
import json
import unittest
from urllib.parse import parse_qs, urlparse

import test_browser as desktop
from test_bridge import contribution


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
        self.key_denied = False
        self.subject = desktop.ACCOUNT
        self.page.route("**/*", self.route)

    def tearDown(self):
        self.fixture.tearDown()

    def reply(self, route, data, status=200):
        route.fulfill(status=status, content_type="application/json", headers={
            "access-control-allow-origin": desktop.SHELL,
            "access-control-allow-headers": "authorization,content-type,if-match,idempotency-key",
            "access-control-allow-methods": "GET,PUT,OPTIONS",
            "access-control-expose-headers": "etag,x-request-id",
            "etag": '"rev-1"', "x-request-id": desktop.ACCOUNT,
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
            if self.key_denied:
                self.reply(route, {"error": {"code": "PERMISSION_DENIED", "message": "component ceiling denied"}}, 403)
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

    def pin_from_launcher(self, value):
        self.open_launcher()
        self.page.locator("#app-search").fill(value["spec"]["title"])
        self.page.get_by_role("button", name="固定 " + value["spec"]["title"], exact=True).click()
        self.page.get_by_role("button", name="取消固定 " + value["spec"]["title"], exact=True).last.wait_for()
        self.page.locator("#launcher-close").click()
        self.page.get_by_role("button", name="打开固定贡献 " + value["spec"]["title"], exact=True).wait_for()

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
        window.locator("iframe").wait_for()
        self.assertEqual(window.locator("iframe").get_attribute("sandbox"), "allow-scripts allow-same-origin")
        self.assertTrue(window.locator("iframe").get_attribute("src").startswith(second["frame_url"] + "#protocol=xelab.ui%2F1"))
        self.page.get_by_role("button", name="取消固定 " + second["spec"]["title"], exact=True).click()
        self.page.wait_for_function("() => !document.querySelector('.dock-pin')")
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [])
        self.assertEqual(len(self.custom_calls), 2)
        self.assertEqual(self.custom_calls[0]["headers"]["if-match"], '"rev-1"')
        self.assertEqual(self.custom_calls[1]["headers"]["if-match"], '"rev-2"')
        self.assertNotEqual(self.custom_calls[0]["headers"]["idempotency-key"], self.custom_calls[1]["headers"]["idempotency-key"])

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
        self.assertEqual(len([call for call in self.fixture.calls if call["method"] == "POST"]), 1)
        self.assertNotIn("BUSINESS-RESULT-NOT-LAYOUT", self.page.locator("body").inner_text())

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


if __name__ == "__main__":
    unittest.main()
