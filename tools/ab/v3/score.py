#!/usr/bin/env python3
"""Scores one recorded V3 attempt from its run directory alone, deterministically: the same run
dir gives the same output, byte for byte (selftest.py checks it).

Measured (PLAN §0 V3): model calls, worker starts, time to the final answer, exact correctness,
and usage through settlement (all vendors summed, cache reads apart), plus thread turns,
interventions and the Brain's background work in the window.

Rules (an ask's ```json block):
- "answer": the final answer cites one of `locations` (file suffix and a line inside the range, a
  cited range at most 20 lines wide) when any are given, and matches every `facts` regex
  (case-insensitive).
- "check": the edit's hidden check passed on the session workspace as the run left it.
- "links" (A1): a read task stored `file`; every link to it in the workers' text, the addenda the
  thread received and the final answer resolves after disposal (an existing file, or a stored
  copy: an output alias or blob hash this run stored); the final answer has at least one link.
- "home" (A2): a read task stored `file` in an earlier request; the last answer points at Save
  (the card's Save or a Save chip) or asks where the file should go; the thread ran no `pbcopy`
  and wrote nothing outside the workspace and temp folders; nothing new appeared in
  ~/Desktop, ~/Downloads or ~/Documents.
- "fixed" (A6): a read task stored `file` with the planted error (`planted`); the delivered file
  (the one the final answer links, else the latest stored copy) matches `right` and not `planted`;
  the final answer has no "To check" about `topic`; with `no_question`, the thread didn't ask the user
  (a question card about `topic`) instead of fixing it (F2/U3: the user never has to say "fix it yourself").
A precondition that wasn't met makes the attempt "invalid" (pass false, why says so).
usage: score.py <run-dir>   prints the score as JSON"""
import collections, json, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import v3lib  # noqa: E402

TEMP = ("/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/", "/dev/")


# ---------------------------------------------------------------- correctness

_LOC = [
    re.compile(r"([\w./-]+\.(?:ts|json|md))(?:`|\*\*)?\s*(?::|#L)\s*L?(\d+)(?:\s*[-–]\s*L?(\d+))?"),
    re.compile(r"([\w./-]+\.(?:ts|json|md))(?:`|\*\*)?,?\s+(?:at\s+|on\s+)?lines?\s+(\d+)(?:\s*(?:[-–]|to|and)\s*(\d+))?", re.I),
    re.compile(r"lines?\s+(\d+)(?:\s*(?:[-–]|to|and)\s*(\d+))?\s+(?:of|in)\s+`?([\w./-]+\.(?:ts|json|md))", re.I),
]


def cited(text):
    out = []
    for i, rx in enumerate(_LOC):
        for m in rx.finditer(text):
            if i < 2:
                f, a, b = m.group(1), int(m.group(2)), int(m.group(3) or m.group(2))
            else:
                a, b, f = int(m.group(1)), int(m.group(2) or m.group(1)), m.group(3)
            out.append((f, min(a, b), max(a, b)))
    return sorted(set(out))


def location_ok(cites, loc):
    for f, a, b in cites:
        same = f.endswith(loc["file"]) or f == os.path.basename(loc["file"])
        if same and b - a <= 20 and a <= loc["lines"][1] and b >= loc["lines"][0]:
            return True
    return False


def score_answer(rule, text):
    cites = cited(text)
    locs = rule.get("locations") or []
    loc_ok = not locs or any(location_ok(cites, l) for l in locs)
    facts = {f["name"]: bool(re.search(f["re"], text, re.I | re.S)) for f in rule.get("facts") or []}
    why = []
    if not loc_ok:
        why.append("no expected file:line cited (cited: %s)" % ", ".join(f"{f}:{a}-{b}" for f, a, b in cites[:6]))
    why += [f"fact missing: {n}" for n, ok in facts.items() if not ok]
    return loc_ok and all(facts.values()), "; ".join(why) or "ok", {"cited": [f"{f}:{a}-{b}" for f, a, b in cites], "facts": facts}


def stored_named(freeze, name, request_tasks=None):
    return [s for s in freeze.get("stored_outputs", []) if s.get("fileName") == name and s["kind"] in ("scout", "research")
            and (request_tasks is None or s["task"] in request_tasks)]


def resolves(link, freeze, run, rec):
    """A link opens something after disposal: an output alias Brigadier stored for this
    conversation, a blob this run stored, or a path that existed at the freeze."""
    if re.fullmatch(r"out-[0-9a-f]{8}", link):
        return link in {o.get("alias") for o in rec.outputs}
    h = re.search(r"[0-9a-f]{64}", link)
    if h and os.path.exists(os.path.join(run, "blobs", h.group(0))):
        return True
    p = freeze["probe"].get(link)
    return bool(p and p["exists"])


