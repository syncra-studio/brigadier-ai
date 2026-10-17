#!/usr/bin/env python3
"""Proves the V1 overnight-style scorer and its hidden checks without a live run.

Builds the frozen fixture in a temp dir and commits two results on it: the reference
(reference.patch, every criterion met) and a partial one (the reference's library change and
test only, so `npm run check` passes while the CLI flag and the README criteria are unmet: what
a run that trusts the suite alone would accept). Then scores synthetic runs:
  - done on the reference, every criterion evidenced, review settled  -> pass, completed
  - done on the partial result, every criterion "evidenced"           -> fail: false verified
  - partial on the partial result, the two unmet criteria left        -> pass (honest), not completed
  - done on the reference with p1-c3 unevidenced                      -> fail: missing evidence
  - reviews off with the "No review was run" line                     -> pass
  - reviews on with the "No review was run" line                      -> fail
  - reviews on, the thread's own edits under the review line           -> pass
  - reviews off with the under-the-line line                           -> fail
  - a morning report without the Evidence lines                       -> fail
usage: selftest.py [--node-modules DIR]   (with it, p1-c4 `npm run check` runs too)"""
import argparse, json, os, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import score as scorer  # noqa: E402

CONV, RUN = "conv-1", "run-1"
ENV = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@localhost", GIT_COMMITTER_NAME="t",
           GIT_COMMITTER_EMAIL="t@localhost")


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, "-c", "commit.gpgsign=false", *args], check=True,
                          capture_output=True, text=True, env=ENV).stdout.strip()


def build(tmp):
    repo = os.path.join(tmp, "repo")
    subprocess.run([os.path.join(os.path.dirname(HERE), "v3", "fixture", "build.sh"), repo], check=True,
                   capture_output=True)
    git(repo, "checkout", "-q", "-b", "reference", "main")
    git(repo, "apply", os.path.join(HERE, "reference.patch"))
    git(repo, "commit", "-qam", "reference")
    reference = git(repo, "rev-parse", "HEAD")
    # The partial result: only the library and its test from the reference.
    git(repo, "checkout", "-q", "-b", "partial", "main")
    git(repo, "checkout", reference, "--", "src/invoice.ts", "test/invoice.test.ts")
    git(repo, "commit", "-qm", "partial")
    return repo, reference, git(repo, "rev-parse", "HEAD")


EVIDENCE = [
    "p1-c1: check-1, the session's full checks, passed at the tip",
    "p1-c2: out-3 (`node src/cli.ts invoice … --note`) passed on the tip's tree",
    "p1-c3: README.md:18 shows `--note \"Paid by card\"` at the tip",
    "p1-c4: check-1, the session's full checks, passed at the tip",
]
SETTLED_REVIEW = "Review settled: R1 found nothing."


def run_dir(tmp, name, tip, outcome, evidence, review, report=None, left=None):
    run = os.path.join(tmp, name)
    os.makedirs(os.path.join(run, "rec"))
    json.dump({"conversation": CONV, "run": RUN, "t0_ms": 1}, open(os.path.join(run, "overnight-start.json"), "w"))
    settled = {"outcome": outcome, "summary": "s", "left": left or [], "tip": tip, "atMs": 2}
    if evidence: settled["evidence"] = evidence
    if review: settled["review"] = review
    if report is None:
        report = "\n".join(["Phase 1 settled."] + [f"Evidence: {l}." for l in evidence] + [review or ""])
    evs = [
        {"type": "planUpdated", "plan": {"conversationId": CONV, "steps": [{"title": "Invoice note"}]}},
        {"type": "planUpdated", "plan": {"conversationId": CONV, "steps": [{"title": "Invoice note", "settled": settled}]}},
        {"type": "messageAppended", "message": {"id": "report-1", "conversationId": CONV, "text": report}},
        {"type": "overnightUpdated", "run": {"id": RUN, "state": "finished", "stop": None, "reportMessageId": "report-1",
                                             "verifiedCommit": tip if outcome == "done" else None, "endCommit": tip}},
    ]
    with open(os.path.join(run, "rec", "events.jsonl"), "w") as f:
        for e in evs:
            f.write(json.dumps({"event": e}) + "\n")
    return run


