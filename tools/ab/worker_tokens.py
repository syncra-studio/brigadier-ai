#!/usr/bin/env python3
"""Per-worker token and habit metrics of one conversation, from a Brigadier data dir
(SESSION-FIXES-PLAN.md §3 Q6.7). Rebuilt from routing.sqlite `turn_usage` and the worker event
streams in brigadier.db, opened read-only (`?mode=ro`), so it is safe on any data dir. A data dir
whose daemon has stopped can be read with `--snapshot` (SQLite's `immutable=1`), never a live one.

Per worker (a task; reviews and the thread are listed apart):
- calls: model calls, one `contextSize` event each (a Claude turn's closing repeat is dropped);
- start context (the first call's) and average context;
- tokens from `turn_usage`: input (uncached), cached, cache writes, output, per provider and model;
- Brain: `query_brain` calls, facts in the brief (the "What the Brain knows about this" section of
  its first message) and facts written back (Brain nodes sourced from the task, when the
  project's brain.sqlite is in the data dir);
- `code_search`/`code_refs` calls, shell commands, `run_check` calls and the cache hits of the
  checks it ran.
Then the conversation's totals per provider over everything it ran (thread, workers, reviews and
its project's Brain jobs in the window), and the elapsed time from the request to its answer.

usage: worker_tokens.py <arm-dir> [end_ms] [--snapshot] [--json out]
         (an A/B arm: <arm>/data, conversation and t0 from <arm>/start.json)
       worker_tokens.py --data DIR --conversation ID [--from MS] [--to MS] [--snapshot] [--json out]"""
import argparse, collections, glob, json, os, sqlite3, sys

FIELDS = ("input", "cached_input", "cache_write", "output")
BRAIN_HEADING = "What the Brain knows about this"


def connect(path, snapshot):
    """Read-only. `immutable=1` (no locking, no change detection) only for a snapshot whose
    daemon has stopped (`--snapshot`); a live database that can't be opened `?mode=ro` fails."""
    if snapshot:
        return sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        db.execute("PRAGMA schema_version").fetchone()
    except sqlite3.OperationalError as error:
        sys.exit(f"worker_tokens: can't read {path} read-only ({error}); if its daemon has stopped, "
                 "pass --snapshot")
    return db


def short(name):
    """A tool's bare name: `mcp__brigadier__run_check` and `brigadier/run_check` are `run_check`."""
    return name.rsplit("__", 1)[-1].rsplit("/", 1)[-1]


def brief_facts(text):
    """The fact lines of the brief's Brain section."""
    count, inside = 0, False
    for line in text.splitlines():
        stripped = line.strip()
        if BRAIN_HEADING in stripped:
            inside = True
        elif inside and (stripped.startswith("#") or (stripped.startswith("[") and stripped.endswith("]"))):
            inside = False
        elif inside and stripped.startswith("- "):
            count += 1
    return count


