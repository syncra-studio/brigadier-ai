#!/usr/bin/env python3
"""Scores one V2 run from its directory alone (PLAN §0 V2): each planted item passes or fails on
its own. An item passes when the probe recalls it exactly, every step that must act on it did so
correctly (the hidden checks' RESULT lines), and, for the false `learned` line, no brief carries it
as fact. "unknown" given instead of a guess is scored apart: an item the thread lost and said it
didn't know is `unknown`, one it guessed wrong is `fail`.

An item is `not_tested` where its source was still in the verbatim tail of the last briefing before
a step that needs it: the run didn't test forgetting there. A run whose seams didn't happen at least
twice between the last plant and the probe is invalid (`valid: false`).

usage: score.py <run-dir>     prints the score as JSON (run.py writes it to score.json)"""
import glob, json, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "v3"))
import v3lib  # noqa: E402
from v2lib import load_battery  # noqa: E402

RECENT = "[The conversation's latest messages, verbatim]"
RECENT_END = ("\n\n[Older context]", "\n\n[End of briefing]")
SEAMS_WANTED = 2
UNKNOWN = re.compile(r"\bunknown\b|\bi (no longer|don't|do not) (know|remember)\b", re.I)
TOOL_EVENTS = {"toolCall", "command", "fileChanges"}


def orchestrator(run):
    path = os.path.join(run, "orchestrator.jsonl")
    return [json.loads(l) for l in open(path)] if os.path.exists(path) else []


def briefings(run, log):
    """Each rebirth: its generation, when the new CLI got the briefing, its size, its text and
    its verbatim-tail section."""
    out = []
    for e in log:
        if e["entry"].get("type") != "rebirth":
            continue
        r = e["entry"]["record"]
        path = os.path.join(run, "briefings", f"{r['generation']:02d}-briefing.txt")
        text = open(path).read() if os.path.exists(path) else ""
        recent = ""
        if RECENT in text:
            recent = text.split(RECENT, 1)[1]
            for end in RECENT_END:
                recent = recent.split(end, 1)[0]
        out.append({"generation": r["generation"], "at_ms": r["swappedAtMs"], "tokens": r.get("briefingTokens"),
                    "trigger": r.get("trigger"), "decisions": r.get("decisions"), "recent_messages": r.get("recentMessages"),
                    "sections": [{"name": s.get("name"), "tokens": s.get("tokens"), "truncated": s.get("truncated")}
                                 for s in r.get("sections") or []],
                    "text": text, "recent": recent})
    return sorted(out, key=lambda b: b["at_ms"])


def worker_briefs(run, rec):
    """The first input of every worker session the conversation owns (its brief), from the
    hard-linked transcripts: (session id, sent ms, text)."""
    out = []
    manifest = v3lib.manifest(run)
    for s in manifest:
        if not s.get("owner", "").startswith("task:"):
            continue
        if s["provider"] == "claude":
            for r in v3lib.claude_lines(run, s["id"]):
                if r.get("type") != "user" or not r.get("timestamp"):
                    continue
                content = (r.get("message") or {}).get("content")
                text = content if isinstance(content, str) else "\n".join(
                    c.get("text") or "" for c in content or [] if isinstance(c, dict) and c.get("type") == "text")
                if text:
                    out.append((s["id"], v3lib.tsms(r["timestamp"]), text)); break
        else:
            for path in glob.glob(os.path.join(run, "transcripts", "codex", f"rollout-*{s['id']}.jsonl")):
                for line in open(path, errors="replace"):
                    try:
                        r = json.loads(line)
                    except ValueError:
                        continue
                    p = r.get("payload") or {}
                    if p.get("type") == "message" and p.get("role") == "user":
                        text = "\n".join(c.get("text") or "" for c in p.get("content") or [] if isinstance(c, dict))
                        if text and "<environment_context>" not in text and "<user_instructions>" not in text:
                            out.append((s["id"], v3lib.tsms(r["timestamp"]) if r.get("timestamp") else 0, text)); break
    return out