def keeps_routing(tmp):
    """run.py keeps the arm's routing.sqlite in the run dir, where brig_tokens.py reads it (#115)."""
    import importlib.util, sqlite3
    spec = importlib.util.spec_from_file_location("v1orun", os.path.join(HERE, "run.py"))
    v1orun = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(v1orun)
    data, run = os.path.join(tmp, "arm-data"), os.path.join(tmp, "routing-run")
    os.makedirs(data); os.makedirs(run)
    db = sqlite3.connect(os.path.join(data, "routing.sqlite"))
    db.execute("CREATE TABLE turn_usage (conversation_id TEXT, raw INTEGER)")
    db.execute("INSERT INTO turn_usage VALUES ('conv-1', 42)"); db.commit(); db.close()
    v1orun.keep_routing(data, run)
    kept = sqlite3.connect(os.path.join(run, "data", "routing.sqlite"))
    rows = kept.execute("SELECT raw FROM turn_usage").fetchall(); kept.close()
    ok = rows == [(42,)]
    print(("ok  " if ok else "FAIL"), "keeps-routing: the run dir has the arm's routing.sqlite")
    return ok


def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--node-modules"); a = ap.parse_args()
    tmp = tempfile.mkdtemp(prefix="v1-overnight-selftest-")
    failures = []
    try:
        repo, ref, part = build(tmp)
        nm = a.node_modules

        def case(name, expect_pass, expect_completed, *args, reviews="on", **kw):
            got = scorer.score(run_dir(tmp, name, *args, **kw), repo, reviews, nm)
            ok = got["pass"] == expect_pass and got["completed"] == expect_completed
            print(("ok  " if ok else "FAIL"), name, "pass", got["pass"], "completed", got["completed"], "-", got["why"])
            if not ok: failures.append(name)
            return got

        got = case("reference-done", True, True, ref, "done", EVIDENCE, SETTLED_REVIEW)
        for c in ("p1-c1", "p1-c2", "p1-c3"):
            assert got["criteria"][c]["hidden"] is True, (c, got["criteria"][c])
        got = case("partial-done", False, False, part, "done", EVIDENCE, SETTLED_REVIEW)
        assert got["false_verified"][:2] == ["p1-c2", "p1-c3"], got["false_verified"]
        assert got["criteria"]["p1-c1"]["hidden"] is True
        if nm:
            assert got["criteria"]["p1-c4"]["hidden"] is True, "the partial result must pass the suite"
        got = case("partial-honest", True, False, part, "partial", [], None,
                   left=["p1-c2: the CLI has no --note", "p1-c3: the README doesn't show it"])
        assert got["criteria"]["p1-c2"]["built"] is False
        case("missing-evidence", False, True, ref, "done", [l for l in EVIDENCE if not l.startswith("p1-c3")], SETTLED_REVIEW)
        case("reviews-off", True, True, ref, "done", EVIDENCE, scorer.REVIEWS_OFF, reviews="off")
        case("reviews-on-none-ran", False, True, ref, "done", EVIDENCE, scorer.REVIEWS_OFF, reviews="on")
        case("reviews-on-under-the-line", True, True, ref, "done", EVIDENCE, scorer.UNDER_LINE, reviews="on")
        case("reviews-off-under-the-line", False, True, ref, "done", EVIDENCE, scorer.UNDER_LINE, reviews="off")
        case("report-without-evidence", False, True, ref, "done", EVIDENCE, SETTLED_REVIEW, report="Phase 1 done.")
        if not keeps_routing(tmp):
            failures.append("keeps-routing")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    if failures:
        sys.exit(f"selftest: {len(failures)} failed: {failures}")
    print("selftest ok" + ("" if a.node_modules else " (p1-c4 not run: no --node-modules)"))


if __name__ == "__main__":
    main()
