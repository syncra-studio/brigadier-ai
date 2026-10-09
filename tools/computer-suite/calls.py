"""Model calls and the shortcut audit, from the worker transcripts recorder.py hard-linked into
<root>/transcripts (claude/<session>.jsonl and its subagents, codex/rollout-*<thread>.jsonl and
the rollouts of the threads it started).

Model calls (E1's numerator):
- Claude: distinct assistant message ids; one id per request, however many content blocks it
  streamed. Subagents' are counted too.
- Codex: distinct `response_id`s of its `token_usage_record` lines, one per model request. As a
  cross-check, the `token_count` events whose cumulative total differs from the one before (the
  same snapshot is often sent twice, so counting the events would double count). The rollouts of
  threads it started (`parent_thread_id`: an auto-review) are counted apart, as `child_calls`.
  Codex's code mode calls tools from JavaScript in an `exec` call (`tools.mcp__computer__act(...)`):
  an `exec` that calls only computer tools counts as a computer call.

The audit lists every tool call that isn't a computer tool. It fails the trial on a shortcut: a
call that writes to a path or name in `forbidden` (the trial's files, the fixture's log, the
daemon's data dir), or that reaches into Brigadier or the target app another way (its socket, the
helper's CLI, AppleScript, signals). A call that only reads one of them (`od` of the saved file,
the log copied to the worker's outputs) is a peek: listed, not failed, since the checker judges
the end state and the broker's records, never the worker's own check."""
import glob, json, os, re

REACH = ("brigadierd.sock", "ipc.token", "bipc", "brigadier-computer", "osascript", "kill ", "pkill",
         "defaults write", "routing.sqlite", "AXUIElement", "cliclick")


def files(tr, sessions):
    out = []
    for sid in sessions:
        out += [("claude", f) for f in glob.glob(os.path.join(tr, "claude", f"{sid}.jsonl"))]
        out += [("claude", f) for f in glob.glob(os.path.join(tr, "claude", sid, "subagents", "*.jsonl"))]
        out += [("codex", f) for f in glob.glob(os.path.join(tr, "codex", f"rollout-*{sid}.jsonl"))]
    return out


def lines(f):
    for l in open(f, errors="replace"):
        try:
            yield json.loads(l)
        except json.JSONDecodeError:
            continue


def codex_children(tr, sid):
    for f in glob.glob(os.path.join(tr, "codex", "rollout-*.jsonl")):
        if sid in os.path.basename(f):
            continue
        try:
            first = json.loads(open(f).readline())
        except Exception:
            continue
        if first.get("payload", {}).get("parent_thread_id") == sid:
            yield f


def codex_calls(f):
    """(model requests by response id, by changed cumulative snapshot)."""
    ids, n, last = set(), 0, None
    for e in lines(f):
        p = e.get("payload") or {}
        if e.get("type") == "token_usage_record" and p.get("response_id"):
            ids.add(p["response_id"])
        if e.get("type") == "event_msg" and p.get("type") == "token_count" and p.get("info"):
            total = json.dumps(p["info"].get("total_token_usage"), sort_keys=True)
            if total != last:
                n, last = n + 1, total
    return (len(ids) if ids else n), n


def count(tr, sessions):
    """{"model_calls", "child_calls", "per_file", "missing"}: missing lists sessions with no transcript."""
    out = {"model_calls": 0, "child_calls": 0, "per_file": {}, "missing": []}
    for sid in sessions:
        fs = files(tr, [sid])
        if not fs:
            out["missing"].append(sid)
        for kind, f in fs:
            if kind == "claude":
                ids = {e["message"]["id"] for e in lines(f)
                       if e.get("type") == "assistant" and isinstance(e.get("message"), dict) and e["message"].get("id")}
                n = len(ids)
            else:
                n, snapshots = codex_calls(f)
                out.setdefault("codex_snapshot_check", {})[os.path.basename(f)] = snapshots
                for c in codex_children(tr, sid):
                    k, _ = codex_calls(c)
                    out["child_calls"] += k
                    out["per_file"][os.path.basename(c)] = k
            out["model_calls"] += n
            out["per_file"][os.path.basename(f)] = n
    if out["missing"]:
        out["model_calls_complete"] = False
    return out


def tool_calls(tr, sessions):
    """(name, input text) of every tool call in the sessions' transcripts."""
    for kind, f in files(tr, sessions):
        for e in lines(f):
            if kind == "claude":
                m = e.get("message") if e.get("type") == "assistant" else None
                for b in (m or {}).get("content") or []:
                    if isinstance(b, dict) and b.get("type") == "tool_use":
                        yield b.get("name", ""), json.dumps(b.get("input"))
            else:
                p = e.get("payload") or {}
                if e.get("type") == "response_item" and p.get("type") in ("function_call", "custom_tool_call", "local_shell_call"):
                    yield p.get("name", p.get("type")), json.dumps(p.get("arguments") or p.get("input") or p.get("action"))


def computer_tool(name, text=""):
    if "computer" in name.lower():
        return True
    if name == "exec":
        called = re.findall(r"tools\.([A-Za-z0-9_]+)", text)
        return bool(called) and all("computer" in c for c in called)
    return False


WRITE_TOOLS = ("Write", "Edit", "MultiEdit", "NotebookEdit", "apply_patch")
# Shell forms that write to a path named after them.
WRITES = (r">>?\s*['\"]?{p}", r"\btee\b[^|;&]*{p}", r"\bsed\s+-i[^|;&]*{p}", r"\b(rm|truncate|touch|chmod)\b[^|;&]*{p}",
          r"\b(cp|mv|ln|rsync)\b[^|;&]*\s['\"]?{p}\S*['\"]?\s*($|[|;&])", r"\bdd\b[^|;&]*of={p}",
          r"open\([^)]*{p}[^)]*['\"][wa]")


def writes_to(name, text, path):
    if name in WRITE_TOOLS or name.lower() in ("write", "edit"):
        return path in text
    p = re.escape(path)
    return any(re.search(w.format(p=p) + r"", text) for w in WRITES)


def audit(tr, sessions, forbidden):
    forbidden = [f for f in forbidden if f]
    out = {"computer_tools": {}, "other_tools": [], "shortcuts": [], "peeks": []}
    for name, text in tool_calls(tr, sessions):
        if computer_tool(name, text):
            out["computer_tools"][name] = out["computer_tools"].get(name, 0) + 1
            continue
        if name.startswith("mcp__brigadier__") or (name == "exec" and "tools.mcp__brigadier__" in text
                                                   and "tools.mcp__computer__" not in text):
            continue  # the report and Brigadier's own worker tools: words, not actions
        out["other_tools"].append(f"{name}: {text[:300]}")
        command = json.loads(text) if text.startswith("{") else text
        command = command.get("command", text) if isinstance(command, dict) else text
        reach = [r for r in REACH if r in command]
        written = [f for f in forbidden if f in command and writes_to(name, command, f)]
        read = [f for f in forbidden if f in command and f not in written]
        if reach or written:
            out["shortcuts"].append(f"{name} {'writes ' + ', '.join(written) if written else ''}"
                                    f"{' reaches ' + ', '.join(reach) if reach else ''}: {command[:200]}")
        elif read:
            out["peeks"].append(f"{name} reads {', '.join(read)}: {command[:200]}")
    return out