def probe_entries(answer):
    """A numbered answer as {number: text}."""
    entries, current = {}, None
    for line in answer.splitlines():
        m = re.match(r"^\s*(?:\*\*)?(\d+)(?:\*\*)?[.)]\s*(?:\*\*)?(.*)$", line)
        if m:
            current = int(m.group(1)); entries[current] = m.group(2)
        elif current is not None:
            entries[current] += "\n" + line
    return entries


def recall(item, entry):
    """pass: recalled exactly; unknown: said it doesn't know; wrong: anything else (a guess)."""
    if entry is None:
        return "missing"
    if "recall" in item:
        ok = all(re.search(p, entry, re.I) for p in item["recall"])
    else:
        ok = any(re.search(p, entry, re.I) for p in item.get("recall_any", [])) and \
            not any(re.search(p, entry, re.I | re.M) for p in item.get("recall_none", []))
    if ok:
        return "pass"
    return "unknown" if UNKNOWN.search(entry) else "wrong"


def acted(item, checks):
    step = item.get("acted")
    if not step:
        return None
    c = checks.get(step) or {}
    result = c.get("result") or {}
    verdict = result.get(item["id"])
    if verdict is None and item["id"] == "ruled_out":
        verdict = result.get("ruled_out")
    return {"step": step, "ok": verdict is True, "verdict": verdict, "check_rc": c.get("rc")}


def tail_free(source_text, sent, uses_ms, briefs):
    """For each use time: the last briefing between the source's send and the use, and whether the
    source's words had left its verbatim tail (None: no briefing in between, the session kept its
    context whole)."""
    key = source_text[:80]
    out = []
    for use in uses_ms:
        last = [b for b in briefs if sent < b["at_ms"] < use]
        if not last:
            out.append({"use_ms": use, "briefing": None, "left_tail": None})
        else:
            b = last[-1]
            out.append({"use_ms": use, "briefing": b["generation"], "left_tail": key not in b["recent"]})
    return out


def seams_forced(start, briefs, log, steps_ms, last_plant_ms, probe_ms):
    kind = start["seam"]
    if kind == "size":
        return sum(1 for b in briefs if last_plant_ms < b["at_ms"] < probe_ms)
    if kind == "inturn":
        # Rebirths of a turn stopped mid-request (BUILD-PLAN D2.3b), not by size between turns.
        return sum(1 for e in log if e["entry"].get("type") == "rebirth"
                   and e["entry"]["record"].get("trigger") == "midTurn"
                   and last_plant_ms < e["entry"]["record"]["swappedAtMs"] < probe_ms)
    if kind in ("restart", "kill"):
        key = "killed_pid" if kind == "restart" else "killed"
        return sum(1 for s in start.get("seams", []) if s.get(key))
    if kind in ("context", "auth", "billing", "limit"):
        return sum(1 for e in log if e["entry"].get("type") == "provider"
                   and (e["entry"].get("event") or {}).get("type") == "error"
                   and ((e["entry"]["event"].get("error") or {}).get("code") == "injected"))
    return 0


