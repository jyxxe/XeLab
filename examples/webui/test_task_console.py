#!/usr/bin/env python3
"""Task console browser tests against explicit core response fixtures."""
import json
import unittest
from urllib.parse import parse_qs, urlparse

from playwright.sync_api import expect

import test_browser as desktop


BASE = "/api/platform/v1"
TASKS = f"{BASE}/tasks"
TASK = "00000000-0000-4000-8000-000000000101"
WORKFLOW = "00000000-0000-4000-8000-000000000201"
STEP = "00000000-0000-4000-8000-000000000301"
EXECUTION = "00000000-0000-4000-8000-000000000401"
DETAIL = f"{TASKS}/{TASK}"
STEPS = f"{DETAIL}/steps"
ATTEMPTS = f"{BASE}/task-steps/{STEP}/attempts"
OPAQUE_CURSOR = "opaque +/= cursor"
OLD_REQUEST_ID = "00000000-0000-4000-8000-000000000501"


def page(items=(), cursor=None):
    return {"items": list(items), "next_cursor": cursor}


def task(state="running"):
    return {"id": TASK, "workflow_id": WORKFLOW, "state": state,
            "owner_id": desktop.ACCOUNT, "revision": 1, "created_at": desktop.NOW,
            "updated_at": desktop.NOW, "deadline_at": "2026-11-20T00:00:00Z",
            "input": {"value": "private task input"},
            "output": {"value": "private task output"},
            "error": {"message": "private task diagnostics"}}


