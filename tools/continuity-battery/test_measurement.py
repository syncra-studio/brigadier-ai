"""#602: a cell measures only turns that ended on their own, and keeps the CLI's own error text.
Fixtures only; never a model, CLI or daemon."""
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

# Before battery: it puts tools/bench, which has its own nightly.py, first on the path.
import nightly
import battery
import plain
from plain import PlainBattery
import scoring

PREFIX = "20261104-1423-win-issues-w24-test-"
FIXTURES = battery.SRC / "crates/providers/fixtures"
LIMIT = "Claude usage limit reached. Your limit will reset at 5pm (UTC)."


def fixture(name):
    return [json.loads(line) for line in (FIXTURES / name).read_text().splitlines() if line.strip()]


class FakeDaemon:
    """Answers sendMessage with a new request that ends in `state`, as the daemon's view shows it."""

    def __init__(self, state):
        self.state, self.requests = state, []

    def call(self, body, timeout=None):
        if body["method"] == "sendMessage":
            request = f"request-{len(self.requests)}"
            self.requests.append({"id": request, "state": dict(self.state)})
            return {"outcome": {"type": "sent", "message": {"id": request, "requestId": request}}}
        return {}

    def view(self):
        return {"requests": list(self.requests), "run": "idle", "messages": {"messages": []}}


