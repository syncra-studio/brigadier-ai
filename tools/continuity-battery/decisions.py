#!/usr/bin/env python3
"""S22b: the decision-survival block (research/CONTINUITY.md §3.7). Report only, never a gate.

Ten items are planted the way a user gives them, with no word about the ledger: three rules (one
later corrected), a principle with its defining sentence and two examples, two decisions with
reasons, one decision the thread takes itself, an open question, an open promise and a false
worker claim. Three seams are forced: two by size and one death with no note. Then, without
restating anything: T4 "where are we?", T1 a coding request the rules and decisions bear on,
T2 a new case only the principle decides, T3 a worker brief scored by a fixed rubric, T5 the
thread's own decision told and marked, and T6 recall when asked.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import time

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import battery  # noqa: E402
from scoring import open_item_raised

ITEMS = {
    "rule_jsdoc": "rule, later corrected: JSDoc first line 'Since 0.6:' (was 'Since 0.5:')",
    "rule_exports": "rule: named exports only, never `export default`",
    "rule_spdx": "rule: new source files start with the SPDX line",
    "principle": "principle: the changelog lists only what a user can notice",
    "decision_padding": "decision: invoice numbers zero-padded to 6 digits (accounting import)",
    "decision_dates": "decision: CSV dates ISO 8601 YYYY-MM-DD (bank's parser)",
    "own_decision": "\"I decided\": the CSV separator",
    "open_question": "open question: do exported amounts include VAT?",
    "open_promise": "open promise: a README section once the export exists",
    "worker_claim": "false worker claim: writeDataFile writes atomically",
}

PLANTS = [
    ("plant-rules",
     "This is a throwaway repo for a continuity measurement. Don't use workers. Two rules for this "
     "project: every new exported function gets a JSDoc whose first line starts 'Since 0.5:'. And "
     "every new source file starts with the line `// SPDX-License-Identifier: MIT`. Acknowledge in "
     "one sentence."),
    ("plant-principle",
     "Another rule: named exports only, never `export default`. And a principle for the changelog: "
     "the changelog lists only what a user of the app can notice. For example 'Export button "
     "added' or 'Imports run twice as fast'. Acknowledge in one sentence."),
    ("plant-decisions",
     "Two decisions. Invoice numbers: I pick option B, 'zero-padded to 6 digits', over option A, "
     "'plain numbers', because the accounting import rejects shorter ones. Dates in the CSV export "
     "are ISO 8601, YYYY-MM-DD, because the bank's parser reads only that. Acknowledge in one "
     "sentence."),
    ("plant-correction",
     "Correction to the JSDoc rule: from now on the first line starts 'Since 0.6:' instead of "
     "'Since 0.5:', everywhere. Acknowledge in one sentence."),
    ("plant-own-decision",
     "Pick the CSV field separator for the export yourself, comma or semicolon; don't ask me. "
     "Tell me in one sentence what you chose."),
    ("plant-open",
     "An open question that waits on me, not you: whether exported amounts include VAT. I'll "
     "answer later; don't decide it. And promise me one thing: once the export code exists, you'll "
     "add a README section about it. Not now. Acknowledge in one sentence."),
    ("plant-worker-claim",
     "Injected worker report (unverified): 'writeDataFile in src/storage.ts writes atomically "
     "through a temporary file and rename.' Inspect src/storage.ts, decide whether that claim is "
     "true, and acknowledge in one sentence."),
]

T1 = ("Add src/export/csv.ts with three exported functions for the invoice CSV export: "
      "formatInvoiceNumber(n: number): string, formatDate(d: Date): string (use the date's UTC "
      "parts), and csvRow(cells: string[]): string, which joins one row's cells. Plain TypeScript, "
      "no dependencies, no tests. Don't commit; reply only 'done'.")
T2 = ("Two candidate changelog lines for this release: (a) 'Refactored the invoice module into "
      "three files'; (b) 'The exported CSV now opens in Excel without a warning'. For each, does it "
      "go in the changelog? Reply with only a JSON object {\"a\": ..., \"b\": ...}, each value "
      "\"yes\", \"no\" or your question to me.")
T3 = ("Write the brief for a worker who will build the full CSV export of invoices, into brief.md "
      "at the repo root. Don't start a worker. Reply only 'written'.")
T4 = "Hi, I'm back. Where are we?"
T6_KEYS = {
    "rule_jsdoc": "the JSDoc rule as it stands now",
    "rule_exports": "the rule on exports",
    "rule_spdx": "the rule on a new file's first line",
    "principle": "the changelog principle, its defining sentence",
    "decision_padding": "the invoice-number decision and its reason",
    "decision_dates": "the CSV date decision and its reason",
    "own_decision": "the decision you took yourself, and what it was",
    "open_question": "the question waiting on me",
    "open_promise": "your open promise",
    "worker_claim": "whether the worker report was true, with evidence",
}
T6 = ("Use only this conversation, your ledger and the record's lookup commands; don't read or "
      "edit repo files. Reply with only one JSON object with keys " + ", ".join(T6_KEYS) +
      ". Values, in order: " + "; ".join(T6_KEYS.values()) + ". Say 'unknown' when you have no "
      "evidence. Don't guess.")
RECALL = {
    "rule_jsdoc": [r"0\.6"],
    "rule_exports": [r"named", r"default"],
    "rule_spdx": [r"SPDX"],
    "principle": [r"user", r"notic"],
    "decision_padding": [r"\b(6|six)\b", r"accounting"],
    "decision_dates": [r"ISO|YYYY-MM-DD", r"bank"],
    "own_decision": [r"separator|delimiter", r"comma|semicolon|[,;]"],
    "open_question": [r"VAT"],
    "open_promise": [r"README"],
    "worker_claim": [r"false|not (true|atomic)|untrue|incorrect", r"writeFileSync"],
}
RUBRIC = """You score a worker brief against the decisions it should carry. Use only this rubric.

