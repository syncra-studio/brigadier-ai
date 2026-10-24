#!/usr/bin/env python3
"""D66: thirty sparse facts, revised decisions, open promises and one large tool output."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import secrets
import signal
import sys
import time

import battery

FACTS = {
    "release": "release commit", "rollback": "rollback tag", "owner": "reconciliation owner",
    "ticket": "support escalation ticket", "timezone": "billing timezone identifier",
    "fixture": "golden fixture identifier", "currency": "settlement currency code",
    "bucket": "archive bucket", "runbook": "runbook path", "alert": "alert route",
    "checksum": "schema checksum", "migration": "migration identifier", "canary": "canary tenant",
    "dashboard": "dashboard identifier", "incident": "incident reference", "partition": "partition key",
    "audit": "audit reference", "batch": "import batch identifier",
}
DECISIONS = ("region", "queue", "timeout", "export_format", "cache_scope", "auth_mode", "notify", "retries")
PROMISES = ("promise_owner", "promise_deadline", "promise_reviewer")
DESCRIPTIONS = {**FACTS, **{k: "current " + k.replace("_", " ") + " decision" for k in DECISIONS},
                **{k: "still-open " + k.replace("_", " ") for k in PROMISES},
                "tool_receipt": "receipt in the one-time diagnostic output"}


def scenario():
    # Nonces make prior knowledge and sensible guesses useless, including for revisions.
    token = lambda label: label.upper() + "-" + secrets.token_hex(5).upper()
    early = {key: token(key) for key in DESCRIPTIONS if key != "tool_receipt"}
    revisions = [{key: token(key) for key in DECISIONS} for _ in range(2)]
    # Two reversals deliberately restore the first value; chronological order must win.
    for key in DECISIONS[:2]:
        revisions[1][key] = early[key]
    expected = {**early, **revisions[-1], "tool_receipt": token("receipt")}
    stale = {key: sorted({early[key], *(rev[key] for rev in revisions)} - {expected[key]})
             for key in DECISIONS}
    return early, revisions, expected, stale


def contains(value, expected):
    return bool(re.search(r"(?<![\w-])" + re.escape(expected) + r"(?![\w-])", str(value), re.I))


def grade(answer, expected, stale, status=""):
    parsed = battery.parse_answer(answer)
    rows = {}
    for key, target in expected.items():
        value = parsed.get(key, "unknown")
        old = [item for item in stale.get(key, []) if contains(value, item)]
        correct = contains(value, target) and not old
        rows[key] = {"description": DESCRIPTIONS[key], "answer": str(value),
                     "category": "visibility" if key == "tool_receipt" else "recall",
                     "verdict": "pass" if correct else "unknown" if str(value).lower() == "unknown" else "fail",
                     "newest": correct if key in DECISIONS else None,
                     "stale": bool(old) if key in DECISIONS else None}
    scores = {"recall": sum(r["verdict"] == "pass" for r in battery.recall_rows(rows).values()) / len(battery.recall_rows(rows)),
              "newest_decision_wins": sum(rows[k]["newest"] for k in DECISIONS) / len(DECISIONS),
              "stale_decision_mistakes": sum(rows[k]["stale"] for k in DECISIONS),
              "decision_count": len(DECISIONS),
              "open_promises_raised": sum(contains(status, expected[k]) for k in PROMISES),
              "open_promise_count": len(PROMISES)}
    return rows, scores


class Updates(battery.Battery):
    def run(self):
        self.setup()
        early, revisions, expected, stale = scenario()
        self.result.update(block="updates", expected=expected, stale=stale)
        self.send("plant-early", "We are planning a synthetic import release. These facts are each given once. "
                  "The decisions are current until I change them; promises remain open until explicitly closed. "
                  "Do not use workers, commit or push. Acknowledge without repeating values.\n" +
                  "\n".join(f"{key} ({DESCRIPTIONS[key]}): {value}" for key, value in early.items()))
        script = self.repo / "diagnostic.py"
        script.write_text("from pathlib import Path\nPath(__file__).unlink()\nfor i in range(1800):\n    print(" +
                          repr("release diagnostic receipt: " + expected["tool_receipt"]) +
                          " if i == 900 else 'diagnostic noise %04d ' % i + 'x' * 120)\n")
        self.send("plant-diagnostic", "Run `python3 diagnostic.py` once. Its one release diagnostic receipt matters. "
                  "Do not repeat or save the output in workspace files. Reply only 'read'.")
        script.unlink(missing_ok=True)
        for i in range(self.args.max_fillers):
            # All arms get identical positions and lengths, even after enough transitions.
            if i in (self.args.max_fillers // 3, 2 * self.args.max_fillers // 3):
                revision = revisions[0 if i == self.args.max_fillers // 3 else 1]
                self.send(f"revision-{i}", "Update: replace the previous choices below. These are now the current "
                          "decisions; all other facts and open promises stand. Acknowledge without repeating values.\n" +
                          "\n".join(f"{key}: {value}" for key, value in revision.items()))
            before = self.generations()
            ballast = " ".join(hashlib.sha256(f"updates-{i}-{j}".encode()).hexdigest()[:12]
                               for j in range(self.args.ballast))
            self.send(f"ballast-{i:02}", f"Ignore this inert fixture batch {i}; reply only 'batch {i} read'.\n" + ballast)
            if self.args.arm in ("seam", "seam-repaired"):
                self.wait(lambda: self.generations() > before, 3)
        self.result["seams_before_checks"] = self.generations()
        status = self.send("status", "Where are we? Mention anything still owed or unresolved, using its exact identifier.")["answer"]
        self.result["update_status"] = status
        probe = self.send("probe", "Return only a JSON object with the following keys. Give the value for each "
                          "fact, the CURRENT value only for each decision (no historical alternatives), and each "
                          "still-open promise identifier. Use conversation context and existing transcript lookup tools; "
                          "do not run commands, read workspace fixtures, or edit files. Say 'unknown' when missing; do not guess.\n" +
                          "\n".join(f"{key}: {description}" for key, description in DESCRIPTIONS.items()))
        self.result["items"], self.result["update_scores"] = grade(probe["answer"], expected, stale, status)


def report(result):
    lines = ["# Continuity updates", "", "Report only; numbers never gate work.", "",
             f"Source: `{result['source']}`; {result['provider']} / `{result['model']}` / {result['arm']}.",
             f"Threshold: {result['threshold']}; transitions before checks: {result.get('seams_before_checks', 'unavailable')}.",
             "Thirty items: 18 one-time facts, eight revised decisions (two reversed), three open promises, "
             "and one fact in the middle of a >250 KB one-time tool output.", "",
             "Scores: `" + json.dumps(result.get("update_scores")) + "`", "",
             "| Item | Recall | Newest wins | Stale mistake | Answer |", "|---|---|---|---|---|"]
    for key, row in result.get("items", {}).items():
        value = row["answer"].replace("|", "\\|").replace("\n", " ")
        lines.append(f"| {key} | {row['verdict']} | {row['newest']} | {row['stale']} | {value} |")
    lines += ["", "Run problems: " + ("; ".join(result["errors"]) or "none"),
              "The large-output item is a visibility diagnostic, excluded from recall. "
              "Exact-identifier scoring measures retention, not semantic equivalence. "
              "CLI summary cut points are checked between turns; seam notes can close asynchronously.", ""]
    lines += battery.arm_markdown(result) + battery.economics_markdown(result)
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon", required=True)
    parser.add_argument("--provider", choices=("claude", "codex"), default="claude")
    from arms import ARMS, configure, make
    parser.add_argument("--arm", choices=ARMS, default="seam")
    parser.add_argument("--fair", action="store_true", help="opt-in shared threshold and identical lookup access")
    parser.add_argument("--model")
    parser.add_argument("--tokens", type=int)
    parser.add_argument("--max-fillers", type=int, default=9)
    parser.add_argument("--min-fillers", type=int, default=9, help="compatibility with the shared nightly schedule")
    parser.add_argument("--ballast", type=int, default=1200)
    parser.add_argument("--timeout", type=int, default=3000)
    parser.add_argument("--turn-timeout", type=int, default=180)
    parser.add_argument("--report")
    parser.add_argument("--evidence")
    parser.add_argument("--scratch-prefix", default="continuity-updates")
    args = parser.parse_args()
    configure(args, parser, 43750)
    args.model = args.model or ("opus" if args.provider == "claude" else "gpt-6.1-sol")
    if "fable" in args.model.lower() or args.tokens <= 0 or args.max_fillers < 3 or args.ballast < 0:
        parser.error("use a permitted model, positive threshold, at least three fillers and nonnegative ballast")
    if str(Path(args.daemon).resolve()).startswith("/Applications/"):
        parser.error("use a debug daemon built from this checkout")
    if args.evidence and Path(args.evidence).resolve().is_relative_to(battery.SRC):
        parser.error("keep detailed evidence outside the repository")
    args.report = args.report or str(Path(__file__).parent / "reports" /
                                  (time.strftime("%Y-%m-%d") + f"-{args.provider}-{args.arm}-updates.md"))
    from zed import SkippedArm
    run = make(Updates, args)
    run.result["block"] = "updates"
    def terminated(signum, frame):
        raise TimeoutError("nightly supervisor stopped the run")
    signal.signal(signal.SIGTERM, terminated)
    try:
        run.run()
    except Exception as error:
        battery.log(f"run problem: {error}")
        if isinstance(error, SkippedArm):
            run.result.update(status="skipped", reason=str(error))
        else:
            run.errors.append(str(error))
    finally:
        try:
            run.capture()
        except Exception as error:
            run.errors.append("capture: " + str(error))
        run.cleanup()
        battery.write_report(run.result, report(run.result), args.report)
        battery.log("report: " + args.report)
    return 0 if "items" in run.result or run.result.get("status") == "skipped" else 2


if __name__ == "__main__":
    sys.exit(main())