def score(run, probe=None):
    """`probe`: the step whose answer is scored (by default the battery's first `probe` step)."""
    start = json.load(open(os.path.join(run, "start.json")))
    b = load_battery(os.path.join(run, "battery.md"))
    steps, harness, items = b["steps"], b["harness"], b["items"]
    rec = v3lib.Record(run)
    log = orchestrator(run)
    briefs = briefings(run, log)
    checks = json.load(open(os.path.join(run, "checks.json"))) if os.path.exists(os.path.join(run, "checks.json")) else {}
    sent = dict(zip(start["steps"], start["sent_ms"]))
    rid = dict(zip(start["steps"], start["requests"]))
    text = {s["id"]: s["text"] for s in steps}
    probe = probe or next((s["id"] for s in steps if s["role"] == "probe"), None)
    completed = not start.get("timed_out") and probe in sent and all(
        (rec.requests.get(r) or {}).get("state", {}).get("type") in ("done", "waiting") for r in start["requests"])
    plants = [s["id"] for s in steps if s["role"] in ("plant", "worker")]
    last_plant_ms = max((sent[p] for p in plants if p in sent), default=0)
    probe_ms = sent.get(probe, 10 ** 15)
    forced = seams_forced(start, briefs, log, sent, last_plant_ms, probe_ms)
    valid = start["seam"] == "none" or forced >= SEAMS_WANTED
    answer = rec.answer(rid[probe]) if probe in rid else ""
    entries = probe_entries(answer)
    wbriefs = worker_briefs(run, rec)

    by_item, detail = {}, {}
    for item in items:
        d = {"what": item["what"]}
        src = item["source"]
        uses = [item["acted"]] if item.get("acted") else []
        uses.append(probe)
        d["tail"] = tail_free(text[src], sent.get(src, 0), [sent.get(u, 10 ** 15) for u in uses], briefs)
        d["recall"] = recall(item, entries.get(item.get("probe")))
        d["probe_entry"] = (entries.get(item.get("probe")) or "")[:400]
        d["acted"] = acted(item, checks)
        if item.get("brief_text"):
            needle = item["brief_text"].lower()
            seen = []
            for b in briefs:
                for line in b["text"].splitlines():
                    if needle in line.lower():
                        seen.append({"where": f"briefing {b['generation']}", "labelled": bool(re.search(r"unverified|claim", line, re.I)),
                                     "line": line.strip()[:300]})
            for sid, at, t in wbriefs:
                if at and at < sent.get(src, 0):
                    continue
                for line in t.splitlines():
                    if needle in line.lower():
                        seen.append({"where": f"worker brief {sid}", "labelled": bool(re.search(r"unverified|claim", line, re.I)),
                                     "line": line.strip()[:300]})
            d["briefs"] = {"seen": seen, "as_fact": [s for s in seen if not s["labelled"]]}
        tested = all(t["left_tail"] is not False for t in d["tail"])
        acted_ok = d["acted"] is None or d["acted"]["ok"]
        briefs_ok = not d.get("briefs") or not d["briefs"]["as_fact"]
        if not tested:
            status = "not_tested"
        elif d["recall"] == "pass" and acted_ok and briefs_ok:
            status = "pass"
        elif item["id"] == "false_learned" and d["recall"] == "unknown" and briefs_ok:
            status = "pass"  # no fact claimed either way
        elif d["recall"] == "unknown" and acted_ok and briefs_ok:
            status = "unknown"
        else:
            status = "fail"
        by_item[item["id"]] = status
        detail[item["id"]] = d

    lost = [i for i, s in by_item.items() if s in ("unknown", "fail")]
    unknowns = sum(1 for i in lost if detail[i]["recall"] == "unknown")
    guessed = sum(1 for i in lost if detail[i]["recall"] == "wrong")
    probe_end = (rec.requests.get(rid.get(probe)) or {}).get("endedAtMs") or 10 ** 15
    probe_tools = sum(1 for e in log if e["entry"].get("type") == "provider" and probe_ms <= e["atMs"] <= probe_end
                      and (e["entry"].get("event") or {}).get("type") in TOOL_EVENTS)
    why = []
    if not completed:
        why.append("not completed")
    if not valid:
        why.append(f"seams forced {forced} of {SEAMS_WANTED}")
    why += [f"{i}: {s}" for i, s in by_item.items() if s != "pass"]
    return {
        "battery": start["battery"], "seam": start["seam"], "provider": start["provider"], "label": start["label"], "n": start["n"],
        "completed": completed, "valid": valid, "seams": {"kind": start["seam"], "forced": forced, "wanted": SEAMS_WANTED},
        "by_item": by_item, "all_pass": completed and valid and all(s == "pass" for s in by_item.values()),
        "unknown_instead_of_guess": {"lost": len(lost), "unknown": unknowns, "guessed": guessed,
                                     "score": round(unknowns / (unknowns + guessed), 3) if unknowns + guessed else None},
        "briefings": [{k: b[k] for k in ("generation", "at_ms", "tokens", "trigger", "decisions", "recent_messages", "sections")}
                      for b in briefs],
        "worker_briefs": [{"session": sid, "bytes": len(t)} for sid, _, t in wbriefs],
        "probe_tool_calls": probe_tools,
        "items": detail,
        "why": "; ".join(why) or "all items pass",
    }


if __name__ == "__main__":
    print(json.dumps(score(sys.argv[1]), indent=1, sort_keys=True))
