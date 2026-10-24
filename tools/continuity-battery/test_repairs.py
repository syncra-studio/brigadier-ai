"""PROPOSALS 3A regressions; all fixtures are local, no model calls."""
import json
from pathlib import Path
import tempfile
import unittest

import battery
import decisions
import nightly
from scoring import definite_rejection

PREFIX = "20261024-1334-long-session-memory-w11-test-"


class ScoringRepairs(unittest.TestCase):
    def test_fix6_visibility_is_not_recall(self):
        result = {"items": {"tool_only": {"verdict": "pass"},
                            "large_failure": {"verdict": "fail"}}}
        self.assertEqual(nightly.metrics(result)["recall"], 1)
        self.assertNotIn("recall.large_failure", nightly.decision_checks(result))

    def test_fix7_requires_definite_rejection_and_all_plants_in_rubric(self):
        for vague in ("unverified writeFileSync", "not false: writeFileSync", "maybe false", "unknown"):
            self.assertFalse(definite_rejection(vague), vague)
        self.assertTrue(definite_rejection("Invalid: calls writeFileSync directly"))
        for item in decisions.ITEMS:
            self.assertIn("- " + item + ":", decisions.RUBRIC)

    def test_fix8_native_fragments_dedup_and_synthetic_is_explicit(self):
        with tempfile.TemporaryDirectory(prefix=PREFIX) as directory:
            file = Path(directory) / "native.jsonl"
            rows = [{"type": "assistant", "message": {"id": "one", "model": "claude-opus-5-5",
                     "usage": {"input_tokens": 10, "output_tokens": output}}} for output in (1, 4)]
            file.write_text("\n".join(map(json.dumps, rows)))
            entries = [{"entry": {"provider": "claude", "event": {
                "type": "sessionStarted", "file": str(file)}}}]
            roles = battery.capture_roles(entries * 2)
            self.assertEqual(roles["parent"]["usage"]["input"], 10)
            self.assertEqual(roles["parent"]["usage"]["output"], 4)
            rows.append({"type": "assistant", "message": {"id": "two", "model": "<synthetic>",
                         "usage": {"input_tokens": 50}}})
            file.write_text("\n".join(map(json.dumps, rows)))
            roles = battery.capture_roles(entries)
            self.assertIsNone(roles["parent"]["list_price_usd"])
            self.assertTrue(any("Synthetic successor" in n for n in roles["parent"]["notes"]))

    def test_fix9_codex_native_compaction_and_fault_class_are_visible(self):
        result = {"provider": "codex", "arm": "seam", "compactions": 3,
                  "seams": [{"kind": "context", "at_ms": 1}]}
        battery.prepare_economics(result)
        metrics = nightly.metrics(result)
        self.assertTrue(metrics["native_compaction_in_seam"])
        self.assertEqual(metrics["run_kind"], "fault-recovery")
        summary = {"providers": [{"provider": "codex", "arms": [{**metrics,
                   "status": "complete", "arm": "seam", "block": "recall", "profile": "long"}]}]}
        self.assertIn("native compaction observed", nightly.regressions(summary)[0])


if __name__ == "__main__":
    unittest.main()


