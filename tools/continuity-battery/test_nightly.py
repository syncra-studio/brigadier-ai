"""Deterministic report/transport checks; never calls a model or daemon."""
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import battery
import decisions
import lineages
import nightly
from plain import PlainBattery


class NightlyTests(unittest.TestCase):
    def test_false_worker_claim_accepts_invalid(self):
        expected = {"tool_only": "VT-X", "absent_ledger": "ORCHID-X", "large_failure": "MIDFAIL-X"}
        answer = json.dumps({"false_worker": "Invalid: the report said writeDataFile writes atomically "
                             "through a temporary file and rename, but it actually just calls writeFileSync "
                             "directly, with no temp file or rename."})
        rows = battery.grade(answer, expected, {})
        self.assertEqual(rows["false_worker"]["verdict"], "pass")

    def test_zed_skips_long_profiles_unless_zed_long(self):
        with tempfile.TemporaryDirectory(prefix="20261022-0805-take-over-5-sessions-w164-zed-") as folder:
            root = Path(folder)
            with patch("nightly.subprocess.Popen", side_effect=OSError("no daemon")):
                nightly.provider_report("claude", root, "/missing/daemon", True, "zed")
            report = json.loads((root / "claude-zed.json").read_text())
            skipped = {(r["block"], r["profile"]) for r in report["arms"] if r["status"] == "skipped"}
            self.assertEqual(skipped, {("recall", "long"), ("decisions", "long")})
            self.assertTrue(all(r["reason"] == "zed: short only tonight (long on dispatch)"
                                 for r in report["arms"] if (r["block"], r["profile"]) in skipped))

    def test_zed_long_dispatch_attempts_every_profile(self):
        with tempfile.TemporaryDirectory(prefix="20261022-0805-take-over-5-sessions-w164-zed-long-") as folder:
            root = Path(folder)
            with patch("nightly.subprocess.Popen", side_effect=OSError("no daemon")):
                nightly.provider_report("claude", root, "/missing/daemon", True, "zed", zed_long=True)
            report = json.loads((root / "claude-zed.json").read_text())
            self.assertEqual({(r["block"], r["profile"]) for r in report["arms"]},
                              {("recall", "short"), ("recall", "long"), ("decisions", "long")})
            self.assertFalse(any(r["status"] == "skipped" for r in report["arms"]))

    def test_missing_secrets_still_yield_both_provider_artifact(self):
        with tempfile.TemporaryDirectory(prefix="20261022-0805-take-over-5-sessions-w117-report-") as folder:
            root = Path(folder)
            for provider in ("claude", "codex"):
                for arm in ("seam", "plain"):
                    nightly.provider_report(provider, root / "input", "/missing/daemon", False, arm)
            (root / "input/claude-recall-short-seam.json").write_text(json.dumps({"provider": "claude", "roles": {}}))
            result = nightly.collect(root / "input", root / "output")
            self.assertEqual([p["status"] for p in result["providers"]], ["skipped", "skipped"])
            self.assertTrue(all(p["reason"] == "needs the user: secrets" for p in result["providers"]))
            self.assertEqual(result["regressions"], [])
            self.assertIn("needs the user: secrets", (root / "output/report.md").read_text())
            self.assertEqual(json.loads((root / "output/summary.json").read_text()), result)
            self.assertEqual(json.loads((root / "output/report.json").read_text()), result)

    def test_native_roles_prices_and_public_report_roundtrip(self):
        with tempfile.TemporaryDirectory(prefix="20261022-0805-take-over-5-sessions-w158-battery-") as folder:
            root = Path(folder)
            entries = []
            for i, model in enumerate(("claude-opus-5-5", "claude-sonnet-5-5")):
                native = root / f"native-{i}.jsonl"
                native.write_text(json.dumps({"type": "assistant", "message": {"id": "m", "model": model,
                    "usage": {"input_tokens": 10, "cache_creation_input_tokens": 20,
                              "cache_read_input_tokens": 30, "output_tokens": 4,
                              "cache_creation": {"ephemeral_1h_input_tokens": 20}}}}))
                entries.append({"entry": {"provider": "claude", "event": {
                    "type": "sessionStarted", "file": str(native), "model": "opus"}}})
            roles = battery.capture_roles(entries + entries)
            self.assertEqual(set(roles), {"parent", "successor"})
            self.assertEqual(roles["successor"]["models"], ["claude-sonnet-5-5"])
            self.assertEqual(roles["parent"]["uncached_input"], 30)
            self.assertEqual(roles["parent"]["cache_read"], 30)
            self.assertEqual(roles["parent"]["cache_hit_ratio"], .5)
            self.assertEqual(roles["parent"]["output"], 4)
            result = {"roles": roles, "provider": "claude", "plain_events": ["private context"]}
            markdown = "\n".join(battery.economics_markdown(result))
            battery.write_report(result, markdown, root / "report.md")
            saved = json.loads((root / "report.json").read_text())
            self.assertAlmostEqual(saved["list_price_usd"], .000429)
            self.assertNotIn("private context", json.dumps(saved))
            self.assertIn("Cache hit ratio", markdown)
            self.assertEqual(nightly.metrics(result)["roles"], roles)
            missing = root / "missing.jsonl"
            missing.write_text(json.dumps({"type": "assistant", "message": {"id": "m", "model": "no-price",
                                          "usage": {"input_tokens": 10}}}))
            entries[1]["entry"]["event"]["file"] = str(missing)
            result["roles"] = battery.capture_roles(entries)
            self.assertIsNone(nightly.metrics(result)["list_price_usd"])
            self.assertIn("n/a", "\n".join(battery.economics_markdown(result)))
            missing.unlink()
            result["roles"] = battery.capture_roles(entries)
            self.assertFalse(result["roles"]["successor"]["usage_available"])
            self.assertIsNone(nightly.metrics(result)["list_price_usd"])

    def test_list_price_aggregates_preserve_reported_cost_regressions(self):
        roles = {"parent": battery.historical({"input": 100}, ["claude-opus-5-5"])}
        summary = {"run_url": "fixture", "source": "fixture", "regressions": [], "providers": [{
            "provider": "claude", "status": "complete", "arms": [
                {"block": "recall", "profile": "short", "arm": arm, "status": "complete",
                 "roles": roles, "list_price_usd": price, "usage": {"cost_usd": None}}
                for arm, price in (("plain", 1), ("seam", 2), ("seam", 4))]}]}
        self.assertEqual(nightly.regressions(summary), [])
        text = nightly.markdown(summary)
        seam = summary["aggregate"][1]
        self.assertEqual(seam["mean_list_price_usd"], 3)
        self.assertEqual(seam["list_price_ratio"], 3)
        self.assertIn("$3.000000", text)
        summary["providers"][0]["arms"][2]["list_price_usd"] = None
        nightly.prepare(summary)
        self.assertIsNone(summary["aggregate"][1]["mean_list_price_usd"])
        self.assertIsNone(summary["aggregate"][1]["list_price_ratio"])

    def test_cold_return_is_reported_but_never_a_regression(self):
        fixture = json.loads((nightly.ROOT / "apps/desktop/src/fixtures/continuity-artifact.json").read_text())
        self.assertEqual(nightly.regressions(fixture), [])
        fixture["providers"][0]["arms"][0]["cold_return_cost_usd"] = 1000000
        self.assertEqual(nightly.regressions(fixture), [])
        fixture["providers"][0]["arms"][0]["applied"] = 8
        self.assertTrue(any("decisions-survive" in r for r in nightly.regressions(fixture)))

    def test_regression_compares_matching_baseline_and_prices_need_both_arms(self):
        fixture = json.loads((nightly.ROOT / "apps/desktop/src/fixtures/continuity-artifact.json").read_text())
        previous = json.loads(json.dumps(fixture))
        fixture["providers"][0]["arms"][0]["recall"] = .8
        self.assertTrue(any("below baseline" in r for r in nightly.regressions(fixture, previous)))
        fixture["providers"][0]["arms"][0]["usage"]["cost_usd"] = None
        fixture["providers"][0]["arms"][0].pop("price_weighted_ratio", None)
        nightly.regressions(fixture)
        self.assertNotIn("price_weighted_ratio", fixture["providers"][0]["arms"][0])

    def test_missing_prices_and_unobserved_cold_returns_are_not_zero(self):
        self.assertIsNone(nightly.metrics({})["usage"]["cost_usd"])
        self.assertIsNone(nightly.metrics({"usage":[{"input":30,"cached_input":20}]})["usage"]["cost_usd"])
        self.assertIsNone(nightly.metrics({})["cold_return_cost_usd"])

    def test_opted_in_native_lineages_count_compactions_once_and_keep_session_ids(self):
        with tempfile.TemporaryDirectory(prefix="20261022-0805-take-over-5-sessions-w117-lineages-") as folder:
            root = Path(folder)
            record = root / "record"
            record.mkdir()
            entries = []
            for provider, rows in (("claude", [{"type":"system","subtype":"compact_boundary"},{"type":"assistant"}]),
                                   ("codex", [{"type":"compacted"},{"type":"event_msg","payload":{"type":"context_compacted"}}])):
                native = root / (provider + ".jsonl")
                native.write_text("\n".join(map(json.dumps, rows)))
                entries.append({"provider":provider,"sessionId":provider + "-id","file":str(native),"startedAt":1})
            (record / "lineage.json").write_text(json.dumps(entries))
            result = lineages.sample([str(record)], "2026-W43")
            self.assertEqual([(r["session_id"],r["compactions"]) for r in result["sessions"]], [("claude-id",1),("codex-id",1)])
            self.assertNotIn("payload", json.dumps(result))
            self.assertNotIn(str(root), json.dumps(result))
            self.assertEqual(lineages.sample([])["status"], "skipped")
            self.assertTrue(lineages.sample([str(root)])["errors"])

    def test_plain_cli_resumes_its_native_session_without_forcing_compaction(self):
        for provider in ("claude", "codex"):
            run = object.__new__(PlainBattery)
            run.args = SimpleNamespace(provider=provider, model="fixture-model", turn_timeout=1)
            run.repo, run.deadline = Path("/tmp"), time.monotonic() + 10
            run.steps, run.errors, run.processes, run.raw_events = [], [], [], []
            run.result, run.native_id = {}, None
            events = ([{"type":"system","session_id":"fixture-id"}, {"type":"result","result":"answer", "usage":{"input_tokens":50,"cache_read_input_tokens":60},"total_cost_usd":1.25}]
                      if provider == "claude" else [{"type":"thread.started","thread_id":"fixture-id"},
                      {"type":"item.completed","item":{"type":"agent_message","text":"answer"}},
                      {"type":"turn.completed","usage":{"input_tokens":110,"cached_input_tokens":60}}])
            fake = SimpleNamespace(returncode=0, communicate=lambda *a, **k: ("\n".join(map(json.dumps,events)), ""))
            with patch("plain.subprocess.Popen", return_value=fake) as launch:
                self.assertEqual(run.send("first","prompt")["answer"], "answer")
                self.assertEqual(run.send("next","prompt")["answer"], "answer")
                command = launch.call_args.args[0]
                self.assertIn("fixture-id", command)
                self.assertNotIn("--autocompact", command)
                self.assertNotIn("model_auto_compact_token_limit", " ".join(command))
            self.assertEqual(run.result["usage"][1]["input"], 50)
            self.assertEqual(run.result["usage"][1]["cached_input"], 60)

    def test_regression_opens_issue_with_exact_report_body(self):
        fixture = json.loads((nightly.ROOT / "apps/desktop/src/fixtures/continuity-artifact.json").read_text())
        fixture["regressions"] = ["recall below baseline"]
        seen = []
        def gh(command, **kwargs):
            if command[1:3] == ["issue", "create"]:
                seen.append(Path(command[command.index("--body-file") + 1]).read_text())
            return SimpleNamespace(stdout="[]")
        with patch("nightly.subprocess.run", side_effect=gh) as call:
            nightly.issue(fixture)
            self.assertEqual(call.call_count, 2)
            self.assertIn("create", call.call_args.args[0])
        self.assertIn(fixture["run_url"], seen[0])
        self.assertIn("recall below baseline", seen[0])

    def test_decision_block_uses_same_script_over_plain_transport(self):
        class PlainDecisions(decisions.Decisions, PlainBattery):
            pass
        self.assertIs(PlainDecisions.run, decisions.Decisions.run)
        self.assertIs(PlainDecisions.send, PlainBattery.send)
        self.assertIs(PlainDecisions.fixture, battery.Battery.fixture)


if __name__ == "__main__":
    unittest.main()
