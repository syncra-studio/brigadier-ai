"""Offline checks for the measured compaction algorithm; no network or model calls."""
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import battery
import decisions
import nightly
from plain import PlainBattery
import zed

PREFIX = "20261022-0805-take-over-5-sessions-w161-test-"


class ZedTests(unittest.TestCase):
    def test_decimal_byte_budget_newest_selection_and_oldest_prefix(self):
        newest = "n" * 30_000
        middle = "m" * 40_000
        oldest = "o" * 15_000
        self.assertEqual(zed.retained_users(["discard", oldest, middle, newest]),
                         [oldest[:10_000], middle, newest])
        self.assertEqual(sum(len(s.encode()) for s in zed.retained_users([oldest, middle, newest])), 80_000)
        self.assertEqual(zed.retained_users(["old", "", "é🙂end"], 7), ["é🙂e"])
        self.assertEqual(zed.retained_users(["older", "🙂"], 3), [])
        self.assertEqual(zed.retained_users(["old", "new"], 0), [])

    def test_bootstrap_replays_chronologically_before_summary(self):
        self.assertEqual(zed.bootstrap(["first", "second", "third"], "handoff"),
                         "first\n\nsecond\n\nthird\n\n" + zed.SUMMARY_PREFIX + "handoff")

    def test_prompt_fetch_uses_pinned_url_and_scratch_cache(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            with patch("zed.urlopen", return_value=io.BytesIO(b"synthetic test prompt")) as fetch:
                self.assertEqual(zed.fetch_prompt(folder), "synthetic test prompt")
                self.assertEqual(zed.fetch_prompt(folder), "synthetic test prompt")
            fetch.assert_called_once_with(zed.PROMPT_URL, timeout=20)
            self.assertIn(zed.COMMIT, zed.PROMPT_URL)

    def test_fetch_failure_is_a_skipped_report_for_both_scripts(self):
        # Exercise actual entry points and their report/cleanup paths in separate interpreters.
        for module in ("battery", "decisions"):
            with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
                output = Path(folder) / "report.md"
                evidence = Path(folder) / "evidence.json"
                source = ("from unittest.mock import patch; import sys; "
                          f"import {module}; "
                          f"sys.argv = ['{module}', '--daemon', '/dev/null', '--arm', 'zed', "
                          f"'--report', {str(output)!r}, '--evidence', {str(evidence)!r}, "
                          f"'--scratch-prefix', {PREFIX!r}]; "
                          "p = patch('zed.urlopen', side_effect=OSError('offline')); p.start(); "
                          f"sys.exit({module}.main())")
                ran = subprocess.run([sys.executable, "-B", "-c", source], cwd=Path(__file__).parent,
                                     capture_output=True, text=True, timeout=20)
                self.assertEqual(ran.returncode, 0, ran.stderr)
                result = json.loads(evidence.read_text())
                self.assertEqual(result["status"], "skipped")
                self.assertEqual(result["errors"], [])
                self.assertIn("skipped: could not fetch", output.read_text())
                self.assertIn("skipped: could not fetch", json.loads(output.with_suffix(".json").read_text())["reason"])

    def test_context_uses_latest_request_including_cached_tokens(self):
        events = [{"type": "assistant", "message": {"usage": {"input_tokens": 10000}}},
                  {"type": "assistant", "message": {"usage": {"input_tokens": 1,
                   "cache_read_input_tokens": 20, "cache_creation_input_tokens": 3, "output_tokens": 4}}},
                  {"type": "result", "usage": {"input_tokens": 50000}}]
        self.assertEqual(zed.active_tokens(events, "claude"), 28)
        self.assertEqual(zed.active_tokens([{"payload": {"type": "token_count", "info": {
            "total_token_usage": {"input_tokens": 90000}, "last_token_usage": {
                "input_tokens": 50, "cached_input_tokens": 30, "output_tokens": 2}}}}], "codex"), 52)

    def test_repeated_compaction_resumes_old_session_then_bootstraps_fresh(self):
        for provider in ("claude", "codex"):
            run = object.__new__(zed.ZedBattery)
            run.args = SimpleNamespace(provider=provider, tokens=50, model="test", turn_timeout=1)
            run.repo, run.deadline = Path("/tmp"), time.monotonic() + 30
            run.steps, run.errors, run.processes, run.raw_events = [], [], [], []
            run.result, run.native_id, run.native_sessions = {}, "session-0", {"session-0": "/old"}
            run.user_messages, run.zed_compactions = ["oldest", "latest"], []
            run.hot_tokens, run.compaction_prompt = 50, "synthetic compaction request"
            ids = iter(("session-0", "session-1", "session-1", "session-2"))
            inputs = []
            def communicate(text, **kwargs):
                inputs.append(text)
                sid = next(ids)
                return "\n".join(map(json.dumps, [
                    {"type": "system", "session_id": sid},
                    {"type": "assistant", "message": {"content": [{"type": "text", "text": "summary"}],
                     "usage": {"input_tokens": 50, "output_tokens": 1}}},
                    {"type": "result", "result": "summary", "usage": {"input_tokens": 50}}])), ""
            fake = SimpleNamespace(returncode=0, communicate=communicate)
            with patch("plain.subprocess.Popen", return_value=fake) as launch, \
                 patch.object(run, "native_path", return_value=None), \
                 patch.object(run, "update_context", side_effect=lambda start: setattr(run, "hot_tokens", 51)):
                run.send("next", "next request")
                first_commands = [call.args[0] for call in launch.call_args_list]
                run.send("last", "last request")
            self.assertEqual(inputs[0], run.compaction_prompt)
            self.assertEqual(inputs[1], zed.bootstrap(["oldest", "latest"], "summary") + "\n\nnext request")
            self.assertEqual(inputs[3], zed.bootstrap(["oldest", "latest", "next request"], "summary") + "\n\nlast request")
            self.assertIn("session-0", first_commands[0])
            self.assertNotIn("session-0", first_commands[1])
            if provider == "claude":
                self.assertIn("--tools", first_commands[0])
                self.assertNotIn("--tools", first_commands[1])
                self.assertEqual(launch.call_args.kwargs["env"]["DISABLE_AUTO_COMPACT"], "1")
            self.assertEqual(run.generations(), 2)
            self.assertEqual(len(run.result["usage"]), 4)
            self.assertEqual(run.user_messages, ["oldest", "latest", "next request", "last request"])

    def test_capture_accounts_for_all_sessions_and_native_compactions(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            root = Path(folder)
            files = []
            for i in range(2):
                file = root / f"native-{i}.jsonl"
                file.write_text(json.dumps({"type": "assistant", "message": {"id": "a",
                    "model": "claude-opus-5-5", "usage": {"input_tokens": 10, "output_tokens": 2}}}))
                files.append(str(file))
            run = object.__new__(zed.ZedBattery)
            run.args, run.root, run.repo = SimpleNamespace(provider="claude"), root, root
            run.native_sessions = dict(zip(("a", "b"), files))
            run.result, run.raw_events, run.native_id = {}, [], "b"
            run.zed_compactions = [{}]
            with patch.object(run, "native_path", return_value=Path(files[1])):
                run.capture()
            self.assertEqual(set(run.result["roles"]), {"parent", "successor"})
            self.assertEqual(run.result["roles"]["parent"]["uncached_input"], 10)
            self.assertEqual(run.result["roles"]["successor"]["uncached_input"], 10)
            self.assertEqual(run.result["compactions"], 0)
            self.assertEqual(run.result["zed_compactions"], 1)

    def test_reports_compare_all_checks_prices_and_skip_reasons(self):
        fixture = {"arm": "zed", "rows": {"own_decision": {"unprompted": True, "recalled": True}},
                   "t1": {key: True for key in ("jsdoc", "named", "spdx", "padding", "dates")},
                   "t2_scored": {"pass": True}, "t3": {"items": {"a": "stated"}, "unmarked_additions": []},
                   "t4_scored": {"open_question": True, "open_promise": True}}
        checks = nightly.metrics(fixture)["checks"]
        self.assertTrue(all(checks[key] for key in ("T1", "T2", "T3", "T4", "T5", "T6")))
        rows = [{"block": "decisions", "profile": "long", "arm": arm, "status": "complete",
                 "checks": checks, "recall": 1, "contexts_before_checks": 3,
                 "list_price_usd": cost, "usage": {"cost_usd": cost}}
                for arm, cost in (("seam", 1), ("zed", 2), ("plain", 3))]
        rows.append({"block": "recall", "profile": "short", "arm": "zed", "status": "skipped",
                     "reason": "skipped: could not fetch pinned Zed compaction prompt (offline)"})
        summary = {"run_url": "fixture", "source": "fixture", "regressions": [], "providers": [
            {"provider": "claude", "status": "incomplete", "arms": rows}]}
        text = nightly.markdown(summary)
        comparison = {r["check"]: r for r in summary["seam_vs_zed"] if r["block"] == "decisions"}
        for key in ("T1", "T2", "T3", "T4", "T5", "T6", "list_price_usd", "reported_cost_usd"):
            self.assertTrue(comparison[key]["seam_meets_zed"])
        self.assertEqual(comparison["list_price_usd"]["dollar_ratio"], .5)
        self.assertIn("skipped: could not fetch", text)
        rows[1]["contexts_before_checks"] = 2
        rows[1]["list_price_usd"] = None
        nightly.prepare(summary)
        comparison = next(r for r in summary["seam_vs_zed"] if r["check"] == "list_price_usd")
        self.assertIsNone(comparison["seam_meets_zed"])
        self.assertFalse(comparison["qualified"])

    def test_nightly_collect_requires_and_keeps_all_three_arms(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            root = Path(folder)
            for provider in ("claude", "codex"):
                for arm in nightly.ARMS:
                    nightly.provider_report(provider, root / "input", "/unused", False, arm)
            result = nightly.collect(root / "input", root / "output")
            self.assertTrue(all(p["status"] == "skipped" for p in result["providers"]))

    def test_zed_decisions_share_scenarios_and_plain_own_decision_scoring(self):
        class ZedDecisions(decisions.Decisions, zed.ZedBattery):
            pass
        self.assertIs(ZedDecisions.run, decisions.Decisions.run)
        self.assertIs(ZedDecisions.send, zed.ZedBattery.send)
        result = {"arm": "zed", "plant_replies": {"plant-own-decision": "I chose a comma separator"},
                  "t1": {"separator_used": ","}, "t3": {"items": {}, "unmarked_additions": []}}
        self.assertTrue(decisions.score(result, "unused")["own_decision"]["unprompted"])


if __name__ == "__main__":
    unittest.main()
