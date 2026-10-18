#!/usr/bin/env python3
"""The bench readers' typed-record guard (#190): a recording that also holds the engine's own
records a feed from before #189 sent (no `type`: a request's file record, a background run's
stored output) reads exactly as the same recording without them. Every reader of
`rec/events.jsonl` under tools/ab carries the guard.

usage: python3 -I tools/ab/checks/typed_records_test.py"""
import glob, json, os, re, subprocess, sys, tempfile, unittest

AB = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
T = 1_760_000_000_000
CONV = "c1"

TYPED = [
    {"type": "messageAppended", "message": {"id": "r1", "conversationId": CONV, "role": "user", "text": "Go."}},
    {"type": "requestUpdated", "request": {"id": "r1", "conversationId": CONV, "state": {"type": "working"}}},
    {"type": "orchestratorStepped", "step": {"requestId": "r1", "atMs": T + 100,
                                             "kind": {"type": "created", "taskId": "t1"}}},
    {"type": "taskUpdated", "task": {"id": "t1", "conversationId": CONV, "title": "Fix it", "kind": "implement",
                                     "state": {"type": "running"},
                                     "route": {"choice": {"provider": "claude", "model": "opus"}}}},
    {"type": "workerEvent", "taskId": "t1", "event": {"type": "sessionStarted", "nativeId": "s1", "model": "opus"}},
    {"type": "workerEvent", "taskId": "t1", "event": {"type": "command", "itemId": "i1", "command": "cargo test"}},
    {"type": "workerEvent", "taskId": "t1", "event": {"type": "turnCompleted", "usage": {
        "inputTokens": 10, "cachedInputTokens": 900, "cacheWriteTokens": 50, "outputTokens": 40}}},
    {"type": "checkRan", "taskId": "t1", "tree": "abc", "command": "cargo test", "status": "passed",
     "durationMs": 1200},
    {"type": "brainJobUpdated", "job": {"id": "j1", "kind": "enrich", "state": {"type": "done"}}},
    {"type": "requestUpdated", "request": {"id": "r1", "conversationId": CONV, "state": {"type": "done"}}},
]

# What a daemon from before #189 also sent on the feed: no `type` to switch on.
UNTYPED = [
    {"stream": "changes:r1", "event": {"changes": [{"path": "src/a.rs", "added": 3, "removed": 1}]}},
    {"stream": "background-output:b1:1", "event": "line one\nline two\n"},
    {"stream": "pending:c1", "event": {"files": {}, "absorbed": [], "reviews": {}}},
]


def write_arm(arm, untyped):
    os.makedirs(os.path.join(arm, "rec"))
    with open(os.path.join(arm, "start.json"), "w") as f:
        json.dump({"t0_ms": T, "conversation": CONV}, f)
    with open(os.path.join(arm, "rec", "events.jsonl"), "w") as f:
        seq = 0
        for i, event in enumerate(TYPED):
            seq += 1
            f.write(json.dumps({"seq": seq, "stream": f"conversation:{CONV}", "atMs": T + 100 * (i + 1),
                                "event": event}) + "\n")
            if untyped and i < len(UNTYPED):
                seq += 1
                f.write(json.dumps({"seq": seq, "atMs": T + 100 * (i + 1) + 50, **UNTYPED[i]}) + "\n")


READERS = [
    ["timeline.py", "{arm}", "--all"],
    ["evtokens.py", "{arm}"],
    ["firstevent.py", "{arm}", "--json"],
    ["checkruns.py", "{arm}", "--json"],
    ["brainwait.py", "{arm}", "--max", "1"],
]


def run(reader, arm):
    args = [a.format(arm=arm) for a in reader[1:]]
    out = subprocess.run([sys.executable, "-I", os.path.join(AB, reader[0]), *args], capture_output=True,
                         text=True, timeout=60)
    # brainwait stamps each line with the time.
    text = re.sub(r"^\d\d:\d\d:\d\d ", "", out.stdout, flags=re.M)
    return out.returncode, text, out.stderr


class TypedRecords(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="typed-records-")
        cls.clean = os.path.join(cls.tmp.name, "clean")
        cls.mixed = os.path.join(cls.tmp.name, "mixed")
        write_arm(cls.clean, untyped=False)
        write_arm(cls.mixed, untyped=True)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_each_reader_reads_the_same_with_untyped_records_in(self):
        for reader in READERS:
            with self.subTest(reader=reader[0]):
                clean, mixed = run(reader, self.clean), run(reader, self.mixed)
                self.assertEqual(clean[0], 0, clean[2])
                self.assertTrue(clean[1].strip(), f"{reader[0]} printed nothing")
                self.assertEqual(mixed, clean)

    def test_reqdone_sees_the_request_end(self):
        for arm in (self.clean, self.mixed):
            out = subprocess.run(["bash", os.path.join(AB, "reqdone.sh"), arm], capture_output=True, text=True,
                                 timeout=60)
            self.assertEqual((out.returncode, out.stdout.strip()), (0, "request ended"), out.stderr)

    def test_every_reader_of_a_recording_carries_the_guard(self):
        guard = re.compile(r'"type" not in|"type" in e|domain_event|read_events')
        unguarded = []
        for path in sorted(glob.glob(os.path.join(AB, "**", "*.py"), recursive=True)
                           + glob.glob(os.path.join(AB, "**", "*.sh"), recursive=True)):
            if os.sep + "checks" + os.sep in path:
                continue
            with open(path) as f:
                text = f.read()
            reads = "events.jsonl" in text and re.search(r"json\.loads\(l(ine)?\)", text)
            if reads and not guard.search(text):
                unguarded.append(os.path.relpath(path, AB))
        self.assertEqual(unguarded, [])


if __name__ == "__main__":
    unittest.main()
