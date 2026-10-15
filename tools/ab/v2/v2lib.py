#!/usr/bin/env python3
"""Shared pieces of the V2 harness: the battery file format."""
import json, os, re

HERE = os.path.dirname(os.path.abspath(__file__))


def load_battery(name_or_path):
    """A battery file (`battery/<name>.md`, or a run's frozen `battery.md`): `### <step> <role>`
    headings each with one ```text request, a ```json block under `## Harness` and one under
    `## Items`."""
    path = name_or_path if name_or_path.endswith(".md") else os.path.join(HERE, "battery", f"{name_or_path}.md")
    text = open(path).read()
    steps = [{"id": m.group(1), "role": m.group(2), "text": m.group(3).strip()}
             for m in re.finditer(r"\n### (\w+) (\w+)\n```text\n(.*?)\n```", text, re.S)]
    harness = re.search(r"\n## Harness\n.*?```json\n(.*?)\n```", text, re.S)
    items = re.search(r"\n## Items\n.*?```json\n(.*?)\n```", text, re.S)
    timeout = re.search(r"^timeout: (\d+)$", text, re.M)
    if not steps or not harness or not items:
        raise SystemExit(f"{path}: a battery needs steps, a Harness block and an Items block")
    name = re.search(r"^id: (\S+)$", text, re.M)
    b = {"name": name.group(1) if name else os.path.basename(path)[:-3], "path": os.path.abspath(path), "steps": steps,
         "harness": json.loads(harness.group(1)), "items": json.loads(items.group(1)),
         "timeout": int(timeout.group(1)) if timeout else 5400}
    ids = [s["id"] for s in steps]
    for item in b["items"]:
        for key in ("source", "acted"):
            if item.get(key) and item[key] not in ids:
                raise SystemExit(f"{path}: item {item['id']}: unknown step {item[key]}")
    return b
