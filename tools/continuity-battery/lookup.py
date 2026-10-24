#!/usr/bin/env python3
"""Read-only access to the run's native record, identical in every comparison arm."""
import argparse
import json
from pathlib import Path


def query(path, action, value):
    items = json.loads(Path(path).read_text())
    if action == "read":
        return [item for item in items if item["id"] == value]
    return [item for item in items if value.casefold() in item["raw"].casefold()]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive")
    parser.add_argument("action", choices=["search", "read"])
    parser.add_argument("value")
    args = parser.parse_args()
    for item in query(args.archive, args.action, args.value):
        # Print exact native row bytes. No summary, rewrite, grader data or future plants.
        print(json.dumps(item, ensure_ascii=False))
