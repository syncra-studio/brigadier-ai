#!/usr/bin/env python3
"""Shared pieces of the V3 harness: ask files, a run's recorded texts, and link extraction.

Everything here reads only a run directory (`<root>/runs/<id>/`): its recorded events
(`rec/events.jsonl`), its hard-linked transcripts (`transcripts/`), and what the run's freeze step
wrote (`freeze.json`, `checks.json`, `tokens.json`, `manifest.json`, `data/routing.sqlite`). Nothing
reads the arm's live data dir, so a run scores the same wherever and whenever it is scored."""
import datetime, json, os, re, sqlite3

HERE = os.path.dirname(os.path.abspath(__file__))
AB = os.path.dirname(HERE)

# The safety sentence every tools/ab and bench arm request ends with (tools/ab/task.py).
import sys
sys.path.insert(0, AB)
from task import SAFETY  # noqa: E402


# ---------------------------------------------------------------- ask files

def load_ask(path):
    """An ask file: `key: value` lines before the first `## `, every ```text block under
    `## Request` (one per request, sent in order into one session), and the ```json block under
    `## Scoring` (the exact rule)."""
    text = open(path).read()
    head = text.split("\n## ", 1)[0]
    ask = {"path": os.path.abspath(path)}
    for line in head.splitlines():
        m = re.match(r"^([a-z0-9-]+): (.+)$", line)
        if m:
            ask[m.group(1)] = m.group(2).strip()
    req = re.search(r"\n## Request\n(.*?)(?=\n## |\Z)", text, re.S)
    ask["requests"] = [b.strip() for b in re.findall(r"```text\n(.*?)\n```", req.group(1), re.S)] if req else []
    rule = re.search(r"\n## Scoring\n.*?```json\n(.*?)\n```", text, re.S)
    ask["rule"] = json.loads(rule.group(1)) if rule else None
    if not ask.get("id") or not ask["requests"] or not ask["rule"]:
        raise ValueError(f"{path}: an ask needs id:, a ```text request and a ```json rule")
    ask["timeout"] = int(ask.get("timeout", "1200"))
    return ask


def ask_files(names=None):
    d = os.path.join(HERE, "asks")
    files = sorted(os.path.join(d, f) for f in os.listdir(d) if f.endswith(".md"))
    asks = [load_ask(f) for f in files]
    if names:
        want = names.split(",") if isinstance(names, str) else list(names)
        by = {a["id"]: a for a in asks}
        missing = [n for n in want if n not in by]
        if missing:
            raise SystemExit(f"unknown asks: {missing} (known: {sorted(by)})")
        return [by[n] for n in want]
    return asks


# ---------------------------------------------------------------- a run's record

def tsms(s):
    return int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def events(run):
    out = []
    with open(os.path.join(run, "rec", "events.jsonl")) as f:
        for line in f:
            if line.endswith("\n"):
                out.append(json.loads(line))
    out.sort(key=lambda env: env["seq"])
    return out


class Record:
    """The measured conversation as recorded: its requests, messages, tasks, reviews, worker
    events and the stored outputs, from the run's own event log (deduplicated by seq)."""

    def __init__(self, run):
        self.run = run
        self.start = json.load(open(os.path.join(run, "start.json")))
        self.conv = self.start["conversation"]
        self.t0 = self.start["t0_ms"]
        self.request_ids = self.start.get("requests", [])
        self.requests, self.tasks, self.reviews, self.messages = {}, {}, {}, []
        self.worker_events, self.setup, self.outputs, self.cards = [], None, [], []
        self.first_end = {}  # a request's first end: a later review or check can reopen it
        for env in events(run):
            e = env["event"]; t = e["type"]
            if t == "requestUpdated" and e["request"]["conversationId"] == self.conv:
                self.requests[e["request"]["id"]] = e["request"]
                if e["request"].get("endedAtMs"):
                    self.first_end.setdefault(e["request"]["id"], e["request"]["endedAtMs"])
            elif t == "messageAppended" and e["message"]["conversationId"] == self.conv:
                self.messages.append(e["message"])
            elif t == "taskUpdated" and e["task"]["conversationId"] == self.conv:
                self.tasks[e["task"]["id"]] = e["task"]
            elif t == "reviewUpdated" and e["review"]["conversationId"] == self.conv:
                self.reviews[e["review"]["id"]] = e["review"]
            elif t == "conversationSetUp" and e["id"] == self.conv:
                self.setup = e["setup"]
            elif t == "outputStored" and e.get("conversationId") == self.conv:
                self.outputs.append(e["output"])
        for env in events(run):
            e = env["event"]
            if e["type"] == "workerEvent" and e["taskId"] in self.tasks:
                self.worker_events.append((env["atMs"], e["taskId"], e["event"]))
        cards = os.path.join(run, "rec", "cards.jsonl")
        if os.path.exists(cards):
            self.cards = [json.loads(l) for l in open(cards) if l.strip()]

    def workspace(self):
        env = (self.setup or {}).get("environment") or {}
        return env.get("path")

    def answer(self, request_id):
        """Every assistant message of one request, in order, joined: the thread's answer."""
        msgs = [m for m in self.messages if m["role"] == "assistant" and m.get("requestId") == request_id]
        msgs.sort(key=lambda m: (m.get("seq", 0), m["id"]))
        return "\n\n".join(m["text"] for m in msgs)

    def final_answer(self):
        return self.answer(self.request_ids[-1]) if self.request_ids else ""

    def worker_texts(self):
        """What the workers wrote: their assistant messages and their reports (each field)."""
        out = []
        for _, tid, w in self.worker_events:
            if w.get("type") == "message" and w.get("role") == "assistant" and w.get("text"):
                out.append(("worker message", tid, w["text"]))
        for tid, t in sorted(self.tasks.items()):
            if t.get("report"):
                out.append(("worker report", tid, json.dumps(t["report"], ensure_ascii=False)))
        return out

    def read_tasks(self):
        return [t for t in self.tasks.values() if t["kind"] in ("scout", "research")]


