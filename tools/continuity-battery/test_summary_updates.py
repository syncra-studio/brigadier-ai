"""Offline D66 transport, adversarial scoring, scenario and nightly coverage checks."""
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
import nightly
from plain import PlainBattery
import summary
import updates

PREFIX = "20261022-0805-take-over-5-sessions-w188-test-"


class SummaryTests(unittest.TestCase):
    def test_fetch_verbatim_pinned_skill_and_cache(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            skill = "---\nname: handoff\n---\n\nWrite a document.\n"
            with patch("summary.urlopen", return_value=io.BytesIO(skill.encode())) as fetch:
                self.assertEqual(summary.fetch_prompt(folder), skill)
                self.assertEqual(summary.fetch_prompt(folder), skill)
            fetch.assert_called_once_with(summary.PROMPT_URL, timeout=20)
            self.assertIn(summary.COMMIT, summary.PROMPT_URL)

    def test_fresh_session_gets_only_read_then_pending_turn_and_repeats(self):
        for provider in ("claude", "codex"):
            with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
                root = Path(folder)
                run = object.__new__(summary.SummaryBattery)
                run.root, run.repo = root, root
                run.args = SimpleNamespace(provider=provider, tokens=50)
                run.hot_tokens, run.native_id = 50, "old"
                run.summary_handoffs, run.raw_events = [], []
                run.handoff_skill = "verbatim skill\n"
                run.result = {}
                seen = []
                def send(instance, name, text, fault=None):
                    seen.append((name, text, run.native_id))
                    if name.startswith("summary-write"):
                        (root / f"summary-{run.generations() + 1}.md").write_text("model document")
                    if run.native_id is None:
                        run.native_id = f"fresh-{run.generations() + 1}"
                    return {"settled": True, "answer": "okay"}
                with patch.object(PlainBattery, "send", send), patch.object(run, "update_context"):
                    run.send("scenario-1", "pending turn")
                    run.send("scenario-2", "second pending turn")
                self.assertEqual(seen[0][1], run.handoff_skill + f"\n\nSave the handoff document to {root}/summary-1.md.")
                self.assertEqual(seen[1], ("summary-read-1", f"Read {root}/summary-1.md and continue.", None))
                self.assertEqual(seen[2], ("scenario-1", "pending turn", "fresh-1"))
                self.assertEqual(seen[3][2], "fresh-1")
                self.assertEqual(seen[4][2], None)
                self.assertEqual(seen[5][1], "second pending turn")
                self.assertEqual(run.generations(), 2)
                self.assertEqual(run.environment().get("DISABLE_AUTO_COMPACT"), "1" if provider == "claude" else None)

    def test_threshold_and_missing_document_do_not_start_successor(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            run = object.__new__(summary.SummaryBattery)
            run.root = Path(folder)
            run.args = SimpleNamespace(tokens=50)
            run.hot_tokens, run.native_id = 49, "old"
            run.raw_events, run.summary_handoffs = [], []
            run.handoff_skill = "skill"
            with patch.object(PlainBattery, "send", return_value={"settled": True}) as send, \
                 patch.object(run, "update_context"):
                run.send("below", "ordinary")
                self.assertEqual(send.call_count, 1)
                run.hot_tokens = 50
                with self.assertRaisesRegex(RuntimeError, "document failed"):
                    run.send("at", "pending")
                self.assertEqual(send.call_count, 2)
                self.assertEqual(run.native_id, "old")

    def test_fetch_failure_skips_all_entrypoints_and_cleans_scratch(self):
        for module in ("battery", "decisions", "updates"):
            with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
                report, evidence = Path(folder) / "report.md", Path(folder) / "evidence.json"
                source = (f"import sys, {module}; from unittest.mock import patch; "
                          f"sys.argv=['{module}', '--daemon', '/dev/null', '--arm', 'summary', "
                          f"'--report', {str(report)!r}, '--evidence', {str(evidence)!r}, '--scratch-prefix', {PREFIX!r}]; "
                          "p=patch('summary.urlopen', side_effect=OSError('offline')); p.start(); "
                          f"sys.exit({module}.main())")
                result = subprocess.run([sys.executable, "-B", "-c", source], cwd=Path(__file__).parent,
                                        capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr)
                saved = json.loads(evidence.read_text())
                self.assertEqual(saved["status"], "skipped")
                self.assertEqual(saved["errors"], [])
                self.assertIn("could not fetch pinned handoff skill", report.read_text())

    def test_capture_keeps_summary_calls_in_parent_and_successor_costs(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            root = Path(folder)
            files = []
            for i in range(2):
                file = root / f"native-{i}.jsonl"
                file.write_text(json.dumps({"type": "assistant", "message": {"id": "a",
                    "model": "claude-opus-5-5", "usage": {"input_tokens": 10, "output_tokens": 2}}}))
                files.append(str(file))
            run = object.__new__(summary.SummaryBattery)
            run.args, run.root, run.repo = SimpleNamespace(provider="claude"), root, root
            run.native_sessions = dict(zip(("old", "fresh"), files))
            run.native_id, run.raw_events, run.result, run.summary_handoffs = "fresh", [], {}, [{}]
            with patch.object(run, "native_path", return_value=Path(files[-1])):
                run.capture()
            self.assertEqual(set(run.result["roles"]), {"parent", "successor"})
            self.assertEqual(run.result["roles"]["parent"]["uncached_input"], 10)
            self.assertEqual(run.result["roles"]["successor"]["uncached_input"], 10)
            self.assertEqual(run.result["summary_compactions"], 1)
            self.assertEqual(run.result["compactions"], 0)


class UpdatesTests(unittest.TestCase):
    def test_thirty_random_items_and_newest_reversals(self):
        early, revisions, expected, stale = updates.scenario()
        self.assertEqual(len(expected), 30)
        self.assertEqual(len(updates.FACTS), 18)
        self.assertEqual(len(stale), 8)
        self.assertEqual(set(expected), set(updates.DESCRIPTIONS))
        for key in updates.DECISIONS:
            self.assertEqual(expected[key], revisions[-1][key])
            self.assertNotIn(expected[key], stale[key])
        self.assertEqual(expected["region"], early["region"])
        self.assertEqual(expected["queue"], early["queue"])
        self.assertNotEqual(expected, updates.scenario()[2])

    def test_recall_newest_stale_and_open_promises_are_separate(self):
        early, _, expected, stale = updates.scenario()
        perfect, scores = updates.grade(json.dumps(expected), expected, stale, " ".join(expected.values()))
        self.assertEqual(scores["recall"], 1)
        self.assertEqual(scores["newest_decision_wins"], 1)
        self.assertEqual(scores["stale_decision_mistakes"], 0)
        self.assertEqual(scores["open_promises_raised"], 3)
        metrics = nightly.metrics({"block": "updates", "items": perfect, "update_scores": scores})
        self.assertIsNone(metrics["task_passed"])
        self.assertTrue(metrics["checks"]["newest.region"])
        self.assertTrue(metrics["checks"]["no_stale.region"])
        answer = dict(expected)
        answer["timeout"] = stale["timeout"][0]
        answer["region"] += " or " + stale["region"][0]
        answer["queue"] = "unknown"
        answer["release"] = "PREFIX-" + expected["release"]
        rows, scores = updates.grade(json.dumps(answer), expected, stale)
        self.assertEqual(scores["newest_decision_wins"], 5 / 8)
        self.assertEqual(scores["stale_decision_mistakes"], 2)
        self.assertEqual(scores["recall"], 25 / 29)
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            report = Path(folder) / "report.md"
            result = {"block": "updates", "update_scores": scores, "items": rows, "expected": expected}
            battery.write_report(result, "synthetic report", report)
            public = json.loads(report.with_suffix(".json").read_text())
            self.assertEqual(public["update_scores"], scores)
            self.assertNotIn(expected["release"], json.dumps(public))
        self.assertEqual(scores["open_promises_raised"], 0)
        self.assertEqual(rows["queue"]["verdict"], "unknown")
        missing, scores = updates.grade("invalid", expected, stale)
        self.assertEqual(scores["recall"], 0)
        self.assertEqual(scores["stale_decision_mistakes"], 0)
        # A value under a different key cannot earn credit.
        wrong = dict(expected)
        wrong["release"], wrong["ticket"] = wrong["ticket"], wrong["release"]
        self.assertEqual(updates.grade(json.dumps(wrong), expected, stale)[1]["recall"], 27 / 29)

    def test_schedule_tool_removal_and_updates_are_identical_for_every_arm(self):
        schedules = []
        for arm in nightly.ARMS:
            with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
                run = object.__new__(updates.Updates)
                run.repo = Path(folder)
                run.args = SimpleNamespace(arm=arm, max_fillers=9, ballast=3)
                run.result = {}
                sent = []
                def send(name, text):
                    sent.append((name, text))
                    if name == "plant-diagnostic":
                        script = (run.repo / "diagnostic.py").read_text()
                        self.assertIn(run.result["expected"]["tool_receipt"], script)
                        output = subprocess.check_output([sys.executable, str(run.repo / "diagnostic.py")])
                        self.assertGreater(len(output), 250_000)
                        self.assertFalse((run.repo / "diagnostic.py").exists())
                        self.assertEqual(output.count(run.result["expected"]["tool_receipt"].encode()), 1)
                    if name.startswith("ballast"):
                        self.assertFalse((run.repo / "diagnostic.py").exists())
                    return {"answer": json.dumps(run.result["expected"]) if name == "probe" else "read"}
                with patch.object(run, "setup"), patch.object(run, "send", side_effect=send), \
                     patch.object(run, "generations", return_value=0), patch.object(run, "wait"):
                    run.run()
                self.assertEqual(run.result["update_scores"]["recall"], 1)
                schedules.append([name for name, _ in sent])
        self.assertTrue(all(schedule == schedules[0] for schedule in schedules))
        self.assertEqual([name for name in schedules[0] if name.startswith("revision")], ["revision-3", "revision-6"])

    def test_nightly_every_arm_runs_both_updates_sizes_and_reports_scores(self):
        commands = []
        _, _, expected, stale = updates.scenario()
        items, scores = updates.grade(json.dumps(expected), expected, stale)
        class FakeProcess:
            returncode = 0
            def __init__(self, command, **kwargs):
                commands.append(command)
                evidence = Path(command[command.index("--evidence") + 1])
                block = "updates" if "updates.py" in command[1] else "other"
                result = {"errors": [], "block": "updates", "items": items, "update_scores": scores} if block == "updates" else {"errors": []}
                evidence.write_text(json.dumps(result))
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def wait(self, **kwargs):
                return 0
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder, patch("nightly.subprocess.Popen", FakeProcess):
            root = Path(folder)
            for arm in nightly.ARMS:
                nightly.provider_report("codex", root / "input", "/unused", True, arm)
            summary_report = nightly.collect(root / "input", root / "output")
            for provider in summary_report["providers"]:
                if provider["provider"] != "codex":
                    continue
                for arm in nightly.ARMS:
                    rows = [row for row in provider["arms"] if row["arm"] == arm and row["block"] == "updates"]
                    self.assertEqual({row["profile"] for row in rows}, {"short", "long"})
                    self.assertTrue(all(row["status"] == "complete" and row["update_scores"] == scores for row in rows))
            text = (root / "output/report.md").read_text()
            self.assertIn("newest decision wins 1.0", text)
            self.assertIn("stale-decision mistakes 0/8", text)
            self.assertIn("Summary handoffs", text)
            selected = [c for c in commands if c[1].endswith("updates.py")]
            self.assertEqual(len(selected), 8)
            self.assertEqual({c[c.index("--max-fillers") + 1] for c in selected}, {"9", "90"})


if __name__ == "__main__":
    unittest.main()
