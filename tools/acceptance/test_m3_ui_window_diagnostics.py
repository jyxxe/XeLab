"""Bounded diagnostics preserve lifecycle facts without recording plugin data."""
import importlib.util
import json
import pathlib
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "m3_ui_window_driver", pathlib.Path(__file__).resolve().parents[1] / "m3-ui-browser.py")
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)
CORE = "https://core.test"
DEPLOYMENT = "00000000-0000-4000-8000-000000000002"
SECRET = "must-never-appear-in-report"


class Request:
    def __init__(self, role="echo_widget", origin=CORE, method="GET", suffix=""):
        self.url = f"{origin}/api/platform/v1/ui/contributions/{DEPLOYMENT}/{role}{suffix}"
        self.method, self.failure = method, SECRET

    def all_headers(self):
        raise AssertionError("diagnostics must not inspect headers")


class WindowOpenDiagnostics(unittest.TestCase):
    def setUp(self):
        self.report = {}
        self.audit = DRIVER.WindowOpenAudit(CORE, DEPLOYMENT, self.report)

    def test_records_only_selected_lifecycle_and_never_reads_body(self):
        ignored = Request()
        self.audit.observe("request", ignored)
        with patch.object(DRIVER.time, "monotonic", side_effect=[10, 10.2, 13.2, 13.3, 13.4, 16.4, 16.5]):
            self.audit.start()
            source, target = Request(), Request("echo_window")
            self.audit.observe("request", source)
            self.audit.observe("response", SimpleNamespace(request=source, status=200))
            self.audit.observe("finished", source)
            self.audit.observe("request", target)
            self.audit.observe("failed", target)
            self.audit.end()
        records = self.report["window_open_observation"]["metadata_requests"]
        self.assertEqual([r["role"] for r in records], ["source", "target"])
        self.assertEqual(records[0]["status"], 200)
        self.assertLess(records[0]["start_ms"], records[0]["response_ms"])
        self.assertLess(records[0]["response_ms"], records[0]["finished_ms"])
        self.assertEqual(records[1]["failure_category"], "OTHER")
        frozen = json.dumps(self.report)
        self.audit.observe("finished", target)
        self.assertEqual(json.dumps(self.report), frozen)
        for private in (SECRET, CORE, DEPLOYMENT, "echo_widget", "echo_window"):
            self.assertNotIn(private, frozen)

    def test_selection_and_storage_are_bounded(self):
        self.audit.start()
        for request in (Request(origin="https://other.test"), Request(method="POST"),
                        Request("unknown"), Request(suffix="?token=" + SECRET)):
            self.audit.observe("request", request)
        for _ in range(100):
            self.audit.observe("request", Request())
        observation = self.report["window_open_observation"]
        self.assertEqual(len(observation["metadata_requests"]), 32)
        self.assertEqual(observation["dropped_requests"], 68)
        self.assertEqual(len(self.audit.requests), 32)
        self.assertNotIn(SECRET, json.dumps(self.report))

    def test_snapshot_filters_untrusted_values_and_failure_text(self):
        self.audit.start()

        class Widget:
            def is_detached(self):
                return False

            def locator(self, selector):
                return self

            def evaluate(self, script, timeout):
                return {"state_class": SECRET, "open_disabled": SECRET,
                        "call_disabled": True, "state_visible": False, "raw_text": SECRET}

        self.audit.snapshot(Widget())
        snapshot = self.report["window_open_observation"]["widget_snapshot"]
        self.assertEqual(snapshot["state_class"], "other")
        self.assertIsNone(snapshot["open_disabled"])
        self.assertIs(snapshot["call_disabled"], True)
        self.assertNotIn(SECRET, json.dumps(self.report))

        class Detached:
            def is_detached(self):
                raise RuntimeError(SECRET)

        self.audit.snapshot(Detached())
        self.assertFalse(self.report["window_open_observation"]["widget_snapshot"]["dom_available"])
        self.assertNotIn(SECRET, json.dumps(self.report))

    def test_diagnostic_callback_failure_cannot_fail_acceptance(self):
        self.audit.start()

        class Broken:
            @property
            def url(self):
                raise RuntimeError(SECRET)

        self.audit.observe("request", Broken())
        self.assertEqual(self.report["window_open_observation"]["diagnostic_error_bucket"], "Other")
        self.assertNotIn(SECRET, json.dumps(self.report))


if __name__ == "__main__":
    unittest.main()