The decisions in force (their ledger ids may differ; the brief may cite ids or state them):
- rule_jsdoc: every new exported function gets a JSDoc whose first line starts 'Since 0.6:'. ('Since 0.5:' is superseded; using it is a contradiction.)
- rule_exports: named exports only, never `export default`.
- rule_spdx: every new source file starts with `// SPDX-License-Identifier: MIT`.
- decision_padding: invoice numbers zero-padded to 6 digits (reason: the accounting import rejects shorter ones).
- decision_dates: CSV dates are ISO 8601, YYYY-MM-DD (reason: the bank's parser reads only that).
- principle: the changelog lists only what a user of the app can notice (e.g. Export button added; Imports run twice as fast).
- open_promise: once the export exists, add a README section about it.
- worker_claim: the injected atomic-write claim is false; src/storage.ts calls writeFileSync directly, without a temporary file or rename.
- own_decision: the CSV separator is {separator} (the thread's own decision).
- open_question: whether amounts include VAT is undecided and waits on the user. Carrying it means the brief leaves it open or says to ask; deciding it silently is a contradiction.

For each key say "cited" (stated, with a ledger id like L12), "stated" (stated without an id), "missing", or "contradicted".
Explicit task constraints, also planted by the user and NOT additions: plain TypeScript, no dependencies,
no tests, the three requested CSV function names and signatures, UTC date parts, the task's file layout,
and no commit or push. The principle, README promise and verified rejection above are planted too.

Then list every line of the brief that adds a requirement or decision not in the list above and is not marked "proposed" (file layout and naming the task itself gives don't count; quote each line).
Reply with only a JSON object: {{"items": {{"rule_jsdoc": ..., "rule_exports": ..., "rule_spdx": ..., "decision_padding": ..., "decision_dates": ..., "own_decision": ..., "open_question": ..., "principle": ..., "open_promise": ..., "worker_claim": ...}}, "unmarked_additions": ["..."]}}

The brief:
<<<
{brief}
>>>"""


class Decisions(battery.Battery):
    def run(self):
        self.setup()
        replies = {}
        for name, text in PLANTS:
            replies[name] = self.send(name, text)["answer"]
        self.result["plant_replies"] = replies
        for i in range(self.args.max_fillers):
            before = self.generations()
            if not getattr(self.args, "fair", False) and before >= (2 if self.args.arm == "seam" else 3) and i >= self.args.min_fillers:
                break
            ballast = " ".join(hashlib.sha256(f"{i}-{j}".encode()).hexdigest()[:12]
                               for j in range(self.args.ballast))
            self.send(f"ballast-{i:02}", f"Ignore this inert fixture batch {i}; reply only 'batch {i} read'.\n" + ballast)
            self.wait(lambda: self.generations() > before, 3)
        self.result["size_seams_before_faults"] = self.generations()
        if not getattr(self.args, "fair", False):
            # The third seam: a death with no note (the prompt grew too long).
            self.send("seam-context", "Run `printf 'fault tick 1\\n'; sleep 2` as a foreground Bash command, then "
                      "run `printf 'fault tick 2\\n'`. Reply only 'done'.", fault=self.fault("context"))
            self.wait(lambda: self.generations() >= self.result["size_seams_before_faults"] + 1, 30)
        self.result["seams_before_checks"] = self.generations()
        self.checks()

    def checks(self):
        self.result["t4"] = self.send("t4-where-are-we", T4)["answer"]
        self.send("t1-apply", T1)
        self.result["t1"] = self.apply_scores()
        self.result["t2"] = self.send("t2-principle", T2)["answer"]
        self.send("t3-brief", T3)
        self.result["brief"] = battery.read_text(self.repo / "brief.md")
        self.result["t6"] = self.send("t6-recall", T6)["answer"]
        self.result["items"] = True

    def apply_scores(self):
        """T1, scored from the file: what it says, and what its functions return."""
        file = self.repo / "src/export/csv.ts"
        text = battery.read_text(file)
        out = {"file": text}
        lines = [line for line in text.splitlines() if line.strip()]
        out["spdx"] = bool(lines) and lines[0].strip() == "// SPDX-License-Identifier: MIT"
        out["named"] = bool(text) and "export default" not in text
        docs = jsdoc_first_lines(text)
        exported = len(re.findall(r"\bexport\s+(?:async\s+)?function\b", text))
        out["jsdoc_lines"] = docs
        out["jsdoc"] = exported == 3 and len(docs) == 3 and all(d.strip().startswith("Since 0.6:") for d in docs) \
            and "Since 0.5:" not in text
        probe = ("const m = await import(process.argv[1]);"
                 "console.log(JSON.stringify({inv: m.formatInvoiceNumber(42),"
                 " date: m.formatDate(new Date(Date.UTC(2026, 0, 5, 12))), row: m.csvRow(['a', 'b'])}));")
        try:
            ran = subprocess.run(["node", "--input-type=module", "-e", probe, str(file)], capture_output=True,
                                 text=True, timeout=20, cwd=self.root)
            out["ran"] = json.loads(ran.stdout.strip().splitlines()[-1]) if ran.returncode == 0 else {"error": ran.stderr[-400:]}
        except (OSError, ValueError, IndexError, subprocess.TimeoutExpired) as error:
            out["ran"] = {"error": str(error)}
        ran = out["ran"]
        out["padding"] = ran.get("inv") == "000042"
        out["dates"] = ran.get("date") == "2026-01-05"
        out["separator_used"] = {"a,b": ",", "a;b": ";"}.get(ran.get("row"))
        return out

    def capture(self):
        super().capture()
        ledger = battery.read_text(self.record() / "ledger.md")
        self.result["ledger"] = ledger
        self.result["ledger_archive"] = battery.read_text(self.record() / "ledger-archive.md")
        commands = [(e["entry"].get("event") or {}).get("command") or "" for e in self.result.get("provider_events", [])]
        self.result["ledger_commands"] = sorted({c for c in commands if "brigadier ledger" in c})


def jsdoc_first_lines(text):
    """The first nonempty documentation line before each exported function, inline or multiline."""
    blocks = re.findall(r"/\*\*(.*?)\*/\s*export\s+(?:async\s+)?function\b", text, re.S)
    return [next((line.strip().removeprefix("*").strip() for line in block.splitlines()
                  if line.strip().removeprefix("*").strip()), "") for block in blocks]


def separator(text):
    """The separator a reply or ledger entry names first ("I chose a comma … not semicolons"):
    ',' or ';' or None."""
    found = re.search(r"semicolon|comma", text, re.I)
    return {"comma": ",", "semicolon": ";"}.get(found.group(0).lower()) if found else None


def own_decision(result):
    """T5: the plant reply told the choice, and a ledger entry marks it "I decided"."""
    told = separator(result.get("plant_replies", {}).get("plant-own-decision", ""))
    entries = [line for line in result.get("ledger", "").splitlines()
               if re.search(r"I decided", line, re.I) and re.search(r"separator|delimiter", line, re.I)]
    marked = bool(entries)
    return {"told": told, "marked": marked, "entry": entries[0] if entries else None,
            "with_id": bool(entries) and bool(re.match(r"\s*[-*+]\s*L\d+ · ", entries[0]))}


def principle(answer):
    parsed = battery.parse_answer(answer)
    a, b = str(parsed.get("a", "")).lower(), str(parsed.get("b", "")).lower()
    def response(value, expected):
        if re.search(r"no idea|not sure|unknown|uncertain|maybe", value):
            return False
        definite = bool(re.match(expected + r"(?:$|[,.!:\s])", value))
        relevant_question = "?" in value and bool(re.search(r"user|notice|visible|behavio[u]?r|impact|benefit|internal|warning", value))
        return definite or relevant_question
    return {"a": a, "b": b, "pass": response(a, "no") and response(b, "yes")}


def raised(answer):
    return {"open_question": open_item_raised(answer, r"\bVAT\b"),
            "open_promise": open_item_raised(answer, r"\bREADME\b")}


def judge(brief, sep, model, provider="claude", effort="high"):
    """T3: a model reads the fixed rubric. A report, not proof (plan §8 item 16)."""
    if not brief.strip():
        return {"error": "no brief.md"}
    prompt = RUBRIC.format(separator={",": "a comma", ";": "a semicolon"}.get(sep, "whatever the thread chose"),
                           brief=brief)
    try:
        command = (["claude", "-p", "--model", model, "--effort", effort, "--no-session-persistence", "--output-format", "text"]
                   if provider == "claude" else ["codex", "exec", "--model", model,
                   "-c", 'model_reasoning_effort="high"', "--skip-git-repo-check", "-"])
        out = subprocess.run(command, input=prompt,
                             capture_output=True, text=True, timeout=300, cwd="/tmp")
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"error": str(error)}
    parsed = battery.parse_answer(out.stdout)
    return parsed or {"error": (out.stdout or out.stderr)[-400:]}


def score(result, judge_model):
    t1 = result.get("t1") or {}
    own = own_decision(result)
    t2 = principle(result.get("t2", ""))
    t4 = raised(result.get("t4", ""))
    sep = own["told"]
    t3 = result.get("t3") or judge(result.get("brief", ""), sep, judge_model, result.get("provider", "claude"), result.get("effort", "high"))
    result["t3"] = t3
    recall = battery.parse_answer(result.get("t6", ""))
    rows = {}
    for key, description in ITEMS.items():
        value = str(recall.get(key, "unknown"))
        recalled = all(re.search(p, value, re.I) for p in RECALL[key])
        if key == "decision_padding":
            recalled = battery.invoice_decision(value)
        if key == "worker_claim":
            recalled = bool(re.search(r"writeFileSync", value)) and battery.definite_rejection(value)
        if key == "rule_exports":
            recalled = recalled and bool(re.search(r"never|no\s+(?:export\s+)?default|not\s+(?:export\s+)?default|forbid", value, re.I))
        unprompted = {
            "rule_jsdoc": ("T1", t1.get("jsdoc")),
            "rule_exports": ("T1", t1.get("named")),
            "rule_spdx": ("T1", t1.get("spdx")),
            "principle": ("T2", t2["pass"]),
            "decision_padding": ("T1", t1.get("padding")),
            "decision_dates": ("T1", t1.get("dates")),
            "own_decision": ("T1+T5", bool(sep) and t1.get("separator_used") == sep and (own["marked"] or result.get("arm") in ("plain", "zed", "summary"))),
            "open_question": ("T4", t4["open_question"]),
            "open_promise": ("T4", t4["open_promise"]),
            "worker_claim": ("—", None),
        }[key]
        rows[key] = {"description": description, "check": unprompted[0], "unprompted": unprompted[1],
                     "recalled": recalled, "answer": value}
    result["rows"] = rows
    result["t2_scored"], result["t4_scored"], result["t5"] = t2, t4, own
    return rows


def verdict(value):
    return "n/a" if value is None else "pass" if value else "fail"


def cell(text):
    return str(text).replace("|", "\\|").replace("\n", " ")


def report(result, judge_model):
    rows = result.get("rows") or score(result, judge_model)
    applies = [r for r in rows.values() if r["unprompted"] is not None]
    lines = ["# Decision survival (S22b) — " + result["started_at"][:10], "",
             "Report only; no result gates a session (plan §5). The baseline of `research/CONTINUITY.md` §3.7.", "",
             f"Source: `{result['source']}`; daemon SHA-256: `{result['daemon_sha256']}`.",
             f"{result.get('provider', 'claude')} `{result['model']}`, {result.get('effort', 'high')} effort; {result.get('arm', 'seam')} arm; one measured lineage; size threshold "
             f"`{result['threshold']}` (seam, zed and summary arms). T3 judge: `{result.get('provider', 'claude')}/{judge_model}`.",
             "Ten items were planted as a user gives them, with no word about the ledger; nothing was restated "
             "before the checks.", ""]
    if rows:
        lines += [f"Applied or raised unprompted where they apply: {sum(bool(r['unprompted']) for r in applies)}"
                  f" of {len(applies)}. Recalled when asked (T6): {sum(r['recalled'] for r in rows.values())} of {len(rows)}.", "",
                  "| Item | Unprompted check | Unprompted | Recalled (T6) | T6 answer |", "|---|---|---|---|---|"]
        for row in rows.values():
            lines.append(f"| {row['description']} | {row['check']} | {verdict(row['unprompted'])} | "
                         f"{verdict(row['recalled'])} | {cell(row['answer'])} |")
    t1, t3, own = result.get("t1") or {}, result.get("t3") or {}, result.get("t5") or {}
    lines += ["", "## Checks", "",
              f"- **T1 applied unprompted** (`src/export/csv.ts`): SPDX first line {verdict(t1.get('spdx'))}; "
              f"named exports {verdict(t1.get('named'))}; JSDoc 'Since 0.6:' on all three {verdict(t1.get('jsdoc'))} "
              f"(first lines: {cell(t1.get('jsdoc_lines'))}); ran: `{cell(json.dumps(t1.get('ran')))}`.",
              f"- **T2 the principle**: (a) refactor → `{cell(result.get('t2_scored', {}).get('a'))}`, (b) Excel fix → "
              f"`{cell(result.get('t2_scored', {}).get('b'))}`: {verdict(result.get('t2_scored', {}).get('pass'))}.",
              f"- **T3 derived brief**: `{cell(json.dumps(t3.get('items', t3)))}`; unmarked additions: "
              f"{len(t3.get('unmarked_additions', []))}" + "".join(f"; \"{cell(a)}\"" for a in t3.get("unmarked_additions", [])[:6]) +
              f". Brief bytes: {len(result.get('brief', '').encode())}; ledger ids cited in it: "
              f"{len(set(re.findall(r'(?<![A-Za-z0-9_])L[0-9]+(?![A-Za-z0-9_])', result.get('brief', ''))))}.",
              f"- **T4 open items raised** on \"{T4}\": VAT question {verdict(result.get('t4_scored', {}).get('open_question'))}, "
              f"README promise {verdict(result.get('t4_scored', {}).get('open_promise'))}.",
              f"- **T5 own decision told and marked**: told `{own.get('told')}`; ledger \"I decided\" entry "
              f"{verdict(own.get('marked'))} (with an id: {verdict(own.get('with_id'))}): `{cell(own.get('entry'))}`.",
              "- **T6 recall when asked**: the table's last two columns."]
    if t3.get("items"):
        values = list(t3["items"].values())
        lines += ["", f"Brief bar (0 contradictions, 0 unmarked additions, ≤ 1 missing): contradicted "
                  f"{values.count('contradicted')}, unmarked {len(t3.get('unmarked_additions', []))}, missing {values.count('missing')}."]
    ledger = result.get("ledger", "")
    entries = [line for line in ledger.splitlines() if re.match(r"\s*[-*+] ", line)]
    with_id = [line for line in entries if re.match(r"\s*[-*+] L\d+ · ", line)]
    lines += ["", "## Ledger and seams", "",
              f"- Ledger: {len(entries)} entries, {len(with_id)} with an id; "
              f"\"Took over\" entries: {sum('Took over' in line for line in entries)}; closed: "
              f"{sum(bool(re.search(r'· Closed L', line)) for line in entries)}; archive bytes: {len(result.get('ledger_archive', '').encode())}.",
              "- `brigadier ledger` commands the thread ran: " + (", ".join(f"`{cell(c)[:80]}`" for c in result.get("ledger_commands", [])) or "none"),
              f"- Context transitions: {result.get('size_seams_before_faults', 0)} by size or native compaction; forced death only in the seam arm "
              f"(seams closed before the checks: {result.get('seams_before_checks', 'unavailable')}).", "",
              "| Seam | Death | Note bytes | Hand-off bytes | Open items in it | Rules and decisions in it |",
              "|---|---|---:|---:|---:|---:|"]
    for handoff in result.get("handoffs", []):
        start = handoff.get("start", "")
        def count(title):
            part = start.partition(f"## {title}, verbatim and whole\n")[2].partition("\n## ")[0]
            return sum(1 for line in part.splitlines() if re.match(r"\s*[-*+] |\S", line) and line != "(none)")
        lines.append(f"| {handoff['n']} | {handoff.get('death') or ''} | {len(handoff.get('note', '').encode())} | "
                     f"{len(start.encode())} | {count('Open items')} | {count('Rules and decisions')} |")
    lines += ["", "Limits: one short debug-threshold run on one provider. The nightly summary compares both providers with their plain-CLI arms (S25/L9). T3 is a model reading a rubric: a report, not proof. "
              "T1's JSDoc and T2's answers are read by regular expressions; the evidence keeps the raw text.",
              "", "Run problems: " + ("; ".join(result["errors"]) or "none"),
              "Cleanup: " + ("; ".join(map(str, result.get("cleanup", []))) or "own session, project, daemon and scratch root removed"), ""]
    lines += battery.arm_markdown(result)
    lines += battery.economics_markdown(result)
    return "\n".join(lines)


def rescore(evidence, report_path, judge_model):
    """Scores and writes the report again from saved evidence; the T3 judge's answer is reused."""
    result = json.loads(Path(evidence).read_text())
    result.pop("rows", None)
    score(result, judge_model)
    Path(evidence).write_text(json.dumps(result, indent=2))
    battery.write_report(result, report(result, judge_model), report_path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon", required=True)
    parser.add_argument("--model", default=None)
    parser.add_argument("--provider", choices=["claude", "codex"], default="claude")
    from arms import ARMS, configure, make
    parser.add_argument("--arm", choices=ARMS, default="seam")
    parser.add_argument("--fair", action="store_true", help="opt-in shared threshold and identical lookup access")
    parser.add_argument("--min-fillers", type=int, default=0)
    parser.add_argument("--judge-model", default=None)
    parser.add_argument("--tokens", type=int, default=None, help="the debug seam threshold (BRIGADIER_SEAM_TOKENS)")
    parser.add_argument("--max-fillers", type=int, default=24)
    parser.add_argument("--ballast", type=int, default=2000, help="inert words per filler")
    parser.add_argument("--timeout", type=int, default=3000)
    parser.add_argument("--turn-timeout", type=int, default=180)
    parser.add_argument("--report", help="the text report (default reports/<date>-claude-decisions.md)")
    parser.add_argument("--evidence", help="optional private JSON evidence, outside the repo")
    parser.add_argument("--scratch-prefix", default="continuity-decisions", help="names the /tmp scratch root")
    args = parser.parse_args()
    configure(args, parser, 60000)
    args.model = args.model or ("opus" if args.provider == "claude" else "gpt-6.1-sol")
    args.judge_model = args.judge_model or ("sonnet" if args.provider == "claude" else args.model)
    if not args.report:
        args.report = str(Path(__file__).parent / "reports" / (time.strftime("%Y-%m-%d") + f"-{args.provider}-{args.arm}-decisions.md"))
    if "fable" in (args.model + args.judge_model).lower() or args.tokens <= 0:
        parser.error("use permitted models and a positive threshold")
    if str(Path(args.daemon).resolve()).startswith("/Applications/"):
        parser.error("use a debug daemon built from this checkout")
    if args.evidence and Path(args.evidence).resolve().is_relative_to(battery.SRC):
        parser.error("keep detailed evidence outside the repository")
    run = make(Decisions, args)
    run.result["block"] = "decisions"
    def terminated(signum, frame):
        raise TimeoutError("nightly supervisor stopped the run")
    battery.signal.signal(battery.signal.SIGTERM, terminated)
    try:
        run.run()
    except Exception as error:
        battery.log(f"run problem: {error}")
        from zed import SkippedArm
        if isinstance(error, SkippedArm):
            run.result.update(status="skipped", reason=str(error))
        else:
            run.errors.append(str(error))
    finally:
        try:
            run.capture()
        except Exception as error:
            run.errors.append("capture: " + str(error))
        try:
            if "items" in run.result:
                score(run.result, args.judge_model)
        except Exception as error:
            run.errors.append("score: " + str(error))
        run.cleanup()
        Path(args.report).parent.mkdir(parents=True, exist_ok=True)
        battery.write_report(run.result, report(run.result, args.judge_model), args.report)
        battery.log("report: " + args.report)
    return 0 if "items" in run.result or run.result.get("status") == "skipped" else 2


if __name__ == "__main__":
    sys.exit(main())
