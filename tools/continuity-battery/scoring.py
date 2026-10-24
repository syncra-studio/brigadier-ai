"""Conservative deterministic checks shared by the existing scorers."""
import re


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
