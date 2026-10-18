#!/usr/bin/env python3
"""tripwire.py on a fixture data dir: its report matches the expected text.

usage: python3 -I tools/ab/checks/tripwire_test.py"""
import json, os, sqlite3, subprocess, sys, tempfile, unittest

SCRIPT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "tripwire.py")
T = 1_760_000_000_000

EXPECTED = """\
Tokens: 121,280, 1.35× the last ship's 90,000: over 1.25×, so the phase report needs one line saying what bought it.
Cache-hit share: 47.4% (57,000 of 120,180 prompt tokens read from cache).
Opening-call cache writes and misses by cause:
  fresh start: 2 sessions, wrote 60,000, missed 0
  resume: 1 session, wrote 1,000, missed 5,000
Unattributed: wrote 2,000, missed 1,500; unestimated calls 2.
Invalid tool calls: 3 (stop_worker 1, submit_report 2).
Brain and upkeep rows by tier:
  brain on claude-sonnet-5-5 (strong): 1 row, 1,000 tokens: above standard
  brain on new-model (frontier): 1 row, 10 tokens: above standard
  upkeep on claude-haiku-5-5 (light): 1 row, 250 tokens
  upkeep on haiku (light): 1 row, 250 tokens
"""


def build(data):
    db = sqlite3.connect(os.path.join(data, "routing.sqlite"))
    db.execute("CREATE TABLE turn_usage (at_ms INTEGER, model TEXT, conversation_id TEXT, task_id TEXT, step TEXT, "
               "input INTEGER, cached_input INTEGER, cache_write INTEGER, output INTEGER, missed INTEGER, "
               "unestimated_calls INTEGER, cause TEXT, cause_write INTEGER, cause_missed INTEGER)")
    db.executemany("INSERT INTO turn_usage VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", [
        (T + 1, "opus", "c1", None, "thread", 100, 0, 40_000, 500, 0, 1, "fresh start", 40_000, 0),
        (T + 2, "opus", "c1", None, "thread", 50, 39_000, 2_000, 300, 1_500, 0, None, None, None),
        (T + 3, "sonnet", "c1", "t1", "worker", 10, 0, 20_000, 200, 0, 1, "fresh start", 20_000, 0),
        (T + 4, "sonnet", "c1", "t1", "worker", 20, 18_000, 1_000, 100, 5_000, 0, "resume", 1_000, 5_000),
        (T + 5_000, "opus", "c1", None, "thread", 9, 9, 9, 9, 9, 9, "rebirth", 9, 9),  # after the window
        (T + 5, "opus", "c2", None, "thread", 7, 7, 7, 7, 7, 7, "rebirth", 7, 7),  # another tab
        (T + 6, "claude-sonnet-5-5", None, None, "brain", 100, 800, 0, 100, None, None, None, None, None),
        (T + 7, "new-model", None, None, "brain", 10, 0, 0, 0, None, None, None, None, None),
        (T + 8, "claude-haiku-5-5", None, None, "upkeep", 200, 0, 0, 50, None, None, None, None, None),
        (T + 9, "haiku", None, None, "upkeep", 200, 0, 0, 50, None, None, None, None, None),
    ])
    db.execute("CREATE TABLE bad_tool_calls (at_ms INTEGER, conversation_id TEXT, task_id TEXT, tool TEXT)")
    db.executemany("INSERT INTO bad_tool_calls VALUES (?, ?, ?, ?)", [
        (T + 1, "c1", None, "stop_worker"), (T + 3, "c1", "t1", "submit_report"),
        (T + 4, "c1", "t1", "submit_report"), (T + 4, "c2", None, "stop_worker")])
    db.commit()
    db.close()
    registry = os.path.join(data, "cache", "registry")
    os.makedirs(registry)
    with open(os.path.join(registry, "models.json"), "w") as f:
        # A download newer than the bundled registry, which alone knows new-model.
        json.dump({"revision": 10_000, "models": [
            {"key": "claude-sonnet-5-5", "match": {"ids": ["sonnet", "claude-sonnet-5-5"]}, "tier": "strong"},
            {"key": "claude-haiku-5-5", "match": {"ids": ["haiku", "claude-haiku-5-5"]}, "tier": "light"},
            {"key": "new-model", "match": {"ids": ["new-model"]}, "tier": "frontier"}]}, f)


class Tripwire(unittest.TestCase):
    def run_on(self, data, *extra):
        run = subprocess.run([sys.executable, "-I", SCRIPT, data, "c1", str(T), str(T + 1_000), *extra],
                             capture_output=True, text=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        return run.stdout

    def test_report_over_the_tripwire(self):
        with tempfile.TemporaryDirectory(prefix="tripwire-") as data:
            build(data)
            self.maxDiff = None
            self.assertEqual(self.run_on(data, "--last", "90000"), EXPECTED)

    def test_under_the_tripwire_no_line_is_owed(self):
        with tempfile.TemporaryDirectory(prefix="tripwire-") as data:
            build(data)
            first = self.run_on(data, "--last", "100000").splitlines()[0]
            self.assertEqual(first, "Tokens: 121,280, 1.21× the last ship's 100,000.")

    def test_with_no_download_the_bundled_registry_names_the_tiers(self):
        with tempfile.TemporaryDirectory(prefix="tripwire-") as data:
            build(data)
            os.remove(os.path.join(data, "cache", "registry", "models.json"))
            lines = self.run_on(data).splitlines()
            self.assertIn("  brain on claude-sonnet-5-5 (strong): 1 row, 1,000 tokens: above standard", lines)
            self.assertIn("  brain on new-model (unknown): 1 row, 10 tokens", lines)

    def test_rows_still_in_the_wal_of_a_read_only_copy_count(self):
        with tempfile.TemporaryDirectory(prefix="tripwire-") as root:
            live = os.path.join(root, "live")
            os.makedirs(live)
            build(live)
            db = sqlite3.connect(os.path.join(live, "routing.sqlite"))
            db.execute("PRAGMA journal_mode=WAL")
            db.execute("PRAGMA wal_autocheckpoint=0")
            db.execute("INSERT INTO bad_tool_calls VALUES (?, 'c1', NULL, 'merge')", (T + 9,))
            db.commit()
            # A snapshot taken while the daemon ran: the database and its WAL, no -shm, in a
            # folder nothing may write to.
            snap = os.path.join(root, "snap")
            os.makedirs(snap)
            for name in ("routing.sqlite", "routing.sqlite-wal"):
                with open(os.path.join(live, name), "rb") as src, open(os.path.join(snap, name), "wb") as dst:
                    dst.write(src.read())
            db.close()
            os.chmod(snap, 0o555)
            try:
                lines = self.run_on(snap).splitlines()
            finally:
                os.chmod(snap, 0o755)
            self.assertIn("Invalid tool calls: 4 (merge 1, stop_worker 1, submit_report 2).", lines)


if __name__ == "__main__":
    unittest.main()
