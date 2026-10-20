"""Ask snapshots at command boundaries, with every benchmark attempt stubbed out."""
import os
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import run


class AskSnapshotTests(unittest.TestCase):
    def test_set_and_ab_keep_the_initial_asks_between_reruns(self):
        for command in (run.cmd_set, run.cmd_ab):
            with self.subTest(command=command.__name__):
                with tempfile.TemporaryDirectory(prefix="20261020-1537-issues-sweep-w06-asks-") as root:
                    os.mkdir(os.path.join(root, "asks"))
                    path = os.path.join(root, "asks", "sample.md")

                    def write_ask(text):
                        with open(path, "w") as f:
                            f.write('id: sample\n## Request\n```text\n' + text +
                                    '\n```\n## Scoring\n```json\n{"rule": "answer"}\n```\n')

                    write_ask("original wording")
                    seen = []

                    def attempt(_root, ask, *_args):
                        seen.append(ask["requests"][0])
                        write_ask("edited during the set")

                    args = SimpleNamespace(root=root, roots=f"A={root},B={root}", asks="sample",
                                           reruns=2, first=3, regime="warm", brain="empty", tag="test")
                    with patch.object(run.v3lib, "HERE", root), \
                         patch.object(run, "check_root", side_effect=lambda p: p), \
                         patch.object(run, "attempt", side_effect=attempt):
                        command(args)
                    self.assertEqual(seen, ["original wording"] * (2 if command == run.cmd_set else 4))


if __name__ == "__main__":
    unittest.main()