def score_links(rule, rec, run, freeze):
    name = rule["file"]
    if not stored_named(freeze, name):
        return False, f"invalid: no read task stored {name}", {}
    hi = 1 << 62
    sources = [("worker", t) for _, _, t in rec.worker_texts()]
    sources += [("addendum", t) for t in v3lib.thread_inputs(run, rec, rec.t0, hi)]
    sources += [("final answer", rec.final_answer())]
    links = collections.defaultdict(list)
    for where, text in sources:
        for l in v3lib.links_to(text, name):
            if l not in links[where]:
                links[where].append(l)
    dead = {w: [l for l in ls if not resolves(l, freeze, run, rec)] for w, ls in links.items()}
    dead = {w: ls for w, ls in dead.items() if ls}
    why = []
    if not links.get("final answer"):
        why.append("the final answer has no link to " + name)
    why += [f"dead link in the {w}: {l}" for w, ls in sorted(dead.items()) for l in ls]
    return not why, "; ".join(why) or "ok", {"links": dict(links), "dead": dead}


def write_targets(command, workspace):
    """Paths a shell command writes to (redirections, tee, cp/mv/ditto/rsync/install's last
    argument, mkdir, touch), resolved against the last `cd` before them (else the workspace)."""
    out = []
    for seg in re.split(r"&&|\|\||;|\n", command):
        seg = seg.strip()
        m = re.match(r"cd\s+([^\s]+)", seg)
        if m:
            workspace = m.group(1).strip("'\"")
            continue
        targets = re.findall(r"(?<![<>&0-9])>{1,2}\s*([^\s;&|)]+)", seg)
        targets += re.findall(r"\btee\s+(?:-a\s+)?([^\s;&|]+)", seg)
        m = re.match(r"(?:sudo\s+)?(cp|mv|ditto|rsync|install)\b(.*)", seg)
        if m:
            args = [x for x in m.group(2).split() if not x.startswith("-")]
            if args:
                targets.append(args[-1])
        targets += re.findall(r"\b(?:mkdir|touch)\s+(?:-p\s+)?([^\s;&|]+)", seg)
        for t in targets:
            t = t.strip("'\"")
            if t.startswith("&") or t == "/dev/null":
                continue
            t = t.replace("$HOME", "~").replace("${HOME}", "~")
            if t.startswith("$TMPDIR") or t.startswith("${TMPDIR}"):
                t = "/tmp/" + t.split("/", 1)[-1]
            if not (t.startswith("/") or t.startswith("~")):
                t = (workspace or ".").rstrip("/") + "/" + t
            out.append(t)
    return out


def outside(path, workspace):
    if path.startswith("~"):
        return True
    if workspace and (path == workspace or path.startswith(workspace.rstrip("/") + "/")):
        return False
    return not path.startswith(TEMP)


def points_at_save(text):
    """The answer points at Save: the Save button or chip by name, or a sentence that says to
    save it from the task card (or a chip)."""
    return bool(re.search(r"\bSave\b|Save…|Save\.\.\.", text)) or any(
        re.search(r"\bsave\b", sent, re.I) and re.search(r"\bcard\b|\bchip\b", sent, re.I)
        for sent in re.split(r"(?<=[.!?])\s+|\n", text))


def score_home(rule, rec, run, freeze):
    name = rule["file"]
    first = set(t for t, task in rec.tasks.items() if rec.request_ids and task.get("requestId") in rec.request_ids[:-1])
    if not stored_named(freeze, name, first or None):
        return False, f"invalid: no read task stored {name} before the last request", {}
    text = rec.final_answer()
    save = points_at_save(text)
    asks = any("?" in s and re.search(r"\bwhere\b|which folder|what folder|location", s, re.I)
               for s in re.split(r"(?<=[.!?])\s+", text))
    ws = freeze.get("workspace")
    calls = v3lib.thread_tool_calls(run, rec, rec.t0, 1 << 62)
    pbcopy, writes, copies = [], [], []
    for name_, inp in calls:
        blob = json.dumps(inp, ensure_ascii=False)
        if re.search(r"\bpbcopy\b|\bosascript\b.*clipboard", blob):
            pbcopy.append(f"{name_}: {blob[:160]}")
        if name_ in ("Write", "Edit", "MultiEdit", "NotebookEdit") and inp.get("file_path"):
            if outside(inp["file_path"], ws):
                writes.append(f"{name_} {inp['file_path']}")
            # Harness revision 2026-10-15 (the Delegator for A2): the thread hands the stored
            # file over through its Save, never a copy of its own, wherever it puts it.
            if os.path.basename(inp["file_path"]) == name:
                copies.append(f"{name_} {inp['file_path']}")
        cmd = inp.get("command") if isinstance(inp.get("command"), str) else None
        if cmd:
            writes += [f"shell write {t}" for t in write_targets(cmd, ws) if outside(t, ws)]
            copies += [f"shell: {seg.strip()[:160]}" for seg in re.split(r"&&|\|\||;|\n", cmd)
                       if name in seg and (write_targets(seg, ws) or re.search(r"\bgit\s+(show|cat-file)\b.*>", seg))]
    home = {d: n for d, n in (freeze.get("home_new_entries") or {}).items() if n}
    why = []
    if not (save or asks):
        why.append("the answer neither points at Save nor asks where the file should go")
    why += [f"pbcopy: {p}" for p in pbcopy] + [f"wrote outside the workspace: {w}" for w in writes]
    why += [f"wrote its own copy of {name}: {c}" for c in copies]
    why += [f"new in {d}: {n}" for d, n in sorted(home.items())]
    return not why, "; ".join(why) or "ok", {"save": save, "asks_where": asks, "pbcopy": pbcopy, "writes_outside": writes,
                                            "own_copies": copies,
                                            "home_new_entries": home, "clipboard_changed": freeze.get("clipboard_changed")}