class TaskConsole(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        desktop.DesktopBrowser.setUpClass()

    @classmethod
    def tearDownClass(cls):
        desktop.DesktopBrowser.tearDownClass()

    def setUp(self):
        self.fixture = desktop.DesktopBrowser(methodName="runTest")
        self.fixture.setUp()
        self.browser = self.fixture.page
        self.responses = {}
        self.requests = []
        self.browser.route("**/*", self.route)

    def tearDown(self):
        self.fixture.tearDown()

    def route(self, route):
        request = route.request
        url = urlparse(request.url)
        if f"{url.scheme}://{url.netloc}" != desktop.CORE or url.path not in {TASKS, DETAIL, STEPS, ATTEMPTS}:
            route.fallback()
            return
        headers = {"access-control-allow-origin": desktop.SHELL,
                   "access-control-allow-headers": "authorization",
                   "access-control-allow-methods": "GET,OPTIONS",
                   "access-control-expose-headers": "x-request-id"}
        if request.method == "OPTIONS":
            route.fulfill(status=204, headers=headers)
            return
        query = parse_qs(url.query)
        self.requests.append((request.method, url.path, query, request.post_data))
        status, body, request_id = self.responses[(url.path, query.get("cursor", [None])[0])]
        route.fulfill(status=status, headers={**headers, "x-request-id": request_id},
                      content_type="application/json", body=json.dumps(body))

    def reply(self, path, body, *, cursor=None, status=200, request_id=desktop.ACCOUNT):
        self.responses[path, cursor] = status, body, request_id

    def open_console(self, authenticated=True):
        self.fixture.open(authenticated=authenticated)
        self.browser.get_by_role("button", name="打开应用启动器", exact=True).click()
        self.browser.get_by_role("button", name="启动任务与知识库", exact=True).click()
        return self.browser.locator('.window[data-app="tasks"]')

    def open_detail(self, console):
        console.get_by_role("button", name=f"查看任务 {TASK}", exact=True).click()
        return self.browser.locator('.window[data-app="task-detail"]')

    def test_read_only_task_hierarchy_and_bounded_opaque_pagination(self):
        step_key = '<img src=x onerror="alert(1)">'
        self.reply(TASKS, page(cursor=OPAQUE_CURSOR))
        self.reply(TASKS, page([task()]), cursor=OPAQUE_CURSOR)
        self.reply(DETAIL, task("succeeded"))
        self.reply(STEPS, page(cursor="steps-next"))
        self.reply(STEPS, page([{"id": STEP, "task_id": TASK, "step_key": step_key,
                                 "state": "succeeded", "attempt_count": 1}]), cursor="steps-next")
        self.reply(ATTEMPTS, page([{"step_id": STEP, "number": 1, "state": "succeeded", "fence": 2,
                                    "execution_id": EXECUTION, "lease_expires_at": desktop.NOW,
                                    "idempotency_key": "private step key"}]))
        console = self.open_console()
        scope = console.get_by_role("region", name="任务列表")
        more = scope.get_by_role("button", name="加载下一页任务")
        expect(more).to_be_visible()
        expect(scope.get_by_text("暂无可见项目", exact=True)).to_have_count(0)
        more.press("Enter")
        expect(scope.get_by_role("heading", name=TASK, exact=True)).to_be_focused()
        expect(console.get_by_role("textbox", name="Operation UUID")).to_be_visible()
        self.assertTrue(desktop.DRIVER.future_features_unavailable(console))

        detail = self.open_detail(console)
        expect(detail.get_by_text("succeeded", exact=True)).to_be_visible()
        detail.get_by_role("button", name="加载下一页步骤").click()
        expect(detail.get_by_role("heading", name=step_key, exact=True)).to_be_visible()
        expect(detail.locator("img")).to_have_count(0)
        detail.get_by_role("button", name=f"查看步骤 {step_key} 的尝试", exact=True).click()
        attempts = self.browser.locator('.window[data-app="task-attempts"]')
        expect(attempts.get_by_text(EXECUTION, exact=True)).to_be_visible()

        self.assertEqual(self.requests, [
            ("GET", TASKS, {"limit": ["20"]}, None),
            ("GET", TASKS, {"limit": ["20"], "cursor": [OPAQUE_CURSOR]}, None),
            ("GET", DETAIL, {}, None),
            ("GET", STEPS, {"limit": ["20"]}, None),
            ("GET", STEPS, {"limit": ["20"], "cursor": ["steps-next"]}, None),
            ("GET", ATTEMPTS, {"limit": ["20"]}, None),
        ])
        for window in (console, detail, attempts):
            self.assertNotIn("private", window.inner_text())

    def test_anonymous_console_has_no_task_requests(self):
        console = self.open_console(authenticated=False)
        expect(console.get_by_role("heading", name="登录后查看任务", exact=True)).to_be_visible()
        self.assertEqual(self.requests, [])

    def test_page_failure_keeps_existing_tasks_and_retries(self):
        self.reply(TASKS, page([task()], cursor=OPAQUE_CURSOR))
        self.reply(TASKS, {"error": {"code": "PERMISSION_DENIED", "message": "private server body"}},
                   cursor=OPAQUE_CURSOR, status=403)
        console = self.open_console()
        more = console.get_by_role("button", name="加载下一页任务")
        more.click()
        expect(console.get_by_role("heading", name="无权限", exact=True)).to_be_visible()
        expect(console.get_by_role("heading", name=TASK, exact=True)).to_be_visible()
        self.assertNotIn("private server body", console.inner_text())
        self.reply(TASKS, page(), cursor=OPAQUE_CURSOR)
        more.press("Enter")
        expect(console.get_by_text("已加载全部可见项目。", exact=True)).to_be_focused()
        expect(console.get_by_role("heading", name="无权限", exact=True)).to_have_count(0)
        expect(console.get_by_role("heading", name=TASK, exact=True)).to_have_count(1)

    def test_detail_refresh_ignores_a_stale_error_body(self):
        self.reply(TASKS, page([task()]))
        self.reply(DETAIL, {"error": {"code": "NOT_READY", "message": "private stale error"}},
                   status=503, request_id=OLD_REQUEST_ID)
        self.reply(STEPS, page())
        console = self.open_console()
        self.browser.evaluate("""id => {
          const original = Response.prototype.arrayBuffer;
          Response.prototype.arrayBuffer = function() {
            const decoded = original.call(this);
            if (this.headers.get('x-request-id') !== id) return decoded;
            return decoded.then(bytes => new Promise(resolve => {
              globalThis.releaseOldTask = () => resolve(bytes);
            }));
          };
        }""", OLD_REQUEST_ID)
        detail = self.open_detail(console)
        self.browser.wait_for_function("typeof globalThis.releaseOldTask === 'function'")

        self.reply(DETAIL, task("succeeded"))
        detail.get_by_role("button", name="刷新任务详情", exact=True).click()
        expect(detail.get_by_text("succeeded", exact=True)).to_be_visible()
        self.browser.evaluate("""() => new Promise(resolve => {
          globalThis.releaseOldTask();
          requestAnimationFrame(() => requestAnimationFrame(resolve));
        })""")
        expect(detail.get_by_text("succeeded", exact=True)).to_be_visible()
        expect(self.browser.locator("#connection")).to_have_text("已连接核心")
        expect(detail.get_by_role("heading", name="当前不可用", exact=True)).to_have_count(0)
        self.assertEqual([path for _, path, _, _ in self.requests], [TASKS, DETAIL, DETAIL, STEPS])


if __name__ == "__main__":
    unittest.main()
