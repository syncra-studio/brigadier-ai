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
