"""Offline tests for PROPOSALS 3A fixes 1, 2, 4, 5, 10 and 11."""
import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import arms
import battery
import codex_transport
import experiment
import lookup
import nightly
import paired
from plain import PlainBattery
import profiles
import retention

PREFIX = "20261024-1334-long-session-memory-w11-test-"


class ProfileRepairs(unittest.TestCase):
    def test_fix1_plain_threshold_is_opt_in_and_switch_arms_use_real_build(self):
        run = object.__new__(PlainBattery)
        run.args = SimpleNamespace(provider="claude", model="fixture", arm="plain@200k", tokens=200000,
                                   shared_transition=True, turn_timeout=10)
        run.repo, run.deadline = Path("/tmp"), time.monotonic() + 10
        run.steps, run.errors, run.processes, run.raw_events = [], [], [], []
        run.result, run.native_id, run.native_sessions = {}, None, {}
        fake = SimpleNamespace(returncode=0, communicate=lambda *a, **k: (
            '{"type":"result","session_id":"fixture","result":"done"}', ""))
        with patch("plain.subprocess.Popen", return_value=fake) as launch:
            run.send("first", "work")
        command = launch.call_args.args[0]
        self.assertEqual(command[command.index("--autocompact") + 1], "200000")
        with tempfile.TemporaryDirectory(prefix=PREFIX) as directory:
            daemon = Path(directory) / "daemon"
            daemon.write_bytes(b"old-build")
            run = object.__new__(arms.CandidateBattery)
            run.args = SimpleNamespace(daemon=str(daemon), arm="native")
            with self.assertRaisesRegex(arms.SkippedArm, "build lacks"):
                run.setup()
            daemon.write_bytes(b"real build " + arms.LONG_SESSION_SWITCH.encode())
            self.assertTrue(arms.supports_switch(daemon))
            self.assertEqual(run.configure_environment({})[arms.LONG_SESSION_SWITCH], "native")

    def test_fix1_codex_appserver_threshold_and_thread_session_are_native(self):
        run = SimpleNamespace(args=SimpleNamespace(arm="plain@200k", tokens=200000), repo=Path("/tmp"),
                              environment=lambda: {}, processes=[], deadline=time.monotonic() + 10)
        fake = Mock(stdout=io.StringIO(""))
        with patch("codex_transport.subprocess.Popen", return_value=fake) as launch, \
             patch.object(codex_transport.Client, "call"), patch.object(codex_transport.Client, "write"):
            client = codex_transport.Client(run)
            self.assertEqual(launch.call_args.args[0], ["codex", "app-server", "-c",
                             'model_reasoning_effort="high"', "-c", "model_auto_compact_token_limit=200000"])
        run = SimpleNamespace(args=SimpleNamespace(provider="codex", model="fixture", turn_timeout=10),
                              tick=lambda: None, steps=[], errors=[], raw_events=[],
                              deadline=time.monotonic() + 10, native_id="thread", native_sessions={"thread": "/exact/rollout"})
        client = Mock(events=[])
        run.codex_client = client
        def call(method, params):
            client.events.extend([{"method": "item/completed", "params": {"item": {"type": "agentMessage", "text": "done"}}},
                                  {"method": "turn/completed", "params": {"turn": {"id": "turn", "status": "completed"}}}])
            return {"turn": {"id": "turn"}}
        client.call.side_effect = call
        self.assertEqual(codex_transport.send(run, "work", "prompt")["answer"], "done")
        self.assertEqual(client.call.call_args.args[0], "turn/start")
        self.assertEqual(client.call.call_args.args[1]["threadId"], "thread")
        self.assertNotIn("exec", str(client.call.call_args))

    def test_fix2_small_work_packets_are_reproducible_and_useful(self):
        packet, total = profiles.packet(123, 4)
        obj = json.loads(packet)
        self.assertLessEqual(len(packet.encode()), 2048)
        self.assertEqual(sum(obj["amounts"]), total)
        self.assertEqual(profiles.packet(123, 4), (packet, total))
        self.assertNotEqual(profiles.packet(124, 4), (packet, total))
        self.assertNotEqual(profiles.packet(123, 5), (packet, total))

    def test_fix4_all_arms_get_identical_read_only_lookup_access(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as directory:
            path = Path(directory) / "archive.json"
            original = '{ "type": "tool_result", "text": "FACT-123" }'
            path.write_text(json.dumps([{"id": "exact-id", "raw": original}]))
            self.assertEqual(lookup.query(path, "search", "FACT-123")[0]["raw"], original)
            self.assertEqual(lookup.query(path, "read", "exact-id")[0]["raw"], original)
            self.assertEqual(lookup.query(path, "read", "missing"), [])
            run = object.__new__(profiles.Work)
            run.lookup_script, run.archive = Path(directory) / "continuity_lookup.py", path
            instructions = []
            for arm in arms.ARMS:
                run.args = SimpleNamespace(arm=arm)
                instructions.append(run.lookup_instruction())
            self.assertEqual(len(set(instructions)), 1)
            self.assertIn("lookup is allowed", instructions[0])
            self.assertIn("before saying unknown", instructions[0])
            self.assertNotIn("do not run commands", instructions[0])

    def test_fix5_native_payload_proves_retained_evicted_or_unverified_per_fact(self):
        user = {"type": "user", "uuid": "kept", "message": {"content": "KEEP-ME"}}
        boundary = {"type": "system", "subtype": "compact_boundary", "compactMetadata": {
            "preservedMessages": {"allUuids": ["kept"]}}}
        summary = {"type": "user", "isCompactSummary": True, "message": {"content": "All user messages: SUMMARY-FACT"}}
        for provider, rows in (("claude", [user, boundary, summary]), ("codex", [
                {"type": "compacted", "payload": {"message": "SUMMARY-FACT", "replacement_history": [user]}}])):
            snapshot = retention.snapshots(rows, provider)[-1]
            self.assertTrue(snapshot["complete"])
            states = retention.classify_facts({"kept": "KEEP-ME", "summary": "SUMMARY-FACT", "old": "GONE"}, snapshot)
            self.assertEqual(states, {"kept": "retained", "summary": "retained", "old": "evicted"})
        sealed = retention.snapshots([{"type": "compacted", "payload": {"message": "sealed"}}], "codex")[-1]
        self.assertEqual(retention.classify("old fact", sealed), "unverified")
        incomplete = retention.snapshots([boundary, summary], "claude")[-1]
        self.assertEqual(retention.classify("old fact", incomplete), "unverified")
        self.assertEqual(retention.classify("old fact", None), "unverified")

    def test_fix5_facts_are_disjoint_small_tool_only_and_not_initial_message(self):
        cases = [profiles.scenario(123, group) for group in "ABC"]
        identifiers = [{value for key, value in case["facts"].items() if key != "closed"} for case in cases]
        self.assertTrue(all(not identifiers[i] & identifiers[j] for i in range(3) for j in range(i + 1, 3)))
        self.assertEqual(cases[0], profiles.scenario(123, "A"))
        with tempfile.TemporaryDirectory(prefix=PREFIX) as directory:
            run = object.__new__(profiles.Work)
            run.repo, run.args = Path(directory), SimpleNamespace(seed=123)
            run.steps, run.snapshot_history, run.result = [], [], {}
            messages = []
            def observe(name, text):
                messages.append((name, text))
                if name.startswith("tool-"):
                    self.assertLessEqual((run.repo / "receipt.txt").stat().st_size, 2048)
            run.observed = observe
            case = run.plant("A")
            self.assertNotIn(case["facts"]["tool_fact"], messages[0][1])
            self.assertFalse((run.repo / "receipt.txt").exists())
            self.assertIn("General artifact label", messages[0][1])

    def test_fix10_paired_intervals_pilot_bar_and_cost_per_pass(self):
        rows = []
        for index, diff in enumerate((0, .02, -.01, 0)):
            for arm, score, price in (("plain@200k", .9, 2), ("native", .9 + diff, 1)):
                rows.append({"provider": "claude", "profile": "work", "arm": arm, "seed": index,
                             "lineage": index, "status": "complete", "run_kind": "continuity",
                             "memory_keys": ["A.user_fact"], "recall_apply": score, "list_price_usd": price,
                             "decision_violations": {"rule": 0}, "fully_passing": True})
        pilot = paired.compare(rows)[0]
        self.assertEqual(pilot["pairs"], 4)
        self.assertAlmostEqual(pilot["mean_difference"], .0025)
        self.assertIsNone(pilot["non_inferior"])
        self.assertTrue(paired.compare(rows, phase="confirmatory")[0]["non_inferior"])
        rows[-1]["decision_violations"]["rule"] = 1
        self.assertFalse(paired.compare(rows, phase="confirmatory")[0]["non_inferior"])
        rows[-1]["run_kind"] = "fault-recovery"
        self.assertEqual(paired.compare(rows)[0]["pairs"], 3)
        rows[-3]["seed"] = 999
        self.assertEqual(paired.compare(rows)[0]["pairs"], 2)
        self.assertIsNone(paired.interval90([.1]))
        self.assertEqual(paired.cost_per_pass([{"status": "complete", "fully_passing": True, "list_price_usd": 2},
                                              {"status": "incomplete", "list_price_usd": 3}]), 5)
        self.assertIsNone(paired.cost_per_pass([{"status": "complete", "fully_passing": True, "list_price_usd": None}]))

    def test_fix11_endurance_probes_are_disjoint_and_depths_are_observed(self):
        class FakeWork(profiles.Work):
            def setup(self):
                self.work_count, self.probed, self.messages = 0, [], []
                self.snapshot_history = []
            def setup_archive(self):
                pass
            def lookup_instruction(self):
                return "lookup allowed"
            def observed(self, name, text):
                self.messages.append((name, text))
                self.steps.append({"name": name})
                return {"settled": True, "answer": "done"}
            def work_turn(self, index):
                self.work_count = max(0, index - 2)
            def generations(self):
                return self.work_count
            def checkpoint_ids(self):
                return []
            def probe(self, case, depth):
                self.probed.append((case["group"], depth, case["facts"]["scoped"]))
        with tempfile.TemporaryDirectory(prefix=PREFIX) as directory:
            run = object.__new__(FakeWork)
            run.args = SimpleNamespace(profile="endurance", arm="plain@200k", seed=123, lineage=0,
                                       shared_transition=True, turns=15)
            run.repo, run.steps, run.errors, run.result = Path(directory), [], [], {}
            run.run()
            self.assertEqual([(group, depth) for group, depth, _ in run.probed], [("A", 3), ("B", 6), ("C", 10)])
            self.assertEqual(run.probed[0][2], profiles.scenario(123, "A")["old"]["scoped"])
            self.assertEqual(run.probed[1][2], profiles.scenario(123, "B")["facts"]["scoped"])
            names = [name for name, _ in run.messages]
            self.assertEqual(sum(name.startswith("revision-") for name in names), 3)
            self.assertLess(names.index("plant-C"), names.index("revision-A"))
            run.args.turns = 8
            run.steps, run.errors, run.result = [], [], {}
            run.run()
            self.assertIn("fewer than ten", run.errors[0])

    def test_no_model_dry_run_and_existing_nightly_schedule_unchanged(self):
        self.assertEqual(nightly.ARMS, ("seam", "plain", "zed", "summary"))
        self.assertEqual(nightly.PROFILES, (("recall", "short", 9, 1200), ("recall", "long", 90, 3500),
                         ("decisions", "long", 90, 3500), ("updates", "short", 9, 1200), ("updates", "long", 90, 3500)))
        with tempfile.TemporaryDirectory(prefix=PREFIX) as directory, patch("experiment.run_one") as model:
            report = Path(directory) / "plan.json"
            with contextlib.redirect_stdout(io.StringIO()):
                code = experiment.main(["--profile", "endurance", "--repetitions", "4", "--dry-run", "--report", str(report)])
            self.assertEqual(code, 0)
            model.assert_not_called()
            plan = json.loads(report.read_text())
            self.assertEqual(plan["probe_depths"], [3, 6, 10])
            self.assertEqual(plan["seeds"], [1729, 1730, 1731, 1732])
            self.assertEqual(plan["shared_transition_tokens"], 200000)
            self.assertFalse(plan["faults"])


if __name__ == "__main__":
    unittest.main()


class ProbeAndReportingTests(unittest.TestCase):
    def test_real_probe_checks_artifact_scope_closed_replacement_and_memory_separately(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as directory:
            run = object.__new__(profiles.Work)
            run.repo = Path(directory)
            run.repo.joinpath("src/artifacts").mkdir(parents=True)
            run.args = SimpleNamespace(seed=321, profile="work", provider="claude", arm="seam", lineage=0,
                                       model="fixture", tokens=200000, turns=300)
            case = profiles.scenario(321, "A")
            facts = case["facts"]
            run.snapshot_history = [{"complete": True, "kept": facts["user_fact"]}]
            run.result = {"source": "source", "daemon_sha256": "hash"}
            run.errors = []
            run.lookup_instruction = lambda: "lookup allowed"
            run.checkpoint_ids = lambda: ["L1"]
            prompts = []
            def observe(name, text):
                prompts.append(text)
                if name == "apply-A":
                    file = run.repo / "src/artifacts/A.json"
                    file.write_text(json.dumps({"stage": "ready", "release": facts["release"],
                        "timeout_ms": int(facts["units"].split()[0]), "invoice_width": int(facts["decision"].split()[0]),
                        "general": facts["rule"], "exporter": facts["scoped"], "artifact": facts["artifact"]}))
                return {"answer": (facts["replacement"] + " " + facts["open"]) if name == "status-A" else json.dumps(facts)}
            run.observed = observe
            run.probe(case, 3)
            score = run.result["depth_scores"][0]
            self.assertEqual(score["recall_apply"], 1)
            self.assertEqual(score["memory_items"], 11)
            self.assertEqual(score["retained"], 1)
            self.assertTrue(all(row["verdict"] == "pass" for row in score["items"].values()))
            self.assertNotIn(facts["rule"], prompts[0])  # Apply task never restates the rule.
            run.result["items"] = {"A." + key: row for key, row in score["items"].items()}
            run.result["work_checks"] = [True]
            summary = experiment.summary_row(run)
            self.assertNotIn("A.user_fact", summary["memory_keys"])
            self.assertEqual(summary["retained_items"], 1)
            self.assertTrue(summary["fully_passing"])
            report = Path(directory) / "report.json"
            experiment.write_summary(report, {"plan": {"phase": "pilot"}, "arms": [summary], "paired": [],
                                               "cost_per_fully_passing_lineage": {"seam": None}})
            public = report.read_text() + report.with_suffix(".md").read_text()
            self.assertNotIn(facts["user_fact"], public)
            self.assertIn("Cache write", public)
            self.assertIn("Retained", public)

    def test_pairs_with_different_build_or_workload_do_not_match(self):
        base = {"provider": "codex", "profile": "work", "seed": 123, "lineage": 0,
                "status": "complete", "run_kind": "continuity", "source": "abc", "daemon_sha256": "hash",
                "turns": 300, "threshold": 200000, "model": "fixture", "memory_keys": ["A.tool_fact"], "recall_apply": 1}
        control, candidate = {**base, "arm": "plain@200k"}, {**base, "arm": "native"}
        self.assertEqual(paired.compare([control, candidate])[0]["pairs"], 1)
        for key, value in (("source", "different"), ("daemon_sha256", "different"), ("turns", 500), ("threshold", 300000)):
            self.assertEqual(paired.compare([control, {**candidate, key: value}])[0]["pairs"], 0)

    def test_fix11_ledger_losses_use_kept_context_and_putback_not_disk_existence(self):
        rows = [{"type": "compacted", "payload": {"replacement_history": []}},
                {"type": "response_item", "payload": {"type": "message", "role": "developer",
                                                        "content": [{"text": "L1: rule restored whole"}]}}]
        snapshot = retention.snapshots(rows, "codex")[-1]
        self.assertEqual(retention.dropped_ids(["L1", "L2"], snapshot), ["L2"])
        self.assertIsNone(retention.dropped_ids(["L1"], {"complete": False, "kept": "sealed"}))
        self.assertEqual(retention.dropped_ids([], None), [])

    def test_new_codex_transport_deletes_only_its_recorded_threads(self):
        run = object.__new__(PlainBattery)
        run.codex_client = Mock()
        run.native_sessions = {"owned-one": "one.jsonl", "owned-two": "two.jsonl"}
        run.deadline, run.cleanup_notes = time.monotonic() + 30, []
        run.cleanup_native()
        self.assertEqual(run.codex_client.call.call_args_list[0].args, ("thread/delete", {"threadId": "owned-one"}))
        self.assertEqual(run.codex_client.call.call_args_list[1].args, ("thread/delete", {"threadId": "owned-two"}))

    def test_fix5_sealed_summary_never_proves_absence_even_with_replacement_history(self):
        snapshot = retention.snapshots([{"type": "compacted", "payload": {
            "replacement_history": [{"type": "compaction", "encrypted_content": "opaque-sealed-bytes"}]}}], "codex")[-1]
        self.assertFalse(snapshot["complete"])
        self.assertEqual(retention.classify("FACT", snapshot), "unverified")
