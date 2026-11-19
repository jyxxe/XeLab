#!/usr/bin/env python3
"""Real Chromium checks for the desktop shell visual contract.

These tests lock the presentation rules introduced by the shell visual slices
(tokens, blur scope, transparency/motion preferences, icon set, window control
hit targets and narrow-viewport sizing) without asserting pixel values, so the
visual layer can evolve as long as the contract holds. They run the actual
desktop shell with the explicit core response fixtures from test_browser, not
Core/OCI acceptance.
"""
import json
import unittest

from playwright.sync_api import expect

import test_browser as desktop
from test_bridge import contribution


class ShellVisual(unittest.TestCase):
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

    def tearDown(self):
        self.fixture.tearDown()

    def open(self, mode="light"):
        self.fixture.saved["spec"]["theme"]["mode"] = mode
        self.fixture.open()

    def style(self, selector, pseudo=None):
        return self.page.evaluate(
            "([selector, pseudo]) => {const el = document.querySelector(selector);"
            " return getComputedStyle(el, pseudo || null);}",
            [selector, pseudo],
        )

    def test_shell_tokens_derive_from_theme_in_light_and_dark(self):
        for mode in ("light", "dark"):
            self.open(mode)
            variables = self.page.evaluate(
                "() => {const style = getComputedStyle(document.documentElement);"
                " return ['--shell-radius', '--shell-border', '--ease-shell', '--menubar-h']"
                "  .map(name => [name, style.getPropertyValue(name).trim()]);}"
            )
            tokens = dict(variables)
            self.assertEqual(tokens["--shell-radius"], "14px", mode)
            self.assertEqual(tokens["--menubar-h"], "46px", mode)
            self.assertIn("cubic-bezier", tokens["--ease-shell"], mode)
            self.assertIn("color-mix", tokens["--shell-border"], mode)
            menubar = self.style(".menubar")
            self.assertNotEqual(menubar["backdropFilter"], "none", mode)
            dock = self.style("#dock")
            self.assertNotEqual(dock["backdropFilter"], "none", mode)
            self.assertIn("radial-gradient", self.style("#desktop")["backgroundImage"], mode)
            self.assertEqual(self.page.evaluate(
                "() => getComputedStyle(document.querySelector('#desktop')).height"),
                "854px", "desktop fills the viewport below the menu bar in " + mode,
            )

    def test_shell_stylesheets_are_linked_in_order(self):
        self.open()
        sheets = self.page.evaluate(
            "() => [...document.styleSheets].map(sheet => (sheet.href || '').split('/').pop())")
        self.assertEqual(sheets[:4], ["styles.css", "desktop.css", "components.css", "motion.css"])
        for name in ("desktop.css", "components.css", "motion.css"):
            status = self.page.evaluate("""async (name) => {
              const response = await fetch('/ui/org.xelab.webui/' + name);
              return {status: response.status, type: response.headers.get('content-type') || ''};
            }""", name)
            self.assertEqual(status["status"], 200, name)
            self.assertIn("css", status["type"], name)
        # The split must not change the cascade: shell and component rules still resolve.
        self.assertEqual(self.style("#dock")["borderRadius"], "20px")
        self.fixture.app("系统状态")
        self.assertEqual(self.style(".window")["borderRadius"], "14px")

    def test_index_referenced_assets_all_resolve(self):
        self.open()
        references = self.page.evaluate(
            "() => [...document.querySelectorAll('link[rel=stylesheet], script[src]')]"
            "  .map(element => element.getAttribute('href') || element.getAttribute('src'))")
        self.assertGreaterEqual(len(references), 5)
        missing = self.page.evaluate("""async (references) => {
          const failed = [];
          for (const reference of references) {
            const response = await fetch(reference);
            if (!response.ok) failed.push(reference + ':' + response.status);
          }
          return failed;
        }""", references)
        self.assertEqual(missing, [], "every stylesheet and script referenced by index.html must load")

    def test_minimizing_a_window_hands_focus_to_its_dock_button(self):
        self.open()
        window = self.fixture.app("系统状态")
        window.get_by_role("button", name="最小化系统状态").click()
        self.page.wait_for_function(
            "() => [...document.querySelectorAll('.window')].every(element => element.hidden)")
        self.assertEqual(
            self.page.evaluate("() => document.activeElement.getAttribute('data-app-id')"),
            "resources",
            "minimizing the focused window should focus its Dock entry")
        self.page.keyboard.press("Enter")
        window.wait_for(state="visible")
        self.assertIn("window-title", self.page.evaluate("document.activeElement.className"))

    def test_minimizing_focused_window_hands_focus_to_remaining_window(self):
        self.open()
        self.fixture.app("系统状态")
        self.fixture.app("设置")
        resources = self.page.locator('.window[data-app="resources"]')
        settings = self.page.locator('.window[data-app="settings"]')
        settings.get_by_role("button", name="最小化设置", exact=True).click()
        settings.wait_for(state="hidden")
        self.assertEqual(settings.get_attribute("data-focused"), "false")
        self.assertEqual(resources.get_attribute("data-focused"), "true")
        self.assertTrue(resources.is_visible())
        self.assertEqual(self.page.locator("#active-app").inner_text(), "系统状态")
        self.assertEqual(
            self.page.evaluate(
                "() => document.activeElement?.closest('.window')?.getAttribute('data-app')"),
            "resources",
        )
        self.assertIn("window-title", self.page.evaluate("document.activeElement.className"))

        self.page.set_viewport_size({"width": 390, "height": 760})
        self.page.locator(".dock-restore", has_text="设置").click()
        settings.wait_for(state="visible")
        self.assertTrue(resources.is_hidden())
        settings.get_by_role("button", name="最小化设置", exact=True).click()
        resources.wait_for(state="visible")
        self.assertEqual(resources.get_attribute("data-focused"), "true")
        self.assertEqual(self.page.locator("#active-app").inner_text(), "系统状态")
        self.assertEqual(
            self.page.evaluate(
                "() => document.activeElement?.closest('.window')?.getAttribute('data-app')"),
            "resources",
        )
        self.assertEqual(self.page.locator(".dock-restore").count(), 1)

    def test_minimizing_last_non_dock_window_focuses_its_dock_restore_button(self):
        self.open()
        tasks = self.open_builtin("任务与知识库", launcher=True)
        tasks_id = tasks.get_attribute("data-view-id")
        tasks.get_by_role("button", name="最小化任务与知识库", exact=True).click()
        tasks.wait_for(state="hidden")
        self.assertEqual(self.page.locator("#active-app").inner_text(), "桌面")
        self.assertEqual(
            self.page.evaluate("() => document.activeElement?.getAttribute('data-restore-id')"),
            tasks_id,
        )
        self.page.keyboard.press("Enter")
        tasks.wait_for(state="visible")
        self.assertEqual(self.page.locator("#active-app").inner_text(), "任务与知识库")
        self.assertEqual(
            self.page.evaluate(
                "() => document.activeElement?.closest('.window')?.getAttribute('data-app')"),
            "tasks",
        )
        self.assertIn("window-title", self.page.evaluate("document.activeElement.className"))

    def test_restoring_saved_minimized_window_keeps_foreground_window_focused(self):
        key = f"xelab:windows:{desktop.CORE}:browser-component:{desktop.ACCOUNT}"
        saved = [
            {"app": "resources", "rect": {"x": 40, "y": 24, "width": 520, "height": 380},
             "minimized": False, "maximized": False},
            {"app": "settings", "rect": {"x": 80, "y": 60, "width": 520, "height": 380},
             "minimized": True, "maximized": False},
        ]
        self.fixture.context.add_init_script(
            f"if(location.origin === {json.dumps(desktop.SHELL)}) "
            f"localStorage.setItem({json.dumps(key)}, {json.dumps(json.dumps(saved))});"
        )
        self.open()
        self.page.wait_for_function("() => document.querySelectorAll('.window').length === 2")
        resources = self.page.locator('.window[data-app="resources"]')
        settings = self.page.locator('.window[data-app="settings"]')
        self.assertTrue(resources.is_visible())
        self.assertTrue(settings.is_hidden())
        self.assertEqual(resources.get_attribute("data-focused"), "true")
        self.assertEqual(settings.get_attribute("data-focused"), "false")
        self.assertEqual(self.page.locator("#active-app").inner_text(), "系统状态")
        self.assertEqual(
            self.page.evaluate(
                "() => document.activeElement?.closest('.window')?.getAttribute('data-app')"),
            "resources",
        )
        self.assertTrue(self.page.get_by_role("button", name="恢复设置", exact=True).is_visible())

    def test_narrow_viewport_round_trip_restores_desktop_window_bounds(self):
        self.open()
        window = self.fixture.app("系统状态")
        bounds = "el => [el.offsetLeft, el.offsetTop, el.offsetWidth, el.offsetHeight]"
        original = window.evaluate(bounds)

        self.page.set_viewport_size({"width": 390, "height": 540})
        self.page.evaluate("() => new Promise(resolve => requestAnimationFrame(resolve))")
        self.assertEqual(window.evaluate(bounds), [6, 6, 378, 400])

        self.page.set_viewport_size({"width": 1280, "height": 900})
        self.page.evaluate("() => new Promise(resolve => requestAnimationFrame(resolve))")
        self.assertEqual(window.evaluate(bounds), original)

    def test_narrow_viewport_reload_preserves_saved_desktop_window_rect(self):
        self.open()
        window = self.fixture.app("系统状态")
        key = f"xelab:windows:{desktop.CORE}:browser-component:{desktop.ACCOUNT}"
        read_rect = "key => JSON.parse(localStorage.getItem(key))[0].rect"
        original = self.page.evaluate(read_rect, key)

        self.page.set_viewport_size({"width": 390, "height": 540})
        self.page.reload()
        window.wait_for()
        self.assertEqual(self.page.evaluate(read_rect, key), original)

    def test_reload_restores_the_last_active_window(self):
        self.page.set_viewport_size({"width": 390, "height": 760})
        self.open()
        self.fixture.app("系统状态")
        self.fixture.app("设置")
        resources = self.page.locator('.window[data-app="resources"]')
        settings = self.page.locator('.window[data-app="settings"]')
        self.assertTrue(resources.is_hidden())

        self.page.locator("#launcher-toggle").click()
        self.page.get_by_role("button", name="聚焦窗口1 系统状态", exact=True).click()
        self.assertTrue(resources.is_visible())
        self.assertTrue(settings.is_hidden())

        for width, height, narrow in ((390, 760, True), (1280, 900, False)):
            with self.subTest(narrow=narrow):
                self.page.set_viewport_size({"width": width, "height": height})
                self.page.reload()
                self.page.wait_for_function("document.querySelectorAll('#windows > .window').length === 2")
                self.assertEqual(resources.get_attribute("data-focused"), "true")
                self.assertEqual(settings.get_attribute("data-focused"), "false")
                self.assertTrue(resources.is_visible())
                self.assertEqual(settings.is_hidden(), narrow)
                self.assertEqual(
                    self.page.evaluate("document.activeElement?.closest('.window')?.dataset.app"),
                    "resources",
                )

    def test_left_dock_growth_keeps_close_button_reachable(self):
        value = contribution()
        self.fixture.contributions[value["spec"]["id"]] = value
        self.fixture.saved["spec"]["dock"]["position"] = "left"
        self.open()
        dock = self.page.locator('#dock[data-position="left"]')
        dock.wait_for()
        window = self.fixture.app("系统状态")
        for _ in range(11):
            window.locator(".window-title").press("Alt+ArrowDown")
        initial_width = dock.bounding_box()["width"]

        self.page.get_by_role("button", name="打开应用启动器", exact=True).click()
        self.page.get_by_role("button", name="固定 " + value["spec"]["title"], exact=True).click()
        self.page.get_by_role("button", name="取消固定 " + value["spec"]["title"], exact=True).last.wait_for()
        self.page.locator("#launcher-close").click()

        dock_bounds = dock.bounding_box()
        self.assertGreater(dock_bounds["width"], initial_width)
        close = window.get_by_role("button", name="关闭系统状态", exact=True)
        close.click(trial=True, timeout=1200)
        self.assertGreaterEqual(window.bounding_box()["x"], dock_bounds["x"] + dock_bounds["width"])
        close.click()
        window.wait_for(state="detached")

    def test_right_dock_leaves_resize_reachable_at_displayed_width(self):
        self.page.set_viewport_size({"width": 730, "height": 900})
        self.fixture.saved["spec"]["dock"]["position"] = "right"
        self.open()
        dock = self.page.locator('#dock[data-position="right"]')
        dock.wait_for()
        window = self.fixture.app("系统状态")
        resize = window.get_by_role("button", name="调整系统状态大小", exact=True)
        resize.click(trial=True, timeout=1200)
        before = window.bounding_box()
        self.assertLessEqual(before["x"] + before["width"], dock.bounding_box()["x"])
        key = f"xelab:windows:{desktop.CORE}:browser-component:{desktop.ACCOUNT}"
        read_rect = "key => JSON.parse(localStorage.getItem(key)).find(entry => entry.app === 'resources').rect"
        saved = self.page.evaluate(read_rect, key)
        self.assertGreater(saved["x"], before["x"])

        handle = resize.bounding_box()
        x, y = handle["x"] + handle["width"] / 2, handle["y"] + handle["height"] / 2
        self.page.mouse.move(x, y)
        self.page.mouse.down()
        self.page.mouse.move(x - 40, y, steps=4)
        self.assertEqual(window.bounding_box()["x"], before["x"])
        self.assertEqual(window.bounding_box()["width"], before["width"] - 40)
        self.page.mouse.move(x, y)
        self.page.mouse.up()
        self.assertEqual(window.bounding_box(), before)
        self.assertEqual(self.page.evaluate(read_rect, key), saved)

        resize.press("ArrowLeft")
        self.assertEqual(window.bounding_box()["x"], before["x"])
        self.assertEqual(window.bounding_box()["width"], before["width"] - 20)

    def test_side_dock_round_trip_preserves_saved_desktop_geometry(self):
        self.page.set_viewport_size({"width": 730, "height": 900})
        self.fixture.saved["spec"]["theme"]["reduce_motion"] = True
        self.open()
        self.page.locator("html.reduced-motion").wait_for()
        self.fixture.app("系统状态")
        resources = self.page.locator('.window[data-app="resources"]')
        original_bounds = resources.bounding_box()
        settings = self.fixture.app("设置")
        dock_position = settings.get_by_label("Dock 位置", exact=True)
        key = f"xelab:windows:{desktop.CORE}:browser-component:{desktop.ACCOUNT}"
        read_rect = "key => JSON.parse(localStorage.getItem(key)).find(entry => entry.app === 'resources').rect"
        original_rect = self.page.evaluate(read_rect, key)

        for position in ("left", "right"):
            with self.subTest(position=position):
                dock_position.select_option(position)
                title = resources.locator(".window-title")
                title.focus()
                self.assertLess(resources.bounding_box()["width"], original_bounds["width"])
                title.press("Alt+ArrowLeft" if position == "left" else "Alt+ArrowRight")
                self.assertEqual(self.page.evaluate(read_rect, key), original_rect)

        dock_position.select_option("bottom")
        self.assertEqual(resources.bounding_box(), original_bounds)
        self.assertEqual(self.page.evaluate(read_rect, key), original_rect)

    def test_closing_stacked_window_focuses_topmost_remaining_z_order(self):
        self.open()
        self.fixture.app("系统状态")
        self.fixture.app("设置")
        self.fixture.app("插件中心")
        resources = self.page.locator('.window[data-app="resources"]')
        settings = self.page.locator('.window[data-app="settings"]')
        # Click each exposed top titlebar strip to raise settings above plugins,
        # then resources above settings, so z-order (settings > plugins) differs
        # from insertion order.
        settings.locator(".window-title").click(position={"x": 160, "y": 8})
        resources.locator(".window-title").click(position={"x": 160, "y": 8})
        self.assertEqual(self.page.locator("#active-app").inner_text(), "系统状态")
        resources.get_by_role("button", name="关闭系统状态", exact=True).click()
        self.page.wait_for_function("() => document.querySelectorAll('.window').length === 2")
        self.assertEqual(settings.get_attribute("data-focused"), "true")
        self.assertEqual(self.page.locator("#active-app").inner_text(), "设置")
        self.assertEqual(
            self.page.evaluate(
                "() => document.activeElement?.closest('.window')?.getAttribute('data-app')"),
            "settings",
        )
        self.assertIn("window-title", self.page.evaluate("document.activeElement.className"))

    def test_window_stack_keeps_its_order_when_z_indices_are_compacted(self):
        self.open()
        self.fixture.app("系统状态")
        self.fixture.app("设置")
        self.fixture.app("插件中心")
        resources = self.page.locator('.window[data-app="resources"]')
        settings_title = self.page.locator('.window[data-app="settings"] .window-title')
        settings_title.focus()
        expect(self.page.locator("#active-app")).to_have_text("设置")
        resources.locator(".window-title").focus()

        compacted = resources.evaluate("""element => {
            let previous = Number(element.style.zIndex);
            for (let count = 0; count < 1100; count++) {
                element.dispatchEvent(new PointerEvent('pointerdown'));
                const current = Number(element.style.zIndex);
                if (current < previous) return true;
                previous = current;
            }
            return false;
        }""")
        self.assertTrue(compacted, "repeatedly focusing the active window should compact its z-index")
        resources.get_by_role("button", name="关闭系统状态", exact=True).press("Enter")
        expect(self.page.locator("#active-app")).to_have_text("设置", timeout=2000)
        expect(settings_title).to_be_focused()

    def test_closing_a_window_hands_focus_back_to_the_desktop(self):
        self.open()
        self.fixture.app("系统状态")
        settings = self.fixture.app("设置")
        settings.get_by_role("button", name="关闭设置").click()
        self.page.wait_for_function("() => document.querySelectorAll('.window').length === 1")
        active = self.page.evaluate("() => {const el = document.activeElement;"
                                    " return el.classList.contains('window-title') ? el.closest('.window').dataset.app : el.tagName;}")
        self.assertEqual(active, "resources", "focus should move into the remaining window")

        self.page.locator('.window[data-app="resources"]').get_by_role("button", name="关闭系统状态").click()
        self.page.wait_for_function("() => document.querySelectorAll('.window').length === 0")
        self.assertEqual(
            self.page.evaluate("() => document.activeElement.getAttribute('data-app-id')"),
            "resources",
            "closing the last window should return focus to its Dock button")

    def test_launcher_manages_keyboard_focus(self):
        self.open()
        trigger = self.page.get_by_role("button", name="打开应用启动器", exact=True)
        trigger.click()
        self.page.locator("#launcher").wait_for(state="visible")
        self.assertEqual(self.page.evaluate("document.activeElement.id"), "app-search")
        self.page.keyboard.press("Escape")
        self.page.wait_for_function("() => !document.querySelector('#launcher').open")
        self.assertEqual(
            self.page.evaluate("document.activeElement.getAttribute('aria-label')"),
            "打开应用启动器")
        trigger.press("Enter")
        self.page.locator("#launcher").wait_for(state="visible")
        self.assertEqual(self.page.evaluate("document.activeElement.id"), "app-search")
        self.page.get_by_role("button", name="启动设置", exact=True).click()
        self.page.locator('.window[data-app="settings"]').wait_for()
        self.assertIn("window-title", self.page.evaluate("document.activeElement.className"))

        # Restoring a minimized window from the launcher list must hand focus
        # into the window, the same contract as the Dock restore paths.
        window = self.fixture.app("系统状态")
        window.get_by_role("button", name="最小化系统状态").click()
        window.wait_for(state="hidden")
        trigger.click()
        self.page.locator("#launcher").wait_for(state="visible")
        self.page.get_by_role("button", name="聚焦窗口2 系统状态", exact=True).click()
        window.wait_for(state="visible")
        self.assertIn("window-title", self.page.evaluate("document.activeElement.className"))

    def test_launcher_escape_returns_focus_to_the_rebuilt_dock_button(self):
        self.page.set_viewport_size({"width": 390, "height": 760})
        value = contribution()
        self.fixture.contributions[value["spec"]["id"]] = value
        self.fixture.saved["spec"]["dock"]["pinned"] = [
            {"deployment_id": value["deployment_id"], "contribution_id": value["spec"]["id"]},
        ]
        self.fixture.hold_preferences = True
        self.open("dark")
        dock = self.page.locator("#dock")
        trigger = dock.get_by_role("button", name="打开应用启动器", exact=True)
        dock.get_by_role("button", name="打开设置", exact=True).focus()
        self.page.keyboard.press("Tab")
        expect(trigger).to_be_focused()
        original = trigger.element_handle()

        self.page.keyboard.press("Enter")
        expect(self.page.locator("#app-search")).to_be_focused()
        self.assertEqual(len(self.fixture.pending_preferences), 1)
        self.fixture.hold_preferences = False
        self.fixture.route(self.fixture.pending_preferences.pop())
        expect(dock.locator(".dock-pin button").first).to_be_enabled()
        self.assertFalse(original.evaluate("el => el.isConnected"))

        self.page.keyboard.press("Escape")
        self.page.locator("#launcher").wait_for(state="hidden")
        expect(trigger).to_be_focused()
        expect(trigger).to_be_in_viewport(ratio=1)
        self.assertTrue(trigger.evaluate("el => el.matches(':focus-visible')"))
        self.page.keyboard.press("Enter")
        expect(self.page.locator("#app-search")).to_be_focused()

    def test_launcher_window_switcher_focuses_open_window_title(self):
        self.open()
        self.fixture.app("系统状态")
        self.fixture.app("设置")
        self.page.get_by_role("button", name="打开应用启动器", exact=True).click()
        self.page.locator("#launcher").wait_for(state="visible")
        self.page.get_by_role("button", name="聚焦窗口1 系统状态", exact=True).click()
        self.page.wait_for_function("() => !document.querySelector('#launcher').open")
        self.page.wait_for_timeout(50)
        self.assertEqual(self.page.locator("#active-app").inner_text(), "系统状态")
        self.assertEqual(
            self.page.evaluate(
                "() => document.activeElement?.closest('.window')?.getAttribute('data-app')"),
            "resources",
        )
        self.assertIn("window-title", self.page.evaluate("document.activeElement.className"))

    def test_desktop_wallpaper_is_served_and_applied(self):
        self.open()
        background = self.style("#desktop")["backgroundImage"]
        self.assertIn("wallpaper.svg", background)
        status = self.page.evaluate("""async () => {
          const response = await fetch('/ui/org.xelab.webui/wallpaper.svg');
          return {status: response.status, type: response.headers.get('content-type') || ''};
        }""")
        self.assertEqual(status["status"], 200)
        self.assertIn("svg", status["type"])
        self.assertIn("cover", self.style("#desktop")["backgroundSize"])

    def test_custom_theme_color_scheme_follows_its_desktop_token(self):
        self.open()
        settings = self.fixture.app("设置")
        settings.get_by_label("主题模式", exact=True).select_option("custom")
        desktop = settings.get_by_label("自定义 desktop", exact=True)
        for colour, expected in (("#0B1020", "dark"), ("#F5F7FB", "light")):
            desktop.evaluate(
                "(input, colour) => { input.value = colour; input.dispatchEvent(new Event('input', {bubbles: true})); }",
                colour)
            self.page.wait_for_function(
                "(expected) => document.documentElement.style.colorScheme === expected",
                arg=expected)

    def test_reduced_transparency_removes_shell_blur(self):
        self.open()
        self.page.evaluate("() => document.documentElement.classList.add('reduced-transparency')")
        for selector in (".menubar", "#dock"):
            style = self.style(selector)
            self.assertEqual(style["backdropFilter"], "none", selector)
            self.assertNotIn("rgba(255, 255, 255, 0.8", style["backgroundColor"], selector)

    def test_reduced_motion_disables_shell_transitions(self):
        self.open()
        self.page.evaluate("() => document.documentElement.classList.add('reduced-motion')")
        self.assertEqual(self.style("#dock")["transitionDuration"], "0s")
        self.assertEqual(self.style("#dock")["animationName"], "none")

    def test_dock_and_launcher_use_icon_set_without_text_glyphs(self):
        self.open()
        self.assertGreaterEqual(self.page.locator("#dock svg.app-glyph").count(), 6)
        for icon in self.page.locator("#dock .app-icon").all():
            self.assertEqual(icon.text_content(), "")
            self.assertEqual(icon.locator("svg").count(), 1)
        self.page.get_by_role("button", name="打开应用启动器", exact=True).click()
        self.page.locator("#launcher").wait_for(state="visible")
        self.assertGreaterEqual(self.page.locator("#app-grid .app-icon svg").count(), 8)
        self.assertGreaterEqual(self.page.locator("#launcher svg.app-glyph").count(), 8)
        launcher_icon = self.style("#app-grid .app-icon")
        self.assertEqual(launcher_icon["display"], "grid")
        centered = self.page.evaluate(
            "() => {"
            " const box = document.querySelector('#app-grid .app-icon').getBoundingClientRect();"
            " const svg = document.querySelector('#app-grid .app-icon svg').getBoundingClientRect();"
            " return Math.abs((svg.left - box.left) - (box.right - svg.right)) <= 1.5"
            "  && Math.abs((svg.top - box.top) - (box.bottom - svg.bottom)) <= 1.5;"
            "}"
        )
        self.assertTrue(centered, "launcher app-icon svg must be centered inside its 42x42 tile")

    def test_builtin_glyph_is_shared_by_launch_and_restore_entries(self):
        self.open()
        dock = self.page.locator("#dock")
        launcher = self.page.get_by_role("button", name="打开应用启动器", exact=True)
        launcher_loaded = self.page.locator("#launcher").get_by_text("已加载贡献中没有匹配项", exact=True)
        initial_glyph = dock.get_by_role("button", name="打开系统状态", exact=True).locator("svg").inner_html()
        launcher.press("Enter")
        launcher_loaded.wait_for()
        shortcut = self.page.get_by_role("button", name="启动系统状态", exact=True)
        self.assertEqual(shortcut.locator("svg").inner_html(), initial_glyph)
        shortcut.press("Enter")

        window = self.page.locator('.window[data-app="resources"]')
        minimize = window.get_by_role("button", name="最小化系统状态", exact=True)
        minimize.press("Space")
        window.wait_for(state="hidden")
        restore = dock.get_by_role("button", name="恢复系统状态", exact=True)
        self.assertEqual(restore.locator("svg").inner_html(), initial_glyph)

        launcher.press("Enter")
        launcher_loaded.wait_for()
        switcher = self.page.get_by_role("button", name="聚焦窗口1 系统状态", exact=True)
        self.assertEqual(switcher.locator("svg").inner_html(), initial_glyph)
        switcher.press("Enter")
        window.wait_for()
        minimize.press("Space")
        window.wait_for(state="hidden")
        restore.press("Enter")
        window.wait_for()

    def test_window_toolbars_and_scrollbars_align_with_shell_tokens(self):
        self.open()
        window = self.fixture.app("插件中心")
        window.locator(".window-content > .toolbar").wait_for()
        header_toolbar = self.style(".window-content > .toolbar")
        self.assertEqual(header_toolbar["borderBottomStyle"], "solid")
        self.assertEqual(header_toolbar["borderBottomWidth"], "1px")
        self.assertEqual(header_toolbar["paddingBottom"], "14px")
        self.assertEqual(self.style(".window-content .toolbar h2")["fontSize"], "17px")
        self.assertEqual(self.style(".window-content .toolbar button")["minHeight"], "32px")
        card_toolbar = self.style(".window-content .card .toolbar")
        self.assertEqual(card_toolbar["borderTopStyle"], "solid")
        self.assertEqual(card_toolbar["borderTopWidth"], "1px")
        self.assertEqual(card_toolbar["paddingTop"], "10px")
        has_scrollbar_and_active = self.page.evaluate(
            "() => {"
            " const texts = [...document.styleSheets].flatMap(s => [...s.cssRules].map(r => r.cssText));"
            " return texts.some(t => t.includes('::-webkit-scrollbar-thumb')) && texts.some(t => t.includes('.primary:active'));"
            "}"
        )
        self.assertTrue(has_scrollbar_and_active)

    def assert_action_glyph(self, button):
        glyph = button.locator(":scope > svg.app-glyph")
        self.assertEqual(glyph.count(), 1)
        self.assertEqual(glyph.get_attribute("aria-hidden"), "true")
        self.assertEqual(glyph.get_attribute("focusable"), "false")
        self.assertEqual(button.text_content(), "")
        self.assertTrue(glyph.evaluate("""svg => {
            const style = getComputedStyle(svg);
            const glyph = svg.getBoundingClientRect(), button = svg.parentElement.getBoundingClientRect();
            return style.visibility === 'visible' && Number(style.opacity) > 0 && style.stroke !== 'none'
                && glyph.width > 0 && glyph.height > 0
                && glyph.left >= button.left && glyph.top >= button.top
                && glyph.right <= button.right && glyph.bottom <= button.bottom
                && Math.abs((glyph.left - button.left) - (button.right - glyph.right)) <= 1.5
                && Math.abs((glyph.top - button.top) - (button.bottom - glyph.bottom)) <= 1.5;
        }"""), "action glyph must be visible, centered and within its button")
        return glyph.inner_html()

    def test_window_control_glyphs_keep_hit_targets_across_viewports_and_themes(self):
        self.fixture.saved["spec"]["restore_windows"] = False
        custom_tokens = {
            "accent": "#80A5FF", "desktop": "#202635", "surface": "#283040", "text": "#EFF2F8",
            "muted_text": "#BAC8E0", "border": "#546784", "danger": "#FF938A", "radius_px": 12,
            "blur_px": 16, "panel_opacity": 0.92, "font": "system",
        }
        for width, height, mode in ((1280, 900, "light"), (1280, 900, "dark"),
                                    (390, 760, "light"), (390, 760, "dark"), (390, 760, "custom")):
            with self.subTest(viewport=(width, height), theme=mode):
                self.page.set_viewport_size({"width": width, "height": height})
                if mode == "custom":
                    self.fixture.saved["spec"]["theme"]["custom_tokens"] = custom_tokens
                self.open(mode)
                self.page.locator(f'html[data-theme="{mode}"]').wait_for(state="attached")
                if mode == "custom":
                    self.assertEqual(self.page.evaluate("document.documentElement.style.getPropertyValue('--desktop')"),
                                     custom_tokens["desktop"])
                window = self.fixture.app("系统状态")
                controls = [window.get_by_role("button", name=name, exact=True)
                            for name in ("关闭系统状态", "最小化系统状态", "最大化系统状态")]
                shapes = []
                for control in controls:
                    shapes.append(self.assert_action_glyph(control))
                    bounds = control.bounding_box()
                    self.assertEqual([bounds["width"], bounds["height"]], [24, 24])
                    self.assertGreaterEqual(min(bounds["x"], bounds["y"]), 0)
                    self.assertLessEqual(bounds["x"] + bounds["width"], width)
                    self.assertLessEqual(bounds["y"] + bounds["height"], height)
                self.assertEqual(len(set(shapes)), 3)

                controls[0].hover()
                self.assert_action_glyph(controls[0])
                window.locator(".window-title").focus()
                for control in controls:
                    self.page.keyboard.press("Tab")
                    self.assertTrue(control.evaluate("""el => el === document.activeElement
                        && el.matches(':focus-visible') && getComputedStyle(el).outlineStyle !== 'none'"""))
                    self.assert_action_glyph(control)
                controls[0].press("Space")
                window.wait_for(state="detached")

                launcher = self.page.locator("#launcher")
                trigger = self.page.get_by_role("button", name="打开应用启动器", exact=True)
                trigger.click()
                close = launcher.get_by_role("button", name="关闭启动器", exact=True)
                self.assertEqual(self.assert_action_glyph(close), shapes[0])
                target = close.evaluate("el => [el.offsetWidth, el.offsetHeight]")
                self.assertGreaterEqual(min(target), 36)
                close.hover()
                self.assertEqual(self.assert_action_glyph(close), shapes[0])
                self.page.keyboard.press("Shift+Tab")
                self.assertTrue(close.evaluate("""el => el === document.activeElement
                    && el.matches(':focus-visible') && getComputedStyle(el).outlineStyle !== 'none'"""))
                self.assertEqual(self.assert_action_glyph(close), shapes[0])
                self.page.keyboard.press("Space")
                launcher.wait_for(state="hidden")
                trigger.click()
                close.click()
                launcher.wait_for(state="hidden")

    def test_dock_unpin_has_distinct_glyph_and_keyboard_action(self):
        self.page.set_viewport_size({"width": 390, "height": 760})
        value = contribution()
        title = value["spec"]["title"]
        self.fixture.contributions[value["spec"]["id"]] = value
        self.fixture.saved["spec"]["dock"]["pinned"] = [
            {"deployment_id": value["deployment_id"], "contribution_id": value["spec"]["id"]},
        ]
        self.open()
        window = self.fixture.app("系统状态")
        close_glyph = self.assert_action_glyph(window.get_by_role("button", name="关闭系统状态", exact=True))
        pin = self.page.locator("#dock .dock-pin")
        pinned = pin.get_by_role("button", name=f"打开固定贡献 {title}", exact=True)
        remove = pin.get_by_role("button", name=f"取消固定 {title}", exact=True)
        remove.scroll_into_view_if_needed()
        self.assertNotEqual(self.assert_action_glyph(remove), close_glyph)
        bounds = remove.bounding_box()
        self.assertGreaterEqual(bounds["width"], 24)
        self.assertGreaterEqual(bounds["height"], 36)
        self.assertGreaterEqual(bounds["x"], 0)
        self.assertLessEqual(bounds["x"] + bounds["width"], 390)
        remove.hover()
        self.assert_action_glyph(remove)
        pinned.focus()
        self.page.keyboard.press("Tab")
        self.assertTrue(remove.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))
        self.assert_action_glyph(remove)
        self.page.keyboard.press("Enter")
        pin.wait_for(state="detached")
        self.assertEqual(self.fixture.saved["spec"]["dock"]["pinned"], [])
        launcher = self.page.locator("#dock").get_by_role("button", name="打开应用启动器", exact=True)
        self.assertTrue(launcher.evaluate("""el => el === document.activeElement
            && el.matches(':focus-visible') && getComputedStyle(el).outlineStyle !== 'none'"""))
        self.page.keyboard.press("Enter")
        self.page.locator("#launcher").wait_for()
        self.assertTrue(self.page.locator("#app-search").evaluate("el => el === document.activeElement"))

    def test_shell_motion_is_finite_and_reducible(self):
        self.open()
        self.fixture.app("系统状态")
        window = self.page.locator(".window").last
        self.assertEqual(window.evaluate("el => getComputedStyle(el).animationName"), "shell-window-in")
        self.assertEqual(window.evaluate("el => getComputedStyle(el).animationDuration"), "0.16s")
        self.assertTrue(self.page.evaluate(
            "() => [...document.querySelectorAll('*')].every(el => {"
            " const style = getComputedStyle(el);"
            " return style.animationIterationCount !== 'infinite';})"),
            "the shell must not run an endless decorative animation",
        )
        self.page.evaluate("() => document.documentElement.classList.add('reduced-motion')")
        self.assertEqual(window.evaluate("el => getComputedStyle(el).animationName"), "none")

    def test_narrow_viewport_has_no_overflow_and_scales_icons(self):
        self.open()
        self.page.set_viewport_size({"width": 390, "height": 760})
        self.assertLessEqual(self.page.evaluate("document.documentElement.scrollWidth"), 390)
        self.assertEqual(self.style("#dock svg")["width"], "16px")
        self.assertEqual(self.page.evaluate(
            "() => getComputedStyle(document.querySelector('.menubar')).height"), "44px")

    def window_rect(self, window):
        return window.evaluate("el => ['left', 'top', 'width', 'height'].map(key => parseInt(el.style[key], 10))")

    def test_maximized_window_shortcuts_preserve_restore_geometry(self):
        self.open()
        window = self.fixture.app("系统状态")
        title = window.locator(".window-title")
        resize = window.get_by_role("button", name="调整系统状态大小", exact=True)
        maximize = window.get_by_role("button", name="最大化系统状态", exact=True)
        restore = window.get_by_role("button", name="恢复系统状态窗口大小", exact=True)
        x, y, width, height = self.window_rect(window)

        title.press("Alt+ArrowRight")
        title.press("Alt+Shift+ArrowDown")
        resize.press("ArrowRight")
        expected = [x + 20, y, width + 20, height + 20]
        self.assertEqual(self.window_rect(window), expected)

        maximize_glyph = self.assert_action_glyph(maximize)
        maximize.press("Enter")
        maximized = self.window_rect(window)
        self.assertNotEqual(maximized, expected)
        self.assertNotEqual(self.assert_action_glyph(restore), maximize_glyph)
        self.assertTrue(restore.evaluate("el => el === document.activeElement"))
        expect(resize).to_have_count(0, timeout=2000)
        expect(title).to_have_attribute("aria-label", "系统状态窗口。")
        title.press("Alt+ArrowRight")
        title.press("Alt+Shift+ArrowDown")
        window.locator(".window-resize").dispatch_event("keydown", {"key": "ArrowRight"})
        self.assertEqual(self.window_rect(window), maximized)

        title.dblclick()
        self.assertEqual(self.window_rect(window), expected)
        self.assertEqual(self.assert_action_glyph(maximize), maximize_glyph)
        expect(resize).to_be_visible()
        expect(title).to_have_attribute("aria-label", "系统状态窗口。Alt加方向键移动。Alt加Shift加方向键调整大小。")
        resize.press("ArrowLeft")
        self.assertEqual(self.window_rect(window), [x + 20, y, width, height + 20])

    def test_narrow_window_shortcuts_do_not_change_desktop_geometry(self):
        self.open()
        self.page.set_viewport_size({"width": 640, "height": 900})
        window = self.fixture.app("系统状态")
        title = window.locator(".window-title")
        expect(title).to_have_attribute("aria-label", "系统状态窗口。", timeout=2000)
        expected = self.window_rect(window)
        title.press("Alt+ArrowDown")
        title.press("Alt+Shift+ArrowUp")

        self.page.set_viewport_size({"width": 1280, "height": 900})
        expect(title).to_have_attribute("aria-label", "系统状态窗口。Alt加方向键移动。Alt加Shift加方向键调整大小。")
        self.assertEqual(self.window_rect(window), expected)

    def open_builtin(self, title, launcher=False):
        if launcher:
            self.page.get_by_role("button", name="打开应用启动器", exact=True).click()
            self.page.get_by_role("button", name=f"启动{title}", exact=True).click()
        else:
            self.fixture.app(title)
        window = self.page.locator(".window").last
        window.locator("[data-window-toolbar]").wait_for()
        return window

    def test_builtin_windows_share_a_window_toolbar_and_named_actions(self):
        self.open()
        windows = [
            ("总览", "总览", False), ("插件中心", "插件中心", False),
            ("Skill", "Skill 浏览器", False), ("账户与权限", "账户与权限", False),
            ("系统状态", "资源与系统状态", False), ("设置", "桌面设置", False),
            ("UI 客户端", "UI 客户端", True), ("插件页面", "插件页面", True),
            ("任务与知识库", "任务与能力", True),
        ]
        for window_name, title, launcher in windows:
            with self.subTest(window=window_name):
                window = self.open_builtin(window_name, launcher=launcher)
                toolbar = window.locator("[data-window-toolbar]")
                self.assertEqual(toolbar.count(), 1, window_name)
                self.assertIn("window-toolbar", toolbar.get_attribute("class"), window_name)
                self.assertEqual(toolbar.locator(".window-toolbar-title").inner_text(), title)
                group = toolbar.get_by_role("group", name=f"{title}操作", exact=True)
                self.assertEqual(group.count(), 1, title)
                refresh = group.get_by_role("button", name=f"刷新{title}", exact=True)
                self.assertEqual(refresh.count(), 1, title)
                self.assertEqual(refresh.inner_text(), "刷新", title)
                self.assertEqual(window.get_by_role("button", name="刷新", exact=True).count(), 0,
                                 "refresh must keep its window-specific accessible name")
                window.get_by_role("button", name=f"关闭{window_name}", exact=True).click()

    def test_window_toolbar_stays_visible_while_content_scrolls(self):
        self.open()
        window = self.open_builtin("系统状态")
        content = window.locator(".window-content")
        content.evaluate("""el => {
          const spacer = document.createElement('div');
          spacer.style.height = '1600px';
          spacer.textContent = 'toolbar sticky probe';
          el.append(spacer);
          el.scrollTop = 600;
        }""")
        self.assertGreater(content.evaluate("el => el.scrollTop"), 0)
        toolbar = window.locator("[data-window-toolbar]")
        self.assertTrue(toolbar.is_visible())
        content_box, toolbar_box = content.bounding_box(), toolbar.bounding_box()
        self.assertLessEqual(abs(toolbar_box["y"] - content_box["y"]), 1)
        self.assertGreaterEqual(toolbar_box["height"], 48)

    def test_window_toolbar_wraps_without_inner_overflow_on_narrow_screens(self):
        self.open()
        self.page.set_viewport_size({"width": 390, "height": 760})
        window = self.open_builtin("Skill")
        content = window.locator(".window-content")
        self.assertLessEqual(
            content.evaluate("el => el.scrollWidth"), content.evaluate("el => el.clientWidth") + 1)
        title = window.locator(".window-toolbar-title")
        actions = window.locator(".window-toolbar-actions")
        title_box, actions_box = title.bounding_box(), actions.bounding_box()
        self.assertGreaterEqual(actions_box["y"], title_box["y"] + title_box["height"] - 1,
                                "action group takes its own row on a narrow window")
        self.assertEqual(title.evaluate("el => getComputedStyle(el).textOverflow"), "ellipsis")
        self.assertEqual(title.evaluate("el => getComputedStyle(el).whiteSpace"), "nowrap")
        self.assertTrue(actions.get_by_role("button", name="刷新Skill 浏览器", exact=True).is_visible())

    def test_window_toolbar_refresh_is_keyboard_reachable_and_named(self):
        self.open()
        window = self.open_builtin("总览")
        self.assertEqual(self.page.evaluate(
            "() => document.activeElement && document.activeElement.className"), "window-title")
        for _ in range(12):
            self.page.keyboard.press("Tab")
            if self.page.evaluate(
                    "() => document.activeElement && document.activeElement.getAttribute('aria-label')") == "刷新总览":
                break
        self.assertEqual(self.page.evaluate(
            "() => document.activeElement && document.activeElement.getAttribute('aria-label')"), "刷新总览")
        self.assertNotEqual(self.page.evaluate(
            "() => getComputedStyle(document.activeElement).outlineStyle"), "none")
        self.assertTrue(window.locator(".window-toolbar-actions").is_visible())

    def test_window_refresh_preserves_keyboard_focus_until_user_moves_it(self):
        self.open()
        window = self.open_builtin("系统状态")
        title = window.locator(".window-title")
        refresh = window.get_by_role("button", name="刷新资源与系统状态", exact=True)
        pending = []

        def hold_read(route):
            if route.request.method != "GET":
                route.fallback()
                return
            pending.append(route)
            self.page.evaluate("() => { globalThis.__refreshReads = (globalThis.__refreshReads ?? 0) + 1; }")

        self.page.route(desktop.CORE + "/api/platform/v1/node", hold_read)
        for count, move_focus in enumerate((False, True), 1):
            with self.subTest(move_focus=move_focus):
                refresh.focus()
                self.page.keyboard.press("Enter")
                self.page.wait_for_function("count => globalThis.__refreshReads === count", arg=count)
                focused_during_loading = title.evaluate("el => el === document.activeElement && el.matches(':focus-visible')")
                if move_focus:
                    self.page.keyboard.press("Tab")
                    left_title = title.evaluate("el => el !== document.activeElement")
                    self.page.keyboard.press("Shift+Tab")
                    returned_to_title = title.evaluate("el => el === document.activeElement")

                pending.pop().fallback()
                refresh.wait_for()
                self.assertTrue(focused_during_loading, "the window title keeps focus while the toolbar reloads")
                if move_focus:
                    self.assertTrue(left_title and returned_to_title)
                target = title if move_focus else refresh
                self.assertTrue(target.evaluate("el => el === document.activeElement && el.matches(':focus-visible')"))


if __name__ == "__main__":
    unittest.main()