class PrivateEvidenceRepair(unittest.TestCase):
    def test_fix3_private_bundle_survives_cleanup_expires_and_never_exports(self):
        from types import SimpleNamespace
        import evidence
        with tempfile.TemporaryDirectory(prefix=PREFIX) as scratch:
            root = Path(scratch)
            owned = root / "scratch"
            owned.mkdir()
            repo = owned / "repo"
            repo.mkdir()
            (repo / "checkpoint.json").write_text('{"private":"checkpoint"}')
            native = owned / "native.jsonl"
            native.write_text('{"private":"transcript"}\n')
            (owned / "daemon.log").write_text("private daemon event")
            run = SimpleNamespace(root=owned, repo=repo, record=lambda: repo, native_files=[str(native)],
                                  args=SimpleNamespace(provider="claude"), result={"steps": ["private answer"]})
            store = root / "private-store"
            directory = evidence.retain(run, store=store)
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
            self.assertEqual((directory / "evidence.json").stat().st_mode & 0o777, 0o600)
            self.assertTrue((directory / "record/checkpoint.json").is_file())
            native.unlink()
            self.assertIn("transcript", (directory / "native-0.jsonl").read_text())
            manifest = json.loads((directory / "manifest.json").read_text())
            self.assertEqual(manifest["expires_at"] - manifest["created_at"], 7 * 86400)
            self.assertEqual(evidence.purge(store, manifest["expires_at"] - 1), [])
            public = root / "public.md"
            battery.write_report(run.result, "safe report", public)
            self.assertNotIn("private", public.with_suffix(".json").read_text())
            foreign = store / "evidence-not-ours"
            foreign.mkdir()
            self.assertEqual(evidence.purge(store, manifest["expires_at"]), [directory.name])
            self.assertTrue(foreign.exists())

    def test_fix3_lookup_hits_and_ttl_calls_are_retained(self):
        import evidence
        rows = [{"type": "assistant", "message": {"id": "a", "model": "fixture",
                 "usage": {"input_tokens": 20, "cache_creation": {"ephemeral_1h_input_tokens": 10}},
                 "content": [{"type": "tool_use", "id": "lookup", "input": {
                     "command": "brigadier brain read --id xyz"}}]}},
                {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "lookup",
                                                               "content": "needed FACT-123"}]}}]
        self.assertEqual(evidence.calls(rows * 2, "claude")[0]["usage"]["cache_creation"]["ephemeral_1h_input_tokens"], 10)
        self.assertEqual(len(evidence.calls(rows * 2, "claude")), 1)
        self.assertEqual(evidence.lookups(rows, "claude", {"fact": "FACT-123"})[0]["hits"], ["fact"])


class ReportingIsolationTests(unittest.TestCase):
    def test_fix8_legacy_ui_dollar_field_is_native_alias_and_cache_writes_visible(self):
        roles = {"parent": battery.historical({"input": 100, "cache_write": 200, "cache_read": 300,
                                               "output": 10}, ["claude-opus-5-5"])}
        result = {"roles": roles, "usage": [{"cost_usd": 1661, "cached_input": 0, "kind": "thread"}]}
        metrics = nightly.metrics(result)
        self.assertEqual(metrics["usage"]["cost_usd"], metrics["list_price_usd"])
        self.assertLess(metrics["usage"]["cost_usd"], 1)
        self.assertIsNone(metrics["cold_return_cost_usd"])
        table = "\n".join(battery.role_table(roles))
        self.assertIn("Cache write", table)
        self.assertIn("200", table)

    def test_fix9_fault_recovery_never_enters_continuity_cost_comparison(self):
        rows = [{"block": "recall", "profile": "short", "arm": arm, "status": "complete",
                 "run_kind": kind, "list_price_usd": price, "recall": 1}
                for arm, kind, price in (("seam", "fault-recovery", 10), ("plain", "continuity", 1),
                                         ("zed", "continuity", 2))]
        summary = {"providers": [{"provider": "claude", "arms": rows}]}
        self.assertEqual(nightly.regressions(summary), [])
        nightly.prepare(summary)
        seam = next(row for row in summary["aggregate"] if row["arm"] == "seam")
        self.assertIsNone(seam["list_price_ratio"])
        self.assertTrue(all(row["seam"] is None and not row["qualified"] for row in summary["seam_vs_zed"]))


class LooseRegexRegressions(unittest.TestCase):
    def test_fix7_decision_requires_padding_and_causal_constraint(self):
        from scoring import invoice_decision
        self.assertFalse(invoice_decision("Six for accounting"))
        self.assertFalse(invoice_decision("Not zero padded to 6 digits, accounting rejects shorter"))
        self.assertTrue(invoice_decision("Zero-padded to 6 digits because accounting import rejects shorter numbers"))

    def test_fix7_completed_items_and_vague_questions_never_pass(self):
        self.assertEqual(decisions.raised("VAT answered; README done"), {"open_question": False, "open_promise": False})
        self.assertEqual(decisions.raised("VAT still awaits you. I still owe the README section"),
                         {"open_question": True, "open_promise": True})
        self.assertFalse(decisions.principle('{"a":"Any question?", "b":"Anything?"}')["pass"])
        self.assertFalse(decisions.principle('{"a":"No idea", "b":"Yes maybe"}')["pass"])
        self.assertTrue(decisions.principle('{"a":"No", "b":"Yes"}')["pass"])
        self.assertIn("no dependencies", decisions.RUBRIC)
        self.assertIn("no tests", decisions.RUBRIC)