def tokens(c):
    raw = sum(c[f] for f in FIELDS)
    return {"input_uncached": c["input"], "cache_read": c["cached_input"], "cache_write": c["cache_write"],
            "output": c["output"], "raw": raw, "raw_without_cache_reads": raw - c["cached_input"],
            "turns": c["turns"]}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("arm", nargs="?")
    ap.add_argument("end", nargs="?", type=int)
    ap.add_argument("--data")
    ap.add_argument("--conversation")
    ap.add_argument("--from", dest="start", type=int)
    ap.add_argument("--to", type=int)
    ap.add_argument("--snapshot", action="store_true", help="the data dir's daemon has stopped")
    ap.add_argument("--json")
    a = ap.parse_args()
    if a.arm:
        st = json.load(open(os.path.join(a.arm, "start.json")))
        data, conv, start, end = os.path.join(a.arm, "data"), st["conversation"], st["t0_ms"], a.end
    elif a.data and a.conversation:
        data, conv, start, end = a.data, a.conversation, a.start, a.to
    else:
        ap.error("give an arm dir, or --data and --conversation")
    start = start or 0
    end = end or 2**62

    events = connect(os.path.join(data, "brigadier.db"), a.snapshot)
    usage = connect(os.path.join(data, "routing.sqlite"), a.snapshot)

    # Usage rows: the thread (no task), reviews (`review:` ids) and workers (tasks).
    rows = collections.defaultdict(collections.Counter)
    by_provider = collections.defaultdict(collections.Counter)
    projects = set()
    for provider, model, task, project, *vals in usage.execute(
            f"SELECT provider, model, task_id, project_id, {', '.join(FIELDS)} FROM turn_usage "
            "WHERE conversation_id = ? AND at_ms BETWEEN ? AND ?", (conv, start, end)):
        c = collections.Counter(dict(zip(FIELDS, vals)), turns=1)
        role = "thread" if not task else "review" if task.startswith("review:") else "worker"
        rows[(role, task or "", provider, model)].update(c)
        by_provider[provider].update(c)
        if project:
            projects.add(project)
    background = collections.defaultdict(collections.Counter)
    for project in projects:  # the project's Brain jobs and other upkeep in the window
        for provider, *vals in usage.execute(
                f"SELECT provider, {', '.join(FIELDS)} FROM turn_usage WHERE conversation_id IS NULL "
                "AND project_id = ? AND at_ms BETWEEN ? AND ?", (project, start, end)):
            c = collections.Counter(dict(zip(FIELDS, vals)), turns=1)
            background[provider].update(c)
            by_provider[provider].update(c)

    # Tasks of the conversation, in order, with title and number.
    tasks = {}
    for (payload,) in events.execute(
            "SELECT payload FROM events WHERE stream = ? AND kind = 'task.updated' AND at_ms <= ? ORDER BY seq",
            (f"conversation:{conv}", end)):
        t = json.loads(payload)["task"]
        tasks[t["id"]] = {"number": t.get("number"), "title": t.get("title"), "kind": t.get("kind"),
                          "subject": t.get("subject")}
    for _, task, _, _ in rows:
        if task and not task.startswith("review:"):
            tasks.setdefault(task, {"number": None, "title": None, "kind": None, "subject": None})

    hits = collections.Counter()
    for (payload,) in events.execute(
            "SELECT payload FROM events WHERE stream = ? AND kind = 'check.ran' AND at_ms BETWEEN ? AND ?",
            (f"conversation:{conv}", start, end)):
        e = json.loads(payload)
        if e.get("taskId") and e.get("cached"):
            hits[e["taskId"]] += 1

    brains = glob.glob(os.path.join(data, "brains", "*", "brain.sqlite"))
    learned = collections.Counter()
    for path in brains:
        brain = connect(path, a.snapshot)
        for task, n in brain.execute("SELECT task_id, COUNT(DISTINCT node_id) FROM node_sources "
                                     "WHERE task_id IS NOT NULL GROUP BY task_id"):
            learned[task] += n

    workers = []
    for task, info in tasks.items():
        contexts, tools, commands, facts_in, first_message = [], collections.defaultdict(set), set(), 0, True
        previous = None
        for (payload,) in events.execute(
                "SELECT payload FROM events WHERE stream = ? AND kind = 'worker.event' AND at_ms BETWEEN ? AND ? "
                "ORDER BY seq", (f"task:{task}", start, end)):
            e = json.loads(payload)["event"]
            kind = e.get("type")
            if kind == "contextSize":
                used = e.get("usedTokens")
                if used is not None and used != previous:
                    contexts.append(used)
                previous = used
            elif kind == "sessionStarted":
                previous = None
            elif kind == "toolCall" and e.get("name"):
                tools[short(e["name"])].add(e.get("itemId"))
            elif kind == "command":
                commands.add(e.get("itemId"))
            elif kind == "message" and e.get("role") == "user" and first_message:
                facts_in, first_message = brief_facts(e.get("text") or ""), False
        usage_rows = {f"{provider}:{model}": tokens(c) for (role, t, provider, model), c in rows.items()
                      if role == "worker" and t == task}
        total = collections.Counter()
        for (role, t, _, _), c in rows.items():
            if role == "worker" and t == task:
                total.update(c)
        workers.append({
            "task": task, "number": info["number"], "title": info["title"], "kind": info["kind"],
            "subject": info["subject"],
            "calls": len(contexts),
            "start_context": contexts[0] if contexts else None,
            "average_context": round(sum(contexts) / len(contexts)) if contexts else None,
            "tokens": tokens(total), "by_model": usage_rows,
            "brain_queries": len(tools.get("query_brain", ())),
            "brain_facts_in": facts_in,
            "brain_facts_out": learned.get(task, 0) if brains else "no brain store",
            "code_search": len(tools.get("code_search", ())) + len(tools.get("code_refs", ())),
            "shell_commands": len(commands),
            "run_check": len(tools.get("run_check", ())),
            "run_check_cache_hits": hits.get(task, 0),
        })
    workers = [w for w in workers if w["calls"] or w["tokens"]["turns"]]

    # The request: the first one started at or after the window's start, to its answer.
    request = None
    for (payload,) in events.execute(
            "SELECT payload FROM events WHERE stream = ? AND kind = 'request.updated' ORDER BY seq",
            (f"conversation:{conv}",)):
        r = json.loads(payload)["request"]
        if request is None and r["startedAtMs"] >= start - 2000:
            request = {"id": r["id"], "started_ms": r["startedAtMs"], "answer_ms": None}
        if request and r["id"] == request["id"] and r.get("endedAtMs"):
            request["answer_ms"] = r["endedAtMs"]
    if request and request["answer_ms"]:
        request["elapsed_s"] = round((request["answer_ms"] - request["started_ms"]) / 1000, 1)

    def role_total(role):
        c = collections.Counter()
        for (r, _, _, _), v in rows.items():
            if r == role:
                c.update(v)
        return tokens(c)

    worker_total = role_total("worker")
    result = {
        "conversation": conv, "window_ms": [start, end if end < 2**62 else None], "request": request,
        "workers": workers,
        "workers_total": {**worker_total,
                          "calls": sum(w["calls"] for w in workers),
                          "brain_queries": sum(w["brain_queries"] for w in workers),
                          "run_check": sum(w["run_check"] for w in workers),
                          "run_check_cache_hits": sum(w["run_check_cache_hits"] for w in workers)},
        "thread": role_total("thread"), "reviews": role_total("review"),
        "brain_jobs": {p: tokens(c) for p, c in background.items()},
        "by_provider": {p: tokens(c) for p, c in by_provider.items()},
    }
    for provider, v in result["by_provider"].items():
        if provider == "codex" and v["cache_write"] == 0:  # Codex reports cache writes only when it has them
            v["cache_write"] = "not reported"
    result["total"] = {k: sum(v[k] for v in result["by_provider"].values())
                       for k in ("raw", "raw_without_cache_reads")}

    print(f"{'task':>5} {'calls':>5} {'start':>7} {'avg ctx':>8} {'raw':>11} {'no-cache':>9} "
          f"{'brain q':>7} {'in':>3} {'out':>3} {'search':>6} {'shell':>5} {'checks':>6} {'hits':>4}  title")
    for w in workers:
        print(f"{w['number'] or '-':>5} {w['calls']:>5} {w['start_context'] or '-':>7} {w['average_context'] or '-':>8} "
              f"{w['tokens']['raw']:>11,} {w['tokens']['raw_without_cache_reads']:>9,} {w['brain_queries']:>7} "
              f"{w['brain_facts_in']:>3} {w['brain_facts_out'] if brains else '-':>3} {w['code_search']:>6} "
              f"{w['shell_commands']:>5} {w['run_check']:>6} {w['run_check_cache_hits']:>4}  {w['title'] or w['task']}")
    t = result["workers_total"]
    print(f"workers: {len(workers)}, calls {t['calls']}, raw {t['raw']:,}, without cache reads "
          f"{t['raw_without_cache_reads']:,}, brain queries {t['brain_queries']}, run_check {t['run_check']} "
          f"({t['run_check_cache_hits']} cached)")
    for provider, v in result["by_provider"].items():
        print(f"{provider}: input {v['input_uncached']:,}, cached {v['cache_read']:,}, cache writes "
              f"{v['cache_write'] if isinstance(v['cache_write'], str) else format(v['cache_write'], ',')}, output {v['output']:,}, raw {v['raw']:,}")
    if request:
        print(f"request {request['id']}: elapsed {request.get('elapsed_s', 'no answer yet')} s")
    if a.json:
        json.dump(result, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    sys.exit(main())
