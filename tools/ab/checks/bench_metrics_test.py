#!/usr/bin/env python3
"""bench_metrics.py on a fixture data dir and repo with known answers for all five rows. The
fixture mixes auto and manual merges, a Ready request never merged and an auto merge with no
Ready record; the first-look rate counts each request once.

usage: python3 -I tools/ab/checks/bench_metrics_test.py"""
import json, os, sqlite3, subprocess, sys, tempfile, unittest

SCRIPT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "bench_metrics.py")
T = 1_760_000_000_000
HOUR = 3_600_000
DAY = 24 * HOUR


def git(repo, *args, at=None):
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
               GIT_COMMITTER_EMAIL="t@t", GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
    if at is not None:
        env["GIT_AUTHOR_DATE"] = env["GIT_COMMITTER_DATE"] = f"@{at // 1000} +0000"
    return subprocess.run(["git", "-C", repo, *args], env=env, check=True, capture_output=True,
                          text=True).stdout.strip()


def commit(repo, path, text, message, at):
    with open(os.path.join(repo, path), "a") as f:
        f.write(text + "\n")
    git(repo, "add", path)
    git(repo, "commit", "-q", "-m", message, at=at)
    return git(repo, "rev-parse", "HEAD")


def build_repo(repo):
    os.makedirs(repo)
    git(repo, "init", "-q", "-b", "main")
    for name in ("a.txt", "b.txt", "c.txt", "d.txt"):
        commit(repo, name, "start", f"Add {name}", T - DAY)
    # m1: a fast-forward of two commits; the fix later touches only the first one's file.
    commit(repo, "a.txt", "one", "Change a", T + 3_900)
    m1 = commit(repo, "e.txt", "one", "Add e", T + 4_000)
    # m2: a squash on GitHub of a branch's two commits; its head stays off main.
    git(repo, "checkout", "-q", "-b", "b-c2")
    commit(repo, "b.txt", "two", "Change b", T + 4_500)
    head2 = commit(repo, "f.txt", "two", "Add f", T + 4_600)
    git(repo, "checkout", "-q", "main")
    git(repo, "merge", "-q", "--squash", "b-c2")
    git(repo, "commit", "-q", "-m", "Change b (#7)", at=T + 5_000)
    m2 = git(repo, "rev-parse", "HEAD")
    m3 = commit(repo, "c.txt", "three", "Change c", T + 6_000)
    commit(repo, "c.txt", "tidy", "Refactor c", T + DAY)  # touches m3's file, no fix
    commit(repo, "d.txt", "x", "Fixes #12: an unrelated file", T + DAY)  # a fix, no shared file
    commit(repo, "a.txt", "fixed", "Fix a parse slip in a", T + 2 * DAY)  # m1 escaped
    commit(repo, "b.txt", "reverted", f'Revert "Change b (#7)"\n\nThis reverts commit {m2}.', T + 3 * DAY)
    commit(repo, "c.txt", "late", "fix typo in c", T + 9 * DAY)  # past 7 days
    return m1, (m2, head2), m3


def user(conv, request, at):
    return [(f"conversation:{conv}", "message.appended", at, {"type": "messageAppended", "message": {
                "id": request, "conversationId": conv, "role": "user", "text": "Do it.", "createdAtMs": at,
                "requestId": request}}),
            (f"conversation:{conv}", "request.updated", at, {"type": "requestUpdated", "request": {
                "id": request, "conversationId": conv, "state": {"type": "working"}, "startedAtMs": at}})]


def ready(conv, request, at):
    return [(f"conversation:{conv}", "request.ready", at,
             {"type": "requestReady", "conversationId": conv, "requestId": request, "head": "h"})]


def merged(conv, at, sha, requests, auto, asked_in=None, position=99, head=None, commits=1):
    kind = {"type": "merged", "branch": f"b-{conv}", "base": "main", "commits": commits, "askedIn": asked_in,
            "head": head or sha, "mergeCommit": sha, "requests": requests}
    if auto:
        kind["auto"] = True
    return [(f"conversation:{conv}", "orchestrator.step", at,
             {"type": "orchestratorStepped", "step": {"requestId": requests[0], "kind": kind, "atMs": at,
                                                      "position": position}})]