class MeasurementTests(unittest.TestCase):
    def daemon_run(self, scratch, state):
        run = object.__new__(battery.Battery)
        run.args = SimpleNamespace(turn_timeout=1, provider="claude", arm="fixture", fair=True,
                                   max_fillers=1, min_fillers=0, ballast=1)
        run.steps, run.errors, run.seams = [], [], []
        run.expected = {"tool_only": "VT-1", "absent_ledger": "ORCHID-1", "large_failure": "MIDFAIL-1"}
        run.result = {"steps": run.steps, "seams": run.seams, "errors": run.errors, "expected": run.expected}
        run.conversation = "test-conversation"
        run.deadline = time.monotonic() + 30
        run.repo = Path(scratch)
        daemon = FakeDaemon(state)
        run.ipc = daemon
        run.view = daemon.view
        run.setup = Mock()
        run.generations = Mock(return_value=0)
        with patch("battery.time.sleep"):
            run.run()
        return run

    def test_a_daemon_run_whose_requests_all_fail_is_incomplete(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            run = self.daemon_run(scratch, {"type": "failed", "error": LIMIT})
        names = [step["name"] for step in run.steps]
        self.assertIn("probe", names)
        self.assertIn("items", run.result)
        # A failed request settles, but it measured nothing.
        self.assertTrue(all(step["settled"] and step["outcome"] == "failed" for step in run.steps))
        self.assertIn(f"cli: probe: request failed: {LIMIT}", run.errors)
        self.assertEqual(scoring.exit_status(run.result), 2)
        self.assertIn("incomplete: no turn settled", run.errors)
        self.assertIn("incomplete: apply ended failed", run.errors)
        self.assertIn("incomplete: probe ended failed", run.errors)
        self.assertIn("incomplete: the CLI reported errors", run.errors)

    def test_a_daemon_run_whose_turns_end_is_complete_even_when_recall_fails(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            run = self.daemon_run(scratch, {"type": "done"})
        self.assertTrue(all(step["outcome"] == "done" for step in run.steps))
        self.assertFalse(any(row["verdict"] == "pass" for row in run.result["items"].values()))
        self.assertEqual(scoring.exit_status(run.result), 0)
        self.assertEqual(run.errors, [])

    def test_a_run_with_no_settled_turn_is_incomplete(self):
        # The pilot's hollow pass: every turn timed out, and the probe was still graded.
        steps = [{"name": name, "settled": False, "outcome": "timeout", "answer": ""}
                 for name in ("plant-user", "ballast-00", "apply", "probe")]
        result = {"steps": steps, "items": battery.grade("", {"tool_only": "a", "absent_ledger": "b",
                                                                   "large_failure": "c"}, {}),
                  "errors": [f"{s['name']}: turn did not settle within bound" for s in steps]}
        self.assertEqual(scoring.exit_status(result), 2)
        self.assertIn("incomplete: no turn settled", result["errors"])
        self.assertIn("incomplete: probe ended timeout", result["errors"])

    def test_each_block_requires_its_own_graded_turns(self):
        def result(block, failed):
            names = scoring.REQUIRED[block] + ("plant",)
            return {"block": block, "items": True, "errors": [], "steps": [
                {"name": name, "outcome": "failed" if name == failed else "done"} for name in names]}
        for block, graded in scoring.REQUIRED.items():
            self.assertEqual(scoring.exit_status(result(block, "plant")), 0, block)
            for name in graded:
                with self.subTest(block=block, step=name):
                    failed = result(block, name)
                    self.assertEqual(scoring.exit_status(failed), 2)
                    self.assertIn(f"incomplete: {name} ended failed", failed["errors"])
        missing = {"block": "updates", "items": {}, "errors": [], "steps": [{"name": "probe", "outcome": "done"}]}
        self.assertEqual(scoring.exit_status(missing), 2)
        self.assertIn("incomplete: status was never sent", missing["errors"])
        # Evidence from before outcomes were recorded falls back to `settled`.
        old = {"items": {}, "errors": [], "steps": [{"name": n, "settled": True} for n in ("apply", "probe")]}
        self.assertEqual(scoring.exit_status(old), 0)
        skipped = {"status": "skipped", "errors": [], "steps": []}
        self.assertEqual(scoring.exit_status(skipped), 0)
        self.assertEqual(skipped["errors"], [])

    def test_only_a_fault_step_may_end_failed(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            run = self.daemon_run(scratch, {"type": "done"})
            run.ipc.state = {"type": "failed", "error": "injected: context window"}
            with patch("battery.time.sleep"):
                step = run.send("seam-context", "fault", fault=lambda step: None)
                run.ipc.state = {"type": "stopped"}
                run.send("seam-crash", "fault", fault=lambda step: None)
        self.assertEqual((step["fault"], step["outcome"], step["error"]), (True, "failed", "injected: context window"))
        self.assertEqual(run.errors, [])
        self.assertEqual(scoring.exit_status(run.result), 0)
        # The same failure on a graded turn is a lost measurement.
        run.steps[-3]["outcome"] = "stopped"
        self.assertEqual(run.steps[-3]["name"], "probe")
        self.assertEqual(scoring.exit_status(run.result), 2)
        self.assertIn("incomplete: probe ended stopped", run.errors)

    def plain_run(self):
        run = object.__new__(PlainBattery)
        run.args = SimpleNamespace(provider="claude", model="fixture-model", turn_timeout=1)
        run.repo, run.deadline = Path(tempfile.gettempdir()), time.monotonic() + 10
        run.steps, run.errors, run.processes, run.raw_events = [], [], [], []
        run.result, run.native_id, run.native_sessions = {}, None, {}
        run.native_path = Mock(return_value=None)
        return run

    def plain_send(self, run, events, returncode=1, stderr=""):
        lines = [{"type": "system", "subtype": "init", "session_id": "fixture-id"}] + events
        fake = SimpleNamespace(returncode=returncode,
                               communicate=lambda *a, **k: ("\n".join(map(json.dumps, lines)), stderr))
        with patch("plain.subprocess.Popen", return_value=fake):
            return run.send("probe", "prompt")

    def test_a_plain_cli_error_result_is_kept_and_never_graded(self):
        for name, text in (("claude-result-limit.jsonl", "API Error: 429 (API status 429)"),
                           ("claude-gateway-limit.jsonl", "API Error: 503 [gateway] [429]: ")):
            with self.subTest(fixture=name):
                run = self.plain_run()
                step = self.plain_send(run, fixture(name))
                self.assertEqual((step["settled"], step["outcome"], step["answer"]), (False, "failed", ""))
                self.assertEqual(run.errors[0], "cli: probe: CLI exit 1")
                self.assertTrue(any(error.startswith("cli: probe: " + text) for error in run.errors), run.errors)
        # An error result fails the turn even when the CLI exits 0.
        run = self.plain_run()
        step = self.plain_send(run, fixture("claude-result-limit.jsonl"), returncode=0)
        self.assertEqual((step["outcome"], step["answer"]), ("failed", ""))
        self.assertEqual(run.errors, ["cli: probe: API Error: 429 (API status 429)"])

    def test_a_plain_cli_refusal_keeps_its_synthetic_reply_and_reset_time(self):
        run = self.plain_run()
        step = self.plain_send(run, fixture("claude-api-error-limit.jsonl"), stderr="")
        self.assertEqual(step["answer"], "")
        self.assertEqual(step["outcome"], "failed")
        self.assertEqual(run.errors, ["cli: probe: CLI exit 1",
                                      "cli: probe: synthetic reply: API request refused. (rate_limit)",
                                      "cli: probe: rate limit rejected: five_hour, resets 2026-09-24T04:20:00Z"])
        self.assertEqual(plain.cli_errors([{"type": "rate_limit_event", "rate_limit_info": {
            "status": "rejected", "rateLimitType": "seven_day", "resetsAt": 1791853200}}], "  signed out\n"),
            ["rate limit rejected: seven_day, resets 2026-10-13T01:00:00Z", "stderr: signed out"])
        # An allowed warning is not an error, and a successful turn's answer is unchanged.
        run = self.plain_run()
        step = self.plain_send(run, fixture("claude-result-limit.jsonl")[:1] + [
            {"type": "result", "result": "the answer", "is_error": False}], returncode=0, stderr="a warning")
        self.assertEqual((step["outcome"], step["answer"]), ("done", "the answer"))
        self.assertEqual(run.errors, [])

    def test_a_daemon_runs_synthetic_error_replies_are_kept(self):
        rows = [
            {"type": "assistant", "timestamp": "2026-10-24T16:42:00Z", "message": {"model": "claude-opus-5-5",
             "content": [{"type": "text", "text": "batch 1 read"}]}},
            {"type": "assistant", "timestamp": "2026-10-24T16:43:00Z", "isApiErrorMessage": False,
             "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "No response requested."}]}},
            {"type": "assistant", "timestamp": "2026-10-24T16:44:00Z", "isApiErrorMessage": True,
             "error": "rate_limit", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": LIMIT}]}},
            {"type": "assistant", "timestamp": "2026-10-24T16:45:00Z", "isApiErrorMessage": True,
             "error": "rate_limit", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": LIMIT}]}},
            {"type": "assistant", "timestamp": "2026-10-24T16:50:00Z", "isApiErrorMessage": True,
             "error": "unknown", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "during a fault"}]}},
        ]
        fault_ms = battery.row_ms(rows[-1])
        steps = [{"name": "seam-limit", "fault": True, "sent_ms": fault_ms - 1000, "ended_ms": fault_ms + 1000},
                 {"name": "probe", "sent_ms": fault_ms + 2000, "ended_ms": fault_ms + 3000}]
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            path = Path(scratch) / "native.jsonl"
            path.write_text("\n".join(map(json.dumps, rows)) + "\n")
            codex = Path(scratch) / "rollout.jsonl"
            codex.write_text(path.read_text())
            found = battery.transcript_errors([("claude", str(path)), ("codex", str(codex)),
                                               ("claude", str(Path(scratch) / "missing.jsonl"))], steps)
        self.assertEqual(found, [f"synthetic reply: {LIMIT} (rate_limit)"])
        self.assertEqual(battery.synthetic_replies([{**rows[2], "apiErrorStatus": 404, "error": "model_not_found"}]),
                         [f"synthetic reply: {LIMIT} (model_not_found, 404)"])

    def test_capture_puts_synthetic_error_text_in_run_problems(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            root = Path(scratch)
            native = root / "native.jsonl"
            native.write_text(json.dumps({"type": "assistant", "isApiErrorMessage": True, "error": "rate_limit",
                                          "message": {"model": "<synthetic>", "content": [{"type": "text", "text": LIMIT}]}}) + "\n")
            run = object.__new__(battery.Battery)
            run.args = SimpleNamespace(provider="claude")
            run.steps, run.errors, run.seams = [], [], []
            run.result = {"steps": run.steps, "seams": [], "errors": run.errors}
            run.conversation, run.root, run.data, run.repo = "test-conversation", root, root / "data", root / "repo"
            run.expected = {"absent_ledger": "secret"}
            run.record = Mock(return_value=root / "record")
            run.seam_file = Mock(return_value={"seams": []})
            run.entries = Mock(return_value=[{"atMs": 0, "entry": {"provider": "claude", "event": {
                "type": "sessionStarted", "nativeId": "native", "file": str(native)}}}])
            db = Mock()
            db.execute.return_value = []
            with patch("battery.sqlite3.connect") as connect, patch("battery.capture_roles", return_value={}), \
                 patch("lineages.compactions", return_value=0), patch("battery.native_calls", return_value={}):
                connect.return_value.__enter__.return_value = db
                run.capture()
        self.assertEqual(run.errors, [f"cli: synthetic reply: {LIMIT} (rate_limit)"])
        self.assertIn("Run problems: cli: synthetic reply: " + LIMIT, battery.report({
            **run.result, "started_at": "2026-10-24", "source": "fixture", "daemon_sha256": "unused",
            "model": "fixture", "threshold": 1}))

    def test_nightly_marks_cli_error_text_incomplete_even_on_exit_0(self):
        evidence_by_arm = {
            "seam": {"errors": ["cli: synthetic reply: " + LIMIT]},
            "plain": {"errors": ["incomplete: probe ended failed"]},
            "zed": {"errors": ["ballast-03: turn did not settle within bound"]},
            "summary": {"status": "skipped", "errors": [], "reason": "skipped: offline"},
        }

        class FakeProcess:
            returncode = 0

            def __init__(self, command, **kwargs):
                arm = command[command.index("--arm") + 1]
                Path(command[command.index("--evidence") + 1]).write_text(json.dumps(evidence_by_arm[arm]))

            def __enter__(self):
                return self

            def __exit__(self, *args):
                pass

            def wait(self, **kwargs):
                return 0

        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder, patch("nightly.subprocess.Popen", FakeProcess):
            statuses = {}
            for arm in evidence_by_arm:
                nightly.provider_report("codex", Path(folder), "/unused", True, arm, zed_long=True)
                report = json.loads((Path(folder) / f"codex-{arm}.json").read_text())
                statuses[arm] = {row["status"] for row in report["arms"] if row["block"] == "recall"}
        self.assertEqual(statuses, {"seam": {"incomplete"}, "plain": {"incomplete"},
                                    "zed": {"complete"}, "summary": {"skipped"}})


if __name__ == "__main__":
    unittest.main()
