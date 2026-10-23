"""D49 deterministic budget/scenario/report checks; no daemon or model calls."""
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import budgets
import decisions
import nightly

PREFIX = "20261022-0805-take-over-5-sessions-w167-test-"


class BudgetTests(unittest.TestCase):
    def test_user_selection_keeps_first_latest_whole_utf8_and_old_rule_at_80kb(self):
        messages = ["first", "RULE-ONLY-HERE"] + ["é" * 1750 for _ in range(16)] + ["latest"]
        old = budgets.retained_users(messages, 32_000)
        new = budgets.retained_users(messages, 80_000)
        self.assertNotIn(messages[1], old)
        self.assertIn(messages[1], new)
        self.assertEqual(new, messages)
        self.assertEqual(budgets.retained_users(["x" * 90_000, "last"], 80_000), ["x" * 90_000, "last"])
        self.assertEqual(budgets.retained_users([], 80_000), [])

    def p2_result(self):
        details = {f"detail_{n}": f"SECRET-{n}" for n in range(5)}
        tail = "".join(f"[#{i} · tool]\n" + "x" * 4500 + "\n\n" for i in range(30, 14, -1))
        tail += "".join(f"[#{i} · tool]\n{v}\n\n" for i, v in enumerate(details.values()))
        return {"details": details, "request_context": 80_000, "request_seam": 1,
                "detail_contexts": {k: 45_000 for k in details},
                "detail_answer": json.dumps(details), "detail_probe_ms": 100, "detail_probe_end_ms": 200,
                "handoffs": [{"n": 1, "start": "## Earlier, newest first\n" + tail}], "provider_events": [],
                "seam_file": {"seams": [{"n": 1, "provider": "codex", "movedTo": "codex"}]},
                "rows": {str(i): {"recalled": True, "unprompted": True if i < 9 else None} for i in range(10)}}

    def test_p2_qualifies_five_details_beyond_old_tail_and_requires_no_lookups(self):
        result = self.p2_result()
        scored = budgets.score_p2(result)
        self.assertTrue(scored["bar"])
        self.assertEqual(scored["recalled"], 5)
        self.assertEqual(scored["legacy_retained"], 0)
        event = {"atMs": 150, "entry": {"event": {"type": "command", "status": "completed", "command": "brigadier brain read x 1 2"}}}
        result["provider_events"] = [event]
        self.assertFalse(budgets.score_p2(result)["qualified"])
        event["atMs"] = 250  # A later T6 lookup is allowed; not part of no-read probe.
        self.assertTrue(budgets.score_p2(result)["qualified"])
        for field, value in (("ledger", "SECRET-0"), ("request_context", 59_999), ("request_context", 100_001)):
            bad = dict(result, **{field: value})
            self.assertFalse(budgets.score_p2(bad)["qualified"])
        result["detail_contexts"]["detail_0"] = 39_999
        self.assertFalse(budgets.score_p2(result)["qualified"])

    def test_legacy_comparison_includes_latest_tools_with_the_old_4kb_cap(self):
        start = "## The latest exchange, after #1\n[#2 · command]\nLATEST-DETAIL\n" + "x" * 5000 + "TOO-LATE" + "\n## Earlier, newest first\n[#0 · assistant]\nOLDER"
        old = budgets.legacy_context(start)
        self.assertIn("LATEST-DETAIL", old)
        self.assertIn("OLDER", old)
        self.assertNotIn("TOO-LATE", old)
        result = self.p2_result()
        result["rows"]["0"]["unprompted"] = False
        self.assertTrue(budgets.score_p2(result)["detail_bar"])
        self.assertFalse(budgets.score_p2(result)["bar"])

    def test_inline_jsdocs_do_not_consume_the_next_functions_comment(self):
        text = """/** Since 0.6: First. */
export function one() {}
/** Since 0.6: Second. */
export function two() {}
/**
 * Since 0.6: Third.
 * More words.
 */
export function three() {}
"""
        self.assertEqual(decisions.jsdoc_first_lines(text), ["Since 0.6: First.", "Since 0.6: Second.", "Since 0.6: Third."])

    def test_p2_missing_start_or_answer_never_passes(self):
        result = self.p2_result()
        result["handoffs"] = []
        self.assertFalse(budgets.score_p2(result)["qualified"])
        result = self.p2_result()
        result["detail_answer"] = '{}'
        self.assertFalse(budgets.score_p2(result)["bar"])

    def test_p3_three_seams_older_rule_applied_nine_of_ten_within_delta(self):
        rule = "RULE-ONLY-HERE"
        messages = ["first", rule] + ["x" * 3500 for _ in range(16)] + ["latest"]
        result = {"user_rule": rule, "user_inputs": messages,
                  "receipts": [{"auditTag": rule}] * 9 + [{}], "seams_before_checks": 3,
                  "handoffs": [{"start": "## The user's messages\n" + rule + "\n## The latest exchange"}] * 3}
        scored = budgets.score_p3(result)
        self.assertTrue(scored["bar"])
        self.assertEqual(scored["applied"], 9)
        self.assertLessEqual(scored["extra_user_tokens_estimate_per_seam"], 12_000)
        for field, value in (("ledger", rule), ("seams_before_checks", 2), ("receipts", [{}] * 10)):
            self.assertFalse(budgets.score_p3(dict(result, **{field: value}))["bar"])
        self.assertFalse(budgets.score_p3({})["bar"])

    def test_native_first_input_prices_codex_uncached_input_and_claude_cache_ttl(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            file = Path(folder) / "native.jsonl"
            file.write_text(json.dumps({"type": "turn_context", "payload": {"model": "gpt-6.1-sol"}}) + "\n" +
                            json.dumps({"type": "event_msg", "payload": {"type": "token_count", "info": {
                                "last_token_usage": {"input_tokens": 1000, "cached_input_tokens": 100}}}}))
            value = budgets.first_input_cost(str(file), "codex")
            self.assertEqual(value["input"], 900)
            self.assertEqual(value["cache_write"], 0)
            self.assertIsNotNone(value["list_price_usd"])
            file.write_text(json.dumps({"type": "assistant", "message": {"model": "claude-opus-5-5", "usage": {
                "input_tokens": 10, "cache_creation_input_tokens": 1000,
                "cache_creation": {"ephemeral_1h_input_tokens": 1000}}}}))
            value = budgets.first_input_cost(str(file), "claude")
            self.assertEqual(value["cache_write"], 1000)
            self.assertAlmostEqual(value["list_price_usd"], .00804)
            self.assertIsNone(budgets.first_input_cost(None, "codex")["list_price_usd"])

    def test_p2_transport_moves_once_with_rules_selected_target_and_then_common_checks(self):
        run = object.__new__(budgets.Budgets)
        run.args = SimpleNamespace(max_fillers=1, target_model="gpt-6.1-sol")
        run.result = {}
        run.conversation = "conv"
        class Ipc:
            def __init__(self): self.calls = []
            def call(self, body): self.calls.append(body)
        run.ipc = Ipc()
        run.generations = lambda: 0
        run.context = lambda: 60_000
        run.read_batch = lambda *args, **kwargs: None
        run.send = lambda *args: {"answer": "noted", "ended_ms": 200}
        run.wait = lambda *args: True
        checks = []
        run.checks = lambda: checks.append(True)
        run.run_p2()
        self.assertEqual(len(run.ipc.calls), 1)
        self.assertEqual(run.ipc.calls[0]["method"], "continueOn")
        self.assertEqual(run.ipc.calls[0]["model"]["provider"], "codex")
        self.assertEqual(checks, [True])
        self.assertEqual(len(run.result["details"]), 5)

    def test_p3_scenario_delivers_old_rule_padding_three_seams_and_ten_receipts(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
            run = object.__new__(budgets.Budgets)
            run.repo = Path(folder)
            run.result = {}
            sent, seams = [], []
            def send(name, text):
                sent.append((name, text))
                if name == "apply-user-rule":
                    (run.repo / "receipts.json").write_text(json.dumps([{"id": i, "auditTag": run.result["user_rule"]} for i in range(10)]))
                return {"answer": "done"}
            run.send = send
            run.force_size = lambda n: seams.append(n)
            run.generations = lambda: 3
            run.run_p3()
            self.assertEqual(seams, [0, 1, 2])
            self.assertGreater(run.result["user_bytes"], 32_000)
            self.assertLess(run.result["user_bytes"], 80_000)
            self.assertEqual(len(run.result["receipts"]), 10)
            self.assertEqual(sum(run.result["user_rule"] in text for _, text in sent), 1)
            self.assertEqual(list(run.repo.iterdir()), [run.repo / "receipts.json"])

    def test_nightly_runs_budget_variants_once_on_claude_seam_short_only(self):
        for provider, arm, expected in (("claude", "seam", {"p2", "p3"}), ("claude", "plain", set()),
                                        ("codex", "seam", set())):
            with tempfile.TemporaryDirectory(prefix=PREFIX) as folder:
                with patch("nightly.subprocess.Popen", side_effect=OSError("offline")) as spawn:
                    nightly.provider_report(provider, Path(folder), "/missing/daemon", True, arm)
                rows = json.loads((Path(folder) / f"{provider}-{arm}.json").read_text())["arms"]
                budget_rows = [r for r in rows if r["block"] in ("p2", "p3")]
                self.assertEqual({r["block"] for r in budget_rows}, expected)
                self.assertTrue(all(r["profile"] == "short" and r["arm"] == "seam" for r in budget_rows))
                budget_commands = [c.args[0] for c in spawn.call_args_list if "budgets.py" in " ".join(c.args[0])]
                self.assertEqual(len(budget_commands), len(expected))

    def test_budget_bars_are_report_only(self):
        row = {"block": "p2", "profile": "short", "arm": "seam", "status": "complete", "recall": 0,
               "task_passed": False, "compactions": 1, "budget_scores": {"bar": False}}
        summary = {"providers": [{"provider": "claude", "arms": [row]}]}
        self.assertEqual(nightly.regressions(summary), [])
        self.assertEqual(nightly.metrics({"budget_scores": {"bar": False}})["budget_scores"], {"bar": False})


if __name__ == "__main__":
    unittest.main()
