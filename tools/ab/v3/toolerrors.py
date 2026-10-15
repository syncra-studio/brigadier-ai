#!/usr/bin/env python3
"""Tool calls the thread got wrong, per V3 run (BUILD-PLAN C1: "V3 counts invalid tool calls, with no rise").
Reads the Claude transcripts a run kept (<run>/transcripts/claude/*.jsonl) and counts, per run:
  calls    tool calls made;
  invalid  calls the CLI or Brigadier refused for their input: an unknown tool, a schema or argument error
           (`<tool_use_error>`, `InputValidationError`, "invalid arguments", "missing field", "unknown field",
           "No such tool");
  errors   every tool result marked as an error (invalid ones, failing commands, refusals).
usage: toolerrors.py <run-dir> [<run-dir> ...]   (prints one JSON line per run, then the totals)"""
import glob, json, os, re, sys

INVALID = re.compile(r"<tool_use_error>|InputValidationError|invalid arguments|missing field|unknown field|"
                     r"No such tool|is not a valid|unknown variant", re.I)


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(c.get("text", "") for c in content if isinstance(c, dict))
    return ""


def count(run):
    calls = invalid = errors = 0
    examples = []
    for path in sorted(glob.glob(os.path.join(run, "transcripts", "claude", "*.jsonl"))):
        for line in open(path):
            try:
                event = json.loads(line)
            except ValueError:
                continue
            message = event.get("message") or {}
            content = message.get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                if event.get("type") == "assistant" and block.get("type") == "tool_use":
                    calls += 1
                if event.get("type") == "user" and block.get("type") == "tool_result" and block.get("is_error"):
                    errors += 1
                    text = text_of(block.get("content"))
                    if INVALID.search(text):
                        invalid += 1
                        examples.append(text[:160])
    return {"run": os.path.basename(run.rstrip("/")), "calls": calls, "invalid": invalid, "errors": errors,
            "invalid_examples": examples}


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    total = {"calls": 0, "invalid": 0, "errors": 0}
    for run in sys.argv[1:]:
        row = count(run)
        print(json.dumps(row))
        for key in total:
            total[key] += row[key]
    print(json.dumps({"total": total}))


if __name__ == "__main__":
    main()
