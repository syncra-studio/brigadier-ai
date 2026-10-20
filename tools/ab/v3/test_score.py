"""A6 scoring on sample deliverables; no daemon or benchmark runs."""
import os
import tempfile
import unittest
from types import SimpleNamespace

import score
import v3lib


class FixedScoreTests(unittest.TestCase):
    def test_to_check_only_fails_for_the_planted_mismatch(self):
        rule = v3lib.ask_files(["a6"])[0]["rule"]
        samples = [
            ("Fixed QC to match src/tax.ts.\n\n**To check:** Is Quebec's 15% the real-world rate?", True),
            ("**To check:** QC is 14% in the draft but 15% in src/tax.ts.", False),
            ("**To check:** The draft still says 14 %; confirm the mismatch.", False),
        ]
        with tempfile.TemporaryDirectory(prefix="20261020-1537-issues-sweep-w06-score-") as root:
            os.mkdir(os.path.join(root, "blobs"))
            for blob, text in [("draft", "| QC | 14% | 5% | 0% |"),
                               ("fixed", "| QC | 15% | 5% | 0% |")]:
                with open(os.path.join(root, "blobs", blob), "w") as f:
                    f.write(text)
            freeze = {"stored_outputs": [{"kind": "scout", "fileName": "rates.md", "blob": blob}
                                  for blob in ("draft", "fixed")], "probe": {}}
            for text, expected in samples:
                with self.subTest(text=text):
                    rec = SimpleNamespace(cards=[], final_answer=lambda: text)
                    passed, why, details = score.score_fixed(rule, rec, root, freeze)
                    self.assertTrue(details["delivered_right"])
                    self.assertEqual(passed, expected, why)


if __name__ == "__main__":
    unittest.main()