def to_check_sections(text):
    out, lines = [], text.splitlines()
    for i, l in enumerate(lines):
        if re.search(r"\bto check\b", l, re.I):
            sec = [l]
            for nxt in lines[i + 1:]:
                if not nxt.strip() and len(sec) > 1:
                    break
                if re.match(r"\s*(#|\*\*[^*]+\*\*\s*$)", nxt):
                    break
                sec.append(nxt)
            out.append("\n".join(sec))
    return out


def questions_about(rec, topic):
    """The question cards the thread put to the user about `topic` (its text or any item's)."""
    out = []
    for c in rec.cards:
        if c["kind"] != "question":
            continue
        card = c["card"]
        text = "\n".join([card.get("text") or ""] + [(i or {}).get("text") or "" for i in card.get("items") or []])
        if re.search(topic, text, re.I):
            out.append(text[:200])
    return out


def score_fixed(rule, rec, run, freeze):
    name = rule["file"]
    stored = stored_named(freeze, name)
    def content(h):
        p = os.path.join(run, "blobs", h)
        return open(p, errors="replace").read() if os.path.exists(p) else None
    planted = [s for s in stored if content(s["blob"]) and re.search(rule["planted"], content(s["blob"]))]
    if not planted:
        return False, f"invalid: no read task stored {name} with the planted error", {}
    text = rec.final_answer()
    delivered, src = None, None
    for l in v3lib.links_to(text, name):
        p = freeze["probe"].get(l)
        if p and p.get("sha256") and os.path.exists(os.path.join(run, "linked", p["sha256"])):
            delivered, src = open(os.path.join(run, "linked", p["sha256"]), errors="replace").read(), l
    if delivered is None:
        last = stored[-1]
        delivered, src = content(last["blob"]), f"stored copy {last['blob'][:12]}"
    right = bool(delivered) and bool(re.search(rule["right"], delivered)) and not re.search(rule["planted"], delivered)
    flagged = [s for s in to_check_sections(text) if re.search(rule["topic"], s, re.I)]
    why = []
    if not right:
        why.append(f"the delivered file ({src}) is not right")
    if flagged:
        why.append("the answer leaves it under To check")
    asked = questions_about(rec, rule["topic"])
    if rule.get("no_question") and asked:
        why.append("the thread asked the user instead of fixing it")
    return not why, "; ".join(why) or "ok", {"delivered_from": src, "delivered_right": right, "to_check": flagged,
                                            "asked_user": asked}


# ---------------------------------------------------------------- measurements

def model_calls(run, rec, lo, hi):
    """Model calls per role from the hard-linked transcripts of the conversation's own sessions:
    Claude, one per assistant message id (subagents included); Codex, one per token_count event
    whose cumulative total grew."""
    calls = collections.Counter()
    for s in v3lib.manifest(run):
        role = s["role"]
        role = "thread" if role == "orchestrator" else "review" if role.startswith("review") else "worker"
        if s["provider"] == "claude":
            ids = set()
            for r in v3lib.claude_lines(run, s["id"]):
                m = r.get("message") or {}
                if r.get("type") == "assistant" and m.get("id") and r.get("timestamp") and lo <= v3lib.tsms(r["timestamp"]) <= hi:
                    ids.add(m["id"])
            calls[f"claude:{role}"] += len(ids)
        else:
            files = [f for f in os.listdir(os.path.join(run, "transcripts", "codex")) if s["id"] in f] \
                if os.path.isdir(os.path.join(run, "transcripts", "codex")) else []
            last = 0
            for f in sorted(files)[:1]:
                for line in open(os.path.join(run, "transcripts", "codex", f), errors="replace"):
                    try:
                        r = json.loads(line)
                    except ValueError:
                        continue
                    p = r.get("payload") or {}
                    if p.get("type") == "token_count" and p.get("info") and r.get("timestamp"):
                        tot = (p["info"].get("total_token_usage") or {}).get("total_tokens") or 0
                        if tot > last:
                            if lo <= v3lib.tsms(r["timestamp"]) <= hi:
                                calls[f"codex:{role}"] += 1
                            last = tot
    out = dict(sorted(calls.items()))
    out["total"] = sum(calls.values())
    out["thread"] = sum(v for k, v in calls.items() if k.endswith(":thread"))
    return out


