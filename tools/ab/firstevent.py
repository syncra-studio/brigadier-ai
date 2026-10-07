#!/usr/bin/env python3
"""From `delegate_task` to each worker's first event (THREAD-PLAN §3 phase 3: ≤ 2 s).
For every task the arm's conversation created: the `created` step's time, then its first
`workerEvent` (any kind), its first model output (reasoning, text, a command or a tool call) and
its `running` task state, in ms after `created`. Recorded events only (`rec/events.jsonl`).
usage: firstevent.py <arm-dir> [--json]"""
import json, os, sys

arm = sys.argv[1]
as_json = "--json" in sys.argv[2:]
MODEL = {"reasoningDelta", "reasoning", "messageDelta", "command", "toolCall"}

created = {}  # task id -> ms
first, model, running, title = {}, {}, {}, {}
for line in open(os.path.join(arm, "rec", "events.jsonl")):
    env = json.loads(line)
    e, at = env["event"], env["atMs"]
    if e["type"] == "orchestratorStepped" and e["step"]["kind"]["type"] == "created":
        created.setdefault(e["step"]["kind"]["taskId"], e["step"]["atMs"])
    elif e["type"] == "taskUpdated":
        task = e["task"]
        title.setdefault(task["id"], task.get("title"))
        state = task.get("state")
        state = state.get("type") if isinstance(state, dict) else state
        if state == "running":
            running.setdefault(task["id"], at)
    elif e["type"] == "workerEvent":
        task = e["taskId"]
        first.setdefault(task, (at, e["event"]["type"]))
        if e["event"]["type"] in MODEL or (
                e["event"]["type"] == "message" and e["event"].get("role") == "assistant"):
            model.setdefault(task, at)

rows = []
for task, at in sorted(created.items(), key=lambda item: item[1]):
    ev = first.get(task)
    rows.append({
        "task": task, "title": title.get(task), "created_ms": at,
        "first_event_ms": None if ev is None else ev[0] - at,
        "first_event": None if ev is None else ev[1],
        "running_ms": None if task not in running else running[task] - at,
        "first_model_output_ms": None if task not in model else model[task] - at,
    })
if as_json:
    print(json.dumps(rows, indent=1))
else:
    for r in rows:
        print(f"{r['title']!r}: first event {r['first_event_ms']} ms ({r['first_event']}), "
              f"running {r['running_ms']} ms, first model output {r['first_model_output_ms']} ms")