def build_events(path, repo, m1, m2, m3):
    events = [("catalog", "project.created", T, {"type": "projectCreated", "project": {
        "id": "p1", "repos": [{"path": repo, "name": "repo"}]}})]
    for conv in ("c1", "c2", "c3", "c4"):
        events.append(("catalog", "conversation.created", T, {"type": "conversationCreated", "conversation": {
            "id": conv, "projectId": "p1"}}))
    # c1: a manual merge whose go-ahead was a user message; Ready recorded twice. First-look.
    events += user("c1", "r1", T + 1_000)
    events.append(("conversation:c1", "message.appended", T + 1_500, {"type": "messageAppended", "message": {
        "id": "a1", "conversationId": "c1", "role": "assistant", "text": "Done.", "createdAtMs": T + 1_500}}))
    events.append(("conversation:c1", "approval.updated", T + 1_600, {"type": "approvalUpdated", "approval": {
        "id": "ap1", "conversationId": "c1", "state": {"type": "allowed", "by": "policy"}}}))
    events += ready("c1", "r1", T + 2_000) + ready("c1", "r1", T + 2_100)
    events.append(("conversation:c1", "message.appended", T + 3_000, {"type": "messageAppended", "message": {
        "id": "u-go", "conversationId": "c1", "role": "user", "text": "Merge it.", "createdAtMs": T + 3_000,
        "requestId": "u-go"}}))
    events += merged("c1", T + 4_000, m1, ["r1"], auto=False, asked_in="u-go", commits=2)
    # c2: an auto merge after the user answered a card (updated twice) and kept a file.
    events += user("c2", "r2", T + 1_000) + ready("c2", "r2", T + 2_000)
    for at in (T + 2_500, T + 2_600):
        events.append(("conversation:c2", "question.updated", at, {"type": "questionUpdated", "question": {
            "id": "q1", "conversationId": "c2", "answer": "Yes", "answeredAtMs": T + 2_500}}))
    events.append(("conversation:c2", "request.updated", T + 2_700, {"type": "requestUpdated", "request": {
        "id": "r2", "conversationId": "c2", "state": {"type": "done"}, "startedAtMs": T + 1_000,
        "review": {"a.txt": "kept", "b.txt": "pending", "f.txt": "later"}}}))
    events += merged("c2", T + 5_000, m2[0], ["r2"], auto=True, head=m2[1], commits=2)
    # c3: Ready, never merged.
    events += user("c3", "r3", T + 1_000) + ready("c3", "r3", T + 2_000)
    # c4: an auto merge with no Ready record, an untyped record before it and a message after it.
    events += user("c4", "r4", T + 1_000)
    events.append(("conversation:c4", "request.updated", T + 1_200, {"changes": [{"path": "c.txt"}]}))
    events += merged("c4", T + 6_000, m3, ["r4"], auto=True)
    events += user("c4", "r5", T + 7_000)
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE events (seq INTEGER PRIMARY KEY AUTOINCREMENT, stream TEXT NOT NULL, "
               "stream_seq INTEGER NOT NULL, kind TEXT NOT NULL, at_ms INTEGER NOT NULL, payload TEXT NOT NULL)")
    for n, (stream, kind, at, payload) in enumerate(events):
        db.execute("INSERT INTO events (stream, stream_seq, kind, at_ms, payload) VALUES (?, ?, ?, ?, ?)",
                   (stream, n, kind, at, json.dumps(payload)))
    db.commit()
    db.close()


