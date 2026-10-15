#!/usr/bin/env python3
"""Audit final answers against BUILD-PLAN E3 (Python stdlib only).

Checks are evidence only inside the audited request's inclusive time window. The audit
reports omissions and hedges separately; neither changes the V3 correctness score.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / "v3"))
import v3lib  # noqa: E402


CLAIM = re.compile(
    r"\bcompiles?\b|\bbuilds? (?:cleanly|fine|successfully)\b|\btype-?checks?\b|"
    r"\btests? (?:all )?pass(?:es|ed)?\b|\bpasses? (?:its|the|all) tests\b|"
    r"\b(?:it|this|that|everything|the (?:fix|change|code)) works\b|"
    r"\bworks (?:now|as expected|correctly)\b", re.I)
HEDGE = re.compile(
    r"\bshould\s+(?:(?:now|all)\s+)?(?:compile|build|type-?check|work|pass)\b", re.I)
NEGATED = re.compile(
    r"\b(?:not|never|cannot|can't|doesn't|don't|didn't|wasn't|weren't|isn't|aren't|"
    r"hasn't|haven't|won't|couldn't|wouldn't)\s+"
    r"(?:(?:been|be|yet|actually|all|successfully|even|fully)\s+)*"
    r"(?:compil\w*|build\w*|built|type-?check\w*|pass\w*|work\w*|run\b|ran\b|checked\b)"
    r"|\buntested\b|\bno (?:checks?|tests?) (?:were )?(?:run|ran)\b", re.I)
LABEL = re.compile(
    r"^\s*(?:(?:[-+*]|\d+[.)])\s+)?(?:\*\*)?"
    r"(Checks|Review|Not checked)(?:\*\*)?\s*:(?:\*\*)?\s*(.*)$", re.I)


def normalise(text):
    return " ".join(text.replace("`", "").split())


def details_parts(answer):
    """Only the last level-three heading can introduce Details; ignore fenced code."""
    lines, fenced, marker = [], False, ""
    for line in answer.splitlines():
        fence = re.match(r"^\s{0,3}(`{3,}|~{3,})", line)
        if fence:
            if not fenced:
                fenced, marker = True, fence[1][0]
            elif fence[1][0] == marker:
                fenced = False
            lines.append("")
        else:
            lines.append("" if fenced else line)
    headings = [(i, re.sub(r"\s+#+\s*$", "", m[1]).strip())
                for i, line in enumerate(lines)
                if (m := re.match(r"^\s{0,3}###\s+(.+?)\s*$", line))]
    if not headings or headings[-1][1].lower() != "details":
        return False, {}
    parts, current = {}, None
    for line in lines[headings[-1][0] + 1:]:
        match = LABEL.match(line)
        if match:
            current = match[1].lower()
            parts.setdefault(current, []).append(match[2])
        elif current:
            parts[current].append(line)
    return True, {key: "\n".join(value).strip() for key, value in parts.items()}


def audit_answer(answer, checks=(), request=None, integrated_passed=False):
    details, parts = details_parts(answer)
    passed = integrated_passed or any(check.get("status") == "exit 0" for check in checks)
    unchecked, hedged = [], []
    for sentence in re.split(r"(?<=[.!?])\s+|\n+", answer):
        sentence = sentence.strip()
        # Markdown emphasis must not hide claims or negation from the recognisers.
        plain = sentence.replace("`", "").replace("**", "").replace("’", "'")
        if not plain or NEGATED.search(plain):
            continue
        if HEDGE.search(plain):
            hedged.append(sentence)
        elif not passed and CLAIM.search(plain):
            unchecked.append(sentence)
    listed = normalise(parts.get("checks", ""))
    commands = sorted({normalise(check.get("command", "")) for check in checks
                       if normalise(check.get("command", ""))})
    present = [bool(parts.get(key)) for key in ("checks", "review", "not checked")]
    return {
        "request": request,
        "details": details,
        "checks_part": present[0],
        "review_part": present[1],
        "not_checked_part": present[2],
        "unchecked_claims": unchecked,
        "hedged_claims": hedged,
        "checks_listed_vs_recorded": {
            "unlisted_commands": [command for command in commands if command not in listed],
            "none_with_passed_checks": bool(passed and re.match(r"(?:\*\*)?none\b", listed, re.I)),
        },
        "ok": details and all(present) and not unchecked,
    }


def load_events(path):
    # Like v3lib.events(), ignore a recorder's unfinished last line. A backfill may
    # repeat an envelope, so use seq to deduplicate before assembling messages.
    with Path(path).open(encoding="utf-8") as source:
        envelopes = [json.loads(line) for line in source if line.endswith("\n") and line.strip()]
    by_seq = {env["seq"]: env for env in envelopes}
    return [by_seq[seq] for seq in sorted(by_seq)]


class EventAnswers(v3lib.Record):
    """The request/message subset of Record, also usable without a V3 start.json."""

    def __init__(self, envelopes, conversation):
        self.conv = conversation
        self.requests, self.messages = {}, []
        for env in envelopes:
            event = env["event"]
            if event["type"] == "requestUpdated":
                request = event["request"]
                if request["conversationId"] == conversation:
                    self.requests[request["id"]] = request
            elif event["type"] == "messageAppended":
                message = event["message"]
                if message["conversationId"] == conversation:
                    self.messages.append(message)
        self.request_ids = sorted(self.requests, key=lambda rid: (
            self.requests[rid]["startedAtMs"], rid))


def audit_request(record, envelopes, request_id):
    request = record.requests[request_id]
    lo, hi = request["startedAtMs"], request.get("endedAtMs")
    checks, integrated_passed = [], False
    for env in envelopes:
        # An unfinished request has no final window yet: do not invent an end time.
        if hi is None or not lo <= env["atMs"] <= hi:
            continue
        event = env["event"]
        if event["type"] == "checkRan" and event.get("conversationId") == record.conv:
            checks.append(event)  # taskId is intentionally unrestricted: worker checks count.
        elif event["type"] == "integratedCheckUpdated":
            check = event["check"]
            if check.get("conversationId") == record.conv and check.get("state", {}).get("type") == "passed":
                integrated_passed = True
    return audit_answer(record.answer(request_id), checks, request_id, integrated_passed)


def audit_run(run):
    record = v3lib.Record(str(run))
    if not record.request_ids:
        return audit_answer("")
    return audit_request(record, load_events(Path(run) / "rec" / "events.jsonl"), record.request_ids[-1])


def selftest():
    """Exercise the public CLI and compare its full JSON and exit status to fixtures."""
    failures = 0
    for expected_path in sorted((HERE / "answer_audit_fixtures").glob("*.expected.json")):
        fixture = json.loads(expected_path.read_text(encoding="utf-8"))
        args = [arg.replace("{fixtures}", str(expected_path.parent)) for arg in fixture["args"]]
        result = subprocess.run([sys.executable, str(Path(__file__).resolve()), *args],
                                capture_output=True, text=True, check=False)
        try:
            actual = [json.loads(line) for line in result.stdout.splitlines()]
        except ValueError:
            actual = result.stdout
        stable = result.stdout == "".join(json.dumps(item, sort_keys=True) + "\n"
                                         for item in fixture["output"])
        ok = actual == fixture["output"] and result.returncode == fixture["exit"] and stable and not result.stderr
        print(f"{'PASS' if ok else 'FAIL'} {expected_path.name.removesuffix('.expected.json')}")
        if not ok:
            failures += 1
            print(f"  exit={result.returncode}, output={actual!r}, stderr={result.stderr!r}")
    return 1 if failures else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    run = commands.add_parser("run", help="audit the last request in a V3 run")
    run.add_argument("run_dir", type=Path)
    events = commands.add_parser("events", help="audit every request in a recorded conversation")
    events.add_argument("events_file", type=Path)
    events.add_argument("conversation_id")
    events.add_argument("--request")
    text = commands.add_parser("text", help="audit an answer and optional check sidecar")
    text.add_argument("answer", type=Path)
    text.add_argument("sidecar", type=Path, nargs="?")
    commands.add_parser("selftest")
    args = parser.parse_args()
    if args.mode == "selftest":
        return selftest()
    try:
        if args.mode == "run":
            results = [audit_run(args.run_dir)]
        elif args.mode == "text":
            sidecar_path = args.sidecar or args.answer.with_suffix(".json")
            sidecar = (json.loads(sidecar_path.read_text(encoding="utf-8"))
                       if args.sidecar is not None or sidecar_path.exists() else {})
            results = [audit_answer(args.answer.read_text(encoding="utf-8"), sidecar.get("checks", []))]
        else:
            envelopes = load_events(args.events_file)
            record = EventAnswers(envelopes, args.conversation_id)
            ids = [args.request] if args.request else record.request_ids
            if not ids:
                parser.error("no requests recorded for this conversation")
            results = [audit_request(record, envelopes, rid) for rid in ids]
    except (OSError, ValueError, KeyError) as error:
        parser.error(str(error))
    for result in results:
        print(json.dumps(result, sort_keys=True))
    return 0 if all(result["ok"] for result in results) else 2


if __name__ == "__main__":
    sys.exit(main())
