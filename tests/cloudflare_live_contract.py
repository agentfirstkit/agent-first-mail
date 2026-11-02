"""Local orchestration checks: never authenticate, provision, or send mail."""
import json
import sys
from pathlib import Path
import runpy
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/test-cloudflare-live.py"
LIVE_TEST = runpy.run_path(str(SCRIPT))["LiveTest"]


class SendingStages(unittest.TestCase):
    def fixture(self, directory, transport):
        test = object.__new__(LIVE_TEST)
        test.root = Path(directory)
        test.secrets_path = test.root / "secrets.json"
        test.state = {"run": "synthetic", "recipients": ["recipient@example.com"]}
        test.args = SimpleNamespace(confirm_plan_id="reviewed")
        test.save = lambda: None
        test.objects = lambda: []
        configs, calls = [], []
        test.config = lambda key, value: configs.append((key, value))
        operation = "smtp_send" if transport == "smtp" else "cloudflare_send"

        def af(*args):
            calls.append(args)
            if args == ("push",):
                return {"plan_id": "reviewed", "items": [{"actions": [operation]}]}
            if args[:2] == ("case", "create"):
                return {"case_uid": "case_test", "case_path": "cases/tests/test"}
            if args[:3] == ("case", "draft", "new"):
                return {"draft_name": "test.md"}
            return {}

        test.af = af
        return test, configs, calls

    @patch.dict(LIVE_TEST.sending_preview.__globals__, {"event": lambda *args, **kwargs: None})
    def test_preview_selects_one_transport_without_sending(self):
        for transport in ["smtp", "api"]:
            with self.subTest(transport=transport), TemporaryDirectory() as directory:
                test, configs, calls = self.fixture(directory, transport)
                test.sending_preview(transport)
                operation = "smtp_send" if transport == "smtp" else "cloudflare_send"
                self.assertIn(("actions.draft.send.steps", [{operation: {}}]), configs)
                self.assertNotIn(("push", "--confirm", "reviewed"), calls)
                if transport == "api":
                    self.assertIn(("smtp", {}), configs)
                    self.assertIn(("cloudflare.sending.api_token_secret", f"file:{test.secrets_path}#smtp_token_secret"), configs)
                preview = json.loads((test.root / f"{transport}-preview.json").read_text())
                self.assertEqual(preview["plan_id"], test.state[f"{transport}_plan_id"])

    def test_uncertain_submission_is_never_attempted_twice(self):
        with TemporaryDirectory() as directory:
            test, _, calls = self.fixture(directory, "api")
            test.state["api_plan_id"] = "reviewed"
            original_af = test.af

            def uncertain(*args):
                if args[:2] == ("push", "--confirm"):
                    calls.append(args)
                    self.assertTrue(test.state["api_send_attempted"])
                    raise RuntimeError("response lost")
                return original_af(*args)

            test.af = uncertain
            with self.assertRaisesRegex(RuntimeError, "response lost"):
                test.sending_send("api")
            with self.assertRaisesRegex(RuntimeError, "already attempted"):
                test.sending_send("api")
            self.assertEqual(calls.count(("push", "--confirm", "reviewed")), 1)


class QuotaCleanup(unittest.TestCase):
    def test_retirement_removes_bindings_and_deploys_a_class_tombstone(self):
        with TemporaryDirectory() as directory:
            test = object.__new__(LIVE_TEST)
            test.worker = Path(directory)
            (test.worker / "src").mkdir()
            test.state = {"account_id": "0" * 32, "worker": "synthetic"}
            calls = []
            test.command = lambda argv, **kwargs: calls.append((argv, kwargs))
            test.retire_spool()
            config = (test.worker / "cloudflare.config.ts").read_text()
            self.assertIn('state: "deleted"', config)
            self.assertNotIn("MAIL_QUOTA", config)
            self.assertNotIn("triggers", config)
            self.assertNotIn("SpoolQuota", (test.worker / "src/retire.ts").read_text())
            self.assertEqual(calls[0][0], ["cf", "deploy"])


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--retirement-dir":
        # The fixed Worker gate compiles this cleanup deployment without cloud access.
        test = object.__new__(LIVE_TEST)
        test.worker = Path(sys.argv[2])
        test.state = {"account_id": "0" * 32, "worker": "afmail-inbound"}
        test.command = lambda *args, **kwargs: None
        test.retire_spool()
    else:
        unittest.main()
