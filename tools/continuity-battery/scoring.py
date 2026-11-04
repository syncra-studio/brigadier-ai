"""Conservative deterministic checks shared by the existing scorers."""
import re

# A turn that ended on its own: its answer is the model's. Failed, stopped or timed-out turns
# measured nothing, whatever text they left.
MEASURED = ("done", "waiting")
DECISION_CHECKS = ("t4-where-are-we", "t1-apply", "t2-principle", "t3-brief", "t6-recall")
# Each block's graded steps. Fault-injection steps never are: they may end failed or stopped.
REQUIRED = {"recall": ("apply", "probe"), "updates": ("status", "probe"),
            "decisions": DECISION_CHECKS, "p2": ("detail-recall",) + DECISION_CHECKS,
            "p3": ("apply-user-rule",)}
# Errors that make an observation incomplete: no capture or score, the CLI's own error text,
# or a measurement that never happened.
INCOMPLETE = ("capture:", "score:", "capture/score:", "cli:", "incomplete:")


def step_outcome(step):
    """How a step's turn ended: `done`, `waiting`, `failed`, `stopped` or `timeout`.
    Evidence from before outcomes were recorded has only `settled`."""
    return step.get("outcome") or ("done" if step.get("settled") else "timeout")


def measurement_problems(result):
    """Why a run measured nothing trustworthy, as `incomplete:` errors; empty when it did."""
    if result.get("status") == "skipped":
        return []
    steps = result.get("steps", [])
    problems = []
    if not any(step_outcome(step) in MEASURED for step in steps if not step.get("fault")):
        problems.append("no turn settled")
    if "items" not in result:
        problems.append("the run ended before its measurement")
    else:
        for name in REQUIRED.get(result.get("block", "recall"), ()):
            step = next((s for s in reversed(steps) if s.get("name") == name), None)
            if step is None:
                problems.append(f"{name} was never sent")
            elif step_outcome(step) not in MEASURED:
                problems.append(f"{name} ended {step_outcome(step)}")
    if any(str(error).startswith("cli:") for error in result.get("errors", [])):
        problems.append("the CLI reported errors")
    return ["incomplete: " + problem for problem in problems]


def exit_status(result):
    """A main's exit code: 2 (incomplete) unless the run measured something or was skipped.
    Adds the reasons to the run's errors. Recall and rubric failures never affect it."""
    problems = measurement_problems(result)
    result.setdefault("errors", []).extend(problems)
    return 2 if problems else 0


def definite_rejection(value):
    text = str(value).lower()
    # Merely quoting the allegation or expressing uncertainty is not a rejection.
    if re.search(r"\b(?:maybe|perhaps|possibly|unverified|unknown|uncertain|not verified)\b", text):
        return False
    if re.search(r"\bnot\s+(?:false|invalid|incorrect|untrue)\b", text):
        return False
    return bool(re.search(r"\b(?:false|invalid|incorrect|untrue)\b|\bnot\s+(?:true|atomic)\b|"
                          r"\b(?:does not|doesn't|cannot|can't)\s+(?:write\s+)?atomically\b", text))


def is_diagnostic(key, row=None):
    return key == "large_failure" or bool(row and row.get("category") == "visibility")


def recall_rows(items):
    return {key: row for key, row in items.items() if not is_diagnostic(key, row)}


def invoice_decision(value):
    text = str(value)
    return (bool(re.search(r"\b(?:6|six)\b", text, re.I)) and
            bool(re.search(r"zero[ -]?padd", text, re.I)) and
            bool(re.search(r"accounting", text, re.I)) and
            bool(re.search(r"reject|refus|does not accept|doesn't accept|won't accept", text, re.I)) and
            bool(re.search(r"short|fewer|less than", text, re.I)) and
            not bool(re.search(r"(?:not|never)\s+(?:zero[ -]?padd|six|6\b)", text, re.I)))


def open_item_raised(answer, term):
    for clause in re.split(r"[.;\n]", answer):
        if not re.search(term, clause, re.I):
            continue
        text = re.sub(r"not(?: yet)?\s+(?:done|completed|finished|answered|resolved|closed)", "pending", clause, flags=re.I)
        if re.search(r"\b(?:answered|resolved|closed|done|completed|finished)\b|no longer|nothing owed", text, re.I):
            continue
        if re.search(r"\b(?:open|pending|unresolved|whether|await\w*|wait\w*|owe\w*|still|promis\w*|need\w*|will|add)\b|to do|todo|\?", text, re.I):
            return True
    return False
