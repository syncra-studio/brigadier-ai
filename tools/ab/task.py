#!/usr/bin/env python3
"""Reads a frozen task file (tasks/*.md): the `key: value` lines before the first `##`, the
verbatim request (the first ```text block) and the check commands (the ```sh block).
usage: task.py <task.md> [key]  prints the task as JSON, or one key."""
import json, os, re, sys


def load_task(path):
    text = open(path).read()
    head = text.split("\n## ", 1)[0]
    task = {}
    for line in head.splitlines():
        m = re.match(r"^([a-z0-9-]+): (.+)$", line)
        if m:
            task[m.group(1)] = os.path.expanduser(m.group(2).strip())
    task["request"] = re.search(r"```text\n(.*?)\n```", text, re.S).group(1).strip()
    checks = re.search(r"## Check commands\n+```sh\n(.*?)\n```", text, re.S)
    task["checks"] = [l for l in checks.group(1).splitlines() if l and not l.startswith("#")] if checks else []
    return task


if __name__ == "__main__":
    t = load_task(sys.argv[1])
    print(t[sys.argv[2]] if len(sys.argv) > 2 else json.dumps(t, indent=1))
