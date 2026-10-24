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
