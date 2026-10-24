"""Regressions from nightly 40052764440; no model calls."""
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import battery
import nightly

PREFIX = "20261022-0805-take-over-5-sessions-w184-test-"


class SeamRunnerTests(unittest.TestCase):
    def runner(self):
        run = object.__new__(battery.Battery)
        run.args = SimpleNamespace(turn_timeout=1, provider="codex")
        run.steps, run.errors, run.result = [], [], {}
        run.conversation = "test-conversation"
        run.deadline = time.monotonic() + 30
        run.ipc = Mock()
        return run

    def view(self, requests=(), run="idle"):
        return {"requests": [{"id": key, "state": {"type": state}} for key, state in requests],
                "run": run, "messages": {"messages": []}}

    def test_queued_message_waits_for_its_new_request_before_injecting_fault(self):
        run = self.runner()
        old = self.view([("old", "done")])
        working = self.view([("old", "done"), ("new", "working")], "running")
        done = self.view([("old", "done"), ("new", "done")])
        run.ipc.call.return_value = {"outcome": {"type": "queued", "item": {"id": "queue-id"}}}
        run.view = Mock(side_effect=[old, old, working, working, done])
        def fault(step):
            self.assertEqual(step["request"], "new")
        with patch("battery.time.sleep"):
            step = run.send("ballast", "inert batch", fault=fault)
        self.assertEqual(step["queued_id"], "queue-id")
        self.assertEqual(step["request"], "new")
        self.assertTrue(step["settled"])
        self.assertEqual(run.errors, [])
        self.assertEqual(run.ipc.call.call_count, 1)  # No interrupt of a completed turn.

    def test_sent_message_does_not_finish_when_only_the_previous_request_finished(self):
        run = self.runner()
        old = self.view([("old", "done")])
        done = self.view([("old", "done"), ("new", "done")])
        run.ipc.call.return_value = {"outcome": {"type": "sent", "message": {"id": "new", "requestId": "new"}}}
        run.view = Mock(side_effect=[old, old, done])
        with patch("battery.time.sleep"):
            self.assertTrue(run.send("probe", "recall")["settled"])
        self.assertEqual(run.errors, [])

    def test_closed_codex_transcripts_are_read_from_the_exact_archive_path(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            home = Path(scratch)
            original = home / "sessions/2026/10/24/rollout-owned.jsonl"
            archived = home / "archived_sessions" / original.name
            archived.parent.mkdir()
            archived.write_text(json.dumps({"type": "compacted"}) + "\n")
            self.assertEqual(battery.native_path(original, "codex"), archived)
            self.assertEqual(battery.native_path(original, "claude"), original)
            original.parent.mkdir(parents=True)
            original.write_text("active")
            self.assertEqual(battery.native_path(original, "codex"), original)
            original.unlink()
            event = {"entry": {"provider": "codex", "event": {
                "type": "sessionStarted", "nativeId": "owned", "file": str(original)}}}
            with patch("battery.parse_native", return_value={"economics": battery.historical({}, [])}) as parse:
                battery.capture_roles([event])
            self.assertEqual(parse.call_args.args[:2], (archived, "codex"))

    def test_capture_keeps_seam_and_ledger_evidence_when_a_native_file_is_missing(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            run = self.runner()
            run.root = Path(scratch)
            run.data = run.root / "data"
            run.repo = run.root / "repo"
            run.proc = Mock()
            run.proc.poll.return_value = None
            run.native_files = []
            run.seams = []
            run.result["seams"] = []
            run.expected = {"absent_ledger": "secret"}
            run.record = Mock(return_value=run.root / "record")
            run.record().mkdir()
            (run.record() / "ledger.md").write_text("kept ledger")
            run.seam_file = Mock(return_value={"seams": []})
            run.entries = Mock(return_value=[{"entry": {"provider": "codex", "event": {
                "type": "sessionStarted", "nativeId": "missing", "file": str(run.root / "missing.jsonl")}}}])
            db = Mock()
            db.execute.return_value = []
            with patch("battery.sqlite3.connect") as connect:
                connect.return_value.__enter__.return_value = db
                run.capture()
            self.assertIsNone(run.result["compactions"])
            self.assertEqual(run.result["ledger_bytes"], 11)
            self.assertEqual(run.result["handoffs"], [])
            self.assertIn("capture: native transcripts unavailable:", run.errors[0])

    def test_consolidated_report_quotes_the_observation_failure(self):
        summary = {"run_url": "run", "source": "source", "regressions": [], "weekly": {},
                   "providers": [{"provider": "codex", "status": "incomplete", "arms": [
                       {"block": "recall", "profile": "long", "arm": "seam", "status": "incomplete",
                        "errors": ["whole-run watchdog expired"]}]}]}
        self.assertIn("recall/long seam run problems: whole-run watchdog expired", nightly.markdown(summary))

    def test_headless_daemon_idle_exit_outlives_the_measurement_and_cleanup(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            run = self.runner()
            run.root = Path(scratch)
            run.data = run.root / "data"
            run.args.daemon, run.args.tokens, run.args.timeout = "/own/debug/brigadierd", 43750, 6000
            run.processes = []
            run.ipc.call.return_value = {"catalog": {}}
            with patch("battery.subprocess.Popen") as popen:
                popen.return_value.poll.return_value = None
                run.start()
            self.assertEqual(popen.call_args.kwargs["env"]["BRIGADIER_IDLE_EXIT_SECS"], "6600")
            self.assertEqual(popen.call_args.kwargs["env"]["BRIGADIER_DATA_DIR"], str(run.data))


if __name__ == "__main__":
    unittest.main()
