#!/usr/bin/env python3
"""Check runs in an arm (THREAD-PLAN §3 phase 3: no check command runs twice on the same tree).
From `rec/events.jsonl`:
- every `checkRan` event (a `run_check` call): runs and cache hits per (tree, workdir, command);
  a key that *ran* twice is a repeat (a cache hit is not). Bypassed runs have no tree and are
  listed apart.
- check-like commands that went around `run_check`: a worker's `command` events and the thread's
  Bash/`run` tool steps whose command line runs a test, lint, type check, build or format check.
  Their tree is unknown, so they are listed, not counted as repeats.
Exit status 1 when a key ran twice.
usage: checkruns.py <arm-dir> [--json]"""
import collections, json, os, re, sys

arm = sys.argv[1]
CHECK = re.compile(
    r"\b(cargo\s+(test|clippy|check|build|fmt)|pnpm\b[^|;&]*\b(test|lint|typecheck|check|build|tsc|vitest)"
    r"|npm\s+(run\s+)?(test|lint|build)|tsc\b|vitest\b|eslint\b|biome\b|pytest\b|go\s+test)")

ran = collections.defaultdict(list)
hits = collections.Counter()
bypassed, outside, seen = [], [], set()
for line in open(os.path.join(arm, "rec", "events.jsonl")):
    env = json.loads(line)
    e = env["event"]
    if e["type"] == "checkRan":
        key = (e.get("tree"), e.get("workdir", ""), e["command"])
        who = e.get("taskId") or "thread"
        if e.get("bypassed"):
            bypassed.append({"who": who, "command": e["command"], "why": e["bypassed"], "status": e["status"]})
        elif e.get("cached"):
            hits[key] += 1
        else:
            ran[key].append({"who": who, "status": e["status"], "ms": e.get("durationMs"), "at": env["atMs"]})
    elif e["type"] == "workerEvent" and e["event"]["type"] == "command":
        command = e["event"].get("command") or ""
        item = e["event"].get("itemId")
        # A heredoc's body is data, not commands: only the command line before it counts.
        if CHECK.search(command.split("<<")[0]) and item not in seen:
            seen.add(item)
            outside.append({"who": e["taskId"], "command": command[:200], "at": env["atMs"]})
    elif e["type"] == "orchestratorStepped" and e["step"]["kind"]["type"] == "tool":
        kind = e["step"]["kind"]
        text = kind.get("detail") or ""
        if (kind.get("name") in ("Bash", "run") and CHECK.search(text.split("<<")[0])
                and kind.get("itemId") not in seen):
            seen.add(kind.get("itemId"))
            outside.append({"who": "thread", "command": text[:200], "at": env["atMs"]})

repeats = {k: v for k, v in ran.items() if len(v) > 1}
report = {
    "keys": len(set(ran) | set(hits)),
    "runs": sum(len(v) for v in ran.values()),
    "cache_hits": sum(hits.values()),
    "repeats": [{"tree": k[0], "workdir": k[1], "command": k[2], "runs": v} for k, v in repeats.items()],
    "bypassed": bypassed,
    "outside_run_check": outside,
}
if "--json" in sys.argv:
    print(json.dumps(report, indent=1))
else:
    print(f"run_check: {report['runs']} runs over {report['keys']} (tree, workdir, command) keys, "
          f"{report['cache_hits']} cache hits, {len(bypassed)} bypassed, {len(repeats)} repeated keys")
    for k, v in ran.items():
        print(f"  {k[0] and k[0][:10]} {k[1] or '.'} {k[2]!r}: ran {len(v)}x, hit {hits[k]}x")
    for b in bypassed:
        print(f"  bypassed: {b['command']!r} ({b['why']})")
    print(f"check-like commands outside run_check: {len(outside)}")
    for o in outside:
        print(f"  {o['who'][:8]}: {o['command']}")
sys.exit(1 if repeats else 0)