def build_routing(path):
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE turn_usage (at_ms INTEGER, provider TEXT, model TEXT, conversation_id TEXT, "
               "project_id TEXT, task_id TEXT, input INTEGER, cached_input INTEGER, cache_write INTEGER, "
               "output INTEGER, step TEXT, account TEXT NOT NULL DEFAULT '')")
    db.executemany("INSERT INTO turn_usage (at_ms, provider, model, conversation_id, project_id, task_id, input, "
                   "cached_input, cache_write, output, step, account) VALUES (?, ?, ?, NULL, 'p1', NULL, ?, 0, 0, 0, "
                   "'brain', ?)", [
        (T + 2 * HOUR, "claude", "opus", 1_494, ""),  # in the first 5-hour window: 1,494 of 3,000
        (T + 3 * HOUR, "claude", "opus", 9_000, "acct-y"),  # another account
        # Codex's weekly window reported as `primary`, and a model's own bucket.
        (T - DAY, "codex", "gpt-5", 500, "acct-w"),
        (T + 3 * HOUR, "codex", "gpt-5.3-codex-spark", 200, "acct-w"),
        (T + 4 * HOUR, "codex", "gpt-5", 300, "acct-w"),
    ])
    db.executemany("INSERT INTO turn_usage (at_ms, provider, model, conversation_id, task_id, input, cached_input, "
                   "cache_write, output) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)", [
        (T + 500, "claude", "opus", "c1", None, 7, 0, 0, 0),  # before the opening message
        (T + 1_500, "claude", "opus", "c1", None, 100, 800, 50, 50),
        (T + 3_000, "claude", "sonnet", "c1", "t1", 400, 0, 0, 100),  # its task
        (T + 9_000, "claude", "opus", "c1", None, 999, 0, 0, 0),  # after the merge
        (T + 2_000, "codex", "gpt", "c4", "t2", 200, 50, 0, 50),
        (T + 5 * HOUR // 2, "claude", "opus", "c9", None, 1_506, 0, 0, 0),  # a tab's turn in the 5-hour window
    ])
    db.execute("CREATE TABLE quota_samples (provider TEXT, window TEXT, used_percent REAL, resets_at_ms INTEGER, "
               "at_ms INTEGER, account TEXT NOT NULL DEFAULT '')")
    db.executemany("INSERT INTO quota_samples VALUES (?, ?, ?, ?, ?, ?)", [
        ("claude", "five_hour", 40, T + 5 * HOUR, T + HOUR, ""),
        ("claude", "five_hour", 70, T + 5 * HOUR + 30_000, T + 2 * HOUR, ""),  # same window, its reset moved a little
        ("claude", "five_hour", 5, T + 10 * HOUR, T + 6 * HOUR, ""),  # resets after the end
        ("claude", "seven_day", 60, T + 3 * HOUR, T + HOUR, ""),
        ("claude", "seven_day", 10, T + 3 * HOUR + 7 * DAY, T + 4 * HOUR, ""),
        ("codex", "secondary", 90, T + 5 * HOUR, T + 2 * HOUR, "acct-x"),  # reset with no later sample
        ("codex", "primary", 20, T + 8 * HOUR, T + 3 * HOUR, "acct-x"),
        # A `primary` window whose reset is days past a sample is the week, not 5 hours.
        ("codex", "primary", 70, T + 6 * HOUR, T - 2 * DAY, "acct-w"),
        # Before it, the same slot held a 5-hour window: it stays 5 hours.
        ("codex", "primary", 40, T - 3 * DAY + 4 * HOUR, T - 3 * DAY, "acct-w"),
        # A model's own bucket: `<bucket>.primary`, 5 hours, its model's rows only.
        ("codex", "codex_spark.primary", 50, T + 6 * HOUR, T + 2 * HOUR, "acct-w"),
        # A bucket whose id is in no model slug: its model is not recorded, so it is unresolved.
        ("codex", "other_bucket.primary", 70, T + 6 * HOUR, T + 2 * HOUR, "acct-w"),
        # Another login on the same slot: its reset time moves back before either passed.
        ("claude", "seven_day", 50, T + 20 * DAY, T + HOUR, "acct-y"),
        ("claude", "seven_day", 30, T + 2 * DAY, T + 2 * HOUR, "acct-y"),
        # A window that resets after the end, seen by a sample after the end.
        ("claude", "five_hour", 60, T + 7 * HOUR + 30 * 60_000, T + HOUR, "acct-z"),
        ("claude", "five_hour", 10, T + 12 * HOUR, T + 8 * HOUR, "acct-z"),
    ])
    db.commit()
    db.close()


class BenchMetrics(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="bench-metrics-")
        root = cls.tmp.name
        data = os.path.join(root, "data")
        os.makedirs(data)
        repo = os.path.join(root, "repo")
        cls.m1, cls.m2, cls.m3 = build_repo(repo)
        build_events(os.path.join(data, "brigadier.db"), repo, cls.m1, cls.m2, cls.m3)
        build_routing(os.path.join(data, "routing.sqlite"))
        run = subprocess.run([sys.executable, "-I", SCRIPT, data, "--until", str(T + 7 * HOUR)],
                             capture_output=True, text=True)
        assert run.returncode == 0, run.stderr
        cls.out = json.loads(run.stdout)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_first_look_counts_each_request_once_in_both_merge_modes(self):
        first = self.out["first_look"]
        rows = {r["request"]: (r["ready"], r["merged"], r["auto"], r["touches"], r["first_look"]) for r in first["requests"]}
        self.assertEqual(rows, {
            "r1": (True, True, False, 0, True),
            "r2": (True, True, True, 2, False),
            "r3": (True, False, None, None, False),
            "r4": (False, True, True, 0, True),
        })
        self.assertEqual((first["population"], first["first_look"], first["rate"]), (4, 2, 0.5))

    def test_touches_per_merged_change(self):
        touches = self.out["touches_per_merged_change"]
        self.assertEqual([(c["conversation"], c["touches"], c["touched_by"]) for c in touches["changes"]],
                         [("c1", 0, []), ("c2", 2, [["card", 1], ["file", 1]]), ("c4", 0, [])])
        self.assertEqual(touches["mean"], 0.6667)

    def test_escaped_bugs(self):
        bugs = self.out["escaped_bugs"]
        self.assertEqual([(c["merge_commit"], c["escaped"], c["by"] and c["by"]["why"]) for c in bugs["changes"]],
                         [(self.m1, True, "fix"), (self.m2[0], True, "revert"), (self.m3, False, None)])
        self.assertEqual((bugs["merged"], bugs["escaped"]), (3, 2))

    def test_wasted_quota_at_reset(self):
        wasted = self.out["wasted_quota_at_reset"]
        self.assertEqual({k: (v["resets"], v["wasted_percent"]) for k, v in wasted.items()},
                         {"five_hour": (4, [30.0, 50.0, 30.0, 60.0]), "weekly": (3, [40.0, 30.0, 10.0])})
        self.assertEqual(wasted["weekly"]["mean"], 26.67)

    def test_brain_quota_per_window(self):
        rows = {(w["provider"], w["account"], w["window"], w["resets_at_ms"]): w
                for w in self.out["brain_quota_per_window"]}
        first = rows[("claude", "", "five_hour", T + 5 * HOUR + 30_000)]
        self.assertEqual((first["tokens"], first["brain_tokens"], first["brain_share"], first["brain_percent"],
                          first["reset"]), (3_000, 1_494, 0.498, 34.86, True))
        # The weekly window began a week before its reset: c1's rows count too (5,506 in all).
        weekly = rows[("claude", "", "seven_day", T + 3 * HOUR)]
        self.assertEqual((weekly["tokens"], weekly["brain_tokens"], weekly["brain_percent"]), (5_506, 1_494, 16.28))
        # Codex's windows saw no Brain rows.
        self.assertEqual(rows[("codex", "acct-x", "secondary", T + 5 * HOUR)]["brain_tokens"], 0)
        # Codex's weekly `primary` spans the week: all three of the account's rows.
        week = rows[("codex", "acct-w", "primary", T + 6 * HOUR)]
        self.assertEqual((week["kind"], week["tokens"], week["brain_percent"]), ("weekly", 1_000, 70.0))
        # The bucket's window counts its own model's rows only.
        spark = rows[("codex", "acct-w", "codex_spark.primary", T + 6 * HOUR)]
        self.assertEqual((spark["kind"], spark["tokens"], spark["brain_percent"]), ("five_hour", 200, 50.0))
        other = rows[("codex", "acct-w", "other_bucket.primary", T + 6 * HOUR)]
        self.assertEqual((other["scope_unresolved"], other["tokens"], other["brain_percent"]), (True, None, None))
        earlier = rows[("codex", "acct-w", "primary", T - 3 * DAY + 4 * HOUR)]
        self.assertEqual((earlier["kind"], earlier["tokens"]), ("five_hour", 0))

    def test_tokens_per_merged_change(self):
        tokens = [(t["conversation"], t["by_model"], t["total"]) for t in self.out["tokens_per_merged_change"]]
        self.assertEqual(tokens, [("c1", {"opus": 1000, "sonnet": 500}, 1500), ("c2", {}, 0),
                                  ("c4", {"gpt": 300}, 300)])

    def test_untyped_records_are_skipped(self):
        self.assertEqual(self.out["skipped_untyped"], 1)


if __name__ == "__main__":
    unittest.main()