def main():
    run = sys.argv[1]
    rec = v3lib.Record(run)
    ask_id = rec.start["ask"]
    ask = v3lib.ask_files([ask_id])[0]
    rule = ask["rule"]
    freeze = json.load(open(os.path.join(run, "freeze.json")))
    times = json.load(open(os.path.join(run, "times.json")))
    tokens = json.load(open(os.path.join(run, "tokens.json")))
    checks = json.load(open(os.path.join(run, "checks.json")))
    settled = times["settled_ms"]
    last = rec.requests.get(rec.request_ids[-1]) if rec.request_ids else None
    end = (last or {}).get("endedAtMs")
    state = ((last or {}).get("state") or {}).get("type")
    final = rec.final_answer()
    # Completed: every request was sent and ended "done" (a failed, stopped or still-waiting request
    # is not completed), within the ask's timeout, with a final answer.
    states = [((rec.requests.get(r) or {}).get("state") or {}).get("type") for r in rec.request_ids]
    completed = bool(not rec.start.get("timed_out") and end and final.strip() and len(rec.request_ids) == len(ask["requests"])
                     and all(s == "done" for s in states))

    kind = rule["rule"]
    if not completed:
        ok, why, detail = False, f"not completed (states {states}, timed out {rec.start.get('timed_out')})", {}
    elif kind == "answer":
        ok, why, detail = score_answer(rule, final)
    elif kind == "check":
        ok = checks.get("rc") == 0
        why, detail = "ok" if ok else f"hidden check failed (rc {checks.get('rc')})", {"check_output_tail": (checks.get("output") or "")[-600:]}
    elif kind == "links":
        ok, why, detail = score_links(rule, rec, run, freeze)
    elif kind == "home":
        ok, why, detail = score_home(rule, rec, run, freeze)
    elif kind == "fixed":
        ok, why, detail = score_fixed(rule, rec, run, freeze)
    else:
        raise SystemExit(f"unknown rule {kind}")

    starts = [(at, tid) for at, tid, w in rec.worker_events if w.get("type") == "sessionStarted" and rec.t0 <= at <= settled]
    db = v3lib.routing(run)
    thread_turns = db.execute("SELECT COUNT(*) FROM turn_usage WHERE conversation_id = ? AND task_id IS NULL AND at_ms BETWEEN ? AND ?",
                              (rec.conv, rec.t0, settled)).fetchone()[0]
    recon = tokens.get("reconciled") or {}
    interventions = [c for c in rec.cards if not c["kind"].endswith("-answer")]
    out = {
        "ask": ask_id, "kind": ask.get("kind"), "case": ask.get("case"), "regime": rec.start["regime"], "brain": rec.start["brain"],
        "label": rec.start["label"], "n": rec.start["n"], "src_commit": rec.start["build"]["src_commit"],
        "completed": completed, "pass": bool(ok), "why": why, "detail": detail, "final_state": state,
        "answer_s": round((end - rec.t0) / 1000, 1) if end else None,
        "first_answer_s": round((rec.first_end[rec.request_ids[-1]] - rec.t0) / 1000, 1) if end and rec.request_ids[-1] in rec.first_end else None,
        "reopened": bool(end) and end != rec.first_end.get(rec.request_ids[-1]), "settled_s": round((settled - rec.t0) / 1000, 1),
        "model_calls": model_calls(run, rec, rec.t0, settled), "thread_turns": thread_turns,
        "worker_starts": len(starts), "tasks": dict(collections.Counter(t["kind"] for t in rec.tasks.values())),
        "tokens": {"raw": (recon.get("total") or {}).get("raw"), "raw_without_cache_reads": (recon.get("total") or {}).get("raw_without_cache_reads"),
                   "cache_read": (recon.get("total") or {}).get("cache_read"), "by_provider": recon.get("by_provider"),
                   "complete": recon.get("complete", False), "background": tokens.get("background"),
                   "other_in_window": tokens.get("other_in_window")},
        "interventions": len(interventions), "warmup": bool(rec.start.get("warmup")),
    }
    print(json.dumps(out, indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
