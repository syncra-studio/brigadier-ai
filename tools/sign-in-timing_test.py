"""Credential cleanup regression tests; never access a real Keychain."""

import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "sign_in_timing", Path(__file__).with_name("sign-in-timing.py")
)
timing = importlib.util.module_from_spec(spec)
spec.loader.exec_module(timing)


class CleanupTests(unittest.TestCase):
    home = "/tmp/sign-in-timing-claude-test"
    # SHA-256 of the exact config path; Claude Code uses its first eight hex digits.
    service = "Claude Code-credentials-1e677167"

    def setUp(self):
        self.platform = patch.object(timing.sys, "platform", "darwin")
        self.platform.start()
        self.addCleanup(self.platform.stop)
        self.env = {"CLAUDE_CONFIG_DIR": self.home, "USER": "timing-test"}

    def test_preserves_default_and_concurrent_credentials(self):
        items = {
            ("Claude Code-credentials", "timing-test"),
            ("Claude Code-credentials-other-run", "timing-test"),
            (self.service, "another-account"),
            (self.service, "timing-test"),
        }
        before = items.copy()

        def security(command, **kwargs):
            self.assertEqual(command[:2], ["security", "delete-generic-password"])
            self.assertEqual(len(command), 6)
            items.discard((command[3], command[5]))
            return type("Result", (), {"returncode": 0})()

        with patch.object(timing.subprocess, "run", side_effect=security) as run:
            timing.cleanup_keychain("claude", self.home, self.env)
        run.assert_called_once()
        self.assertEqual(items, before - {(self.service, "timing-test")})

    def test_codex_and_non_macos_never_access_keychain(self):
        with patch.object(timing.subprocess, "run") as run:
            timing.cleanup_keychain("codex", self.home, self.env)
            with patch.object(timing.sys, "platform", "linux"):
                timing.cleanup_keychain("claude", self.home, self.env)
        run.assert_not_called()

    def test_uncertain_storage_never_accesses_keychain(self):
        for override in ("", "/tmp/another-config"):
            with self.subTest(override=override):
                env = dict(self.env, CLAUDE_SECURESTORAGE_CONFIG_DIR=override)
                with patch.object(timing.subprocess, "run") as run:
                    with contextlib.redirect_stdout(io.StringIO()) as output:
                        timing.cleanup_keychain("claude", self.home, env)
                run.assert_not_called()
                self.assertIn(self.service, output.getvalue())

    def test_missing_scratch_config_never_accesses_keychain(self):
        with patch.object(timing.subprocess, "run") as run:
            with contextlib.redirect_stdout(io.StringIO()):
                timing.cleanup_keychain("claude", self.home, {"USER": "timing-test"})
        run.assert_not_called()

    def test_normalizes_unicode_without_resolving_path(self):
        home = "/tmp/cafe\u0301"
        with patch.object(timing.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            timing.cleanup_keychain("claude", home, dict(self.env, CLAUDE_CONFIG_DIR=home))
            decomposed = run.call_args.args[0]
            timing.cleanup_keychain(
                "claude", "/tmp/caf\u00e9", dict(self.env, CLAUDE_CONFIG_DIR="/tmp/caf\u00e9")
            )
            self.assertEqual(decomposed, run.call_args.args[0])

    def test_custom_oauth_and_invalid_account(self):
        env = dict(self.env, CLAUDE_CODE_CUSTOM_OAUTH_URL="https://example.test", USER="bad user")
        with patch.object(timing.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            timing.cleanup_keychain("claude", self.home, env)
        command = run.call_args.args[0]
        self.assertEqual(command[3], self.service.replace("-credentials", "-custom-oauth-credentials"))
        self.assertEqual(command[5], "claude-code-user")

    def test_absent_item_is_successful_cleanup(self):
        with patch.object(timing.subprocess, "run", return_value=subprocess.CompletedProcess([], 44)):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                timing.cleanup_keychain("claude", self.home, self.env)
        self.assertEqual(output.getvalue(), "")

    def test_failure_reports_only_exact_item_for_manual_cleanup(self):
        with patch.object(timing.subprocess, "run", side_effect=OSError):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                timing.cleanup_keychain("claude", self.home, self.env)
        self.assertIn(self.service, output.getvalue())
        self.assertIn("timing-test", output.getvalue())


if __name__ == "__main__":
    unittest.main()