# ---------------------------------------------------------------- transcripts

def manifest(run):
    p = os.path.join(run, "manifest.json")
    return json.load(open(p)) if os.path.exists(p) else []


def claude_lines(run, sid):
    """Every record of a Claude session's hard-linked transcript (and its subagents)."""
    base = os.path.join(run, "transcripts", "claude")
    files = [os.path.join(base, f"{sid}.jsonl")]
    sub = os.path.join(base, sid, "subagents")
    if os.path.isdir(sub):
        files += [os.path.join(sub, f) for f in sorted(os.listdir(sub))]
    for path in files:
        if not os.path.exists(path):
            continue
        for line in open(path, errors="replace"):
            try:
                yield json.loads(line)
            except ValueError:
                continue


def thread_sessions(run, conv):
    return [s["id"] for s in manifest(run) if s.get("owner") == f"orch:{conv}" and s["provider"] == "claude"]


def thread_inputs(run, rec, lo, hi):
    """What the thread received in [lo, hi]: the text of every user-role record (Brigadier's
    envelopes, addenda and tool results), except the user's own messages."""
    user_texts = {m["text"] for m in rec.messages if m["role"] == "user"}
    out = []
    for sid in thread_sessions(run, rec.conv):
        for r in claude_lines(run, sid):
            if r.get("type") != "user" or not r.get("timestamp") or not lo <= tsms(r["timestamp"]) <= hi:
                continue
            content = (r.get("message") or {}).get("content")
            parts = [content] if isinstance(content, str) else []
            for c in content if isinstance(content, list) else []:
                if c.get("type") == "text":
                    parts.append(c.get("text") or "")
                elif c.get("type") == "tool_result":
                    inner = c.get("content")
                    parts += [inner] if isinstance(inner, str) else [x.get("text") or "" for x in inner or [] if isinstance(x, dict)]
            for p in parts:
                if p and not any(u and u in p for u in user_texts):
                    out.append(p)
    return out


def thread_tool_calls(run, rec, lo, hi):
    """Every tool call the thread made in [lo, hi]: (name, input) from its transcript."""
    calls, seen = [], set()
    for sid in thread_sessions(run, rec.conv):
        for r in claude_lines(run, sid):
            if r.get("type") != "assistant" or not r.get("timestamp") or not lo <= tsms(r["timestamp"]) <= hi:
                continue
            for c in (r.get("message") or {}).get("content") or []:
                if isinstance(c, dict) and c.get("type") == "tool_use" and c.get("id") not in seen:
                    seen.add(c.get("id"))
                    calls.append((c.get("name"), c.get("input") or {}))
    return calls


# ---------------------------------------------------------------- links

_MD_LINK = re.compile(r"\]\(\s*<?([^)\s>]+)>?\s*\)")
_PATH = re.compile(r"(?<![\w.~/-])(?:file://)?((?:/|~/)[^\s)\]`'\"<>|,;]*)")
_ALIAS = re.compile(r"\bout-[0-9a-f]{8}\b")


def links_to(text, name):
    """Every link to the file `name` in `text`: markdown link targets, file:// URLs and absolute
    or ~ paths whose last part is `name` (a #L anchor or :line suffix is cut off), and output
    aliases (out-xxxxxxxx) on a line that names the file. A bare file name is a name, not a link."""
    found = []
    for m in _MD_LINK.finditer(text):
        found.append(m.group(1))
    for m in _PATH.finditer(text):
        found.append(m.group(1))
    out = []
    for raw in found:
        target = raw[len("file://"):] if raw.startswith("file://") else raw
        target = re.sub(r"(#L?\d+.*|:\d+(-\d+)?)$", "", target).rstrip(".")
        if os.path.basename(target) == name and target not in out:
            out.append(target)
    for line in text.splitlines():
        if name in line:
            for m in _ALIAS.finditer(line):
                if m.group(0) not in out:
                    out.append(m.group(0))
    return out


def abs_paths(text):
    """Every absolute or ~ path in `text` (for the freeze step's existence probe)."""
    out = set()
    for m in list(_MD_LINK.finditer(text)) + list(_PATH.finditer(text)):
        target = m.group(1)
        target = target[len("file://"):] if target.startswith("file://") else target
        target = re.sub(r"(#L?\d+.*|:\d+(-\d+)?)$", "", target).rstrip(".")
        if target.startswith("/") or target.startswith("~/"):
            out.add(target)
    return out


def routing(run):
    path = os.path.join(run, "data", "routing.sqlite")
    return sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)
