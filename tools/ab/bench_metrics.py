#!/usr/bin/env python3
"""The bench's four metrics, plus tokens per merged change (WORKSPACES-PLAN, Phase 4 · Bench, Q58),
read-only over a Brigadier data dir and the projects' repos. Run with `python3 -I`.

- First-look merge rate. One population for both merge modes: every request with a `request.ready`
  record or named in a `merged` step's `requests`, each once. A candidate is first-look when it
  merged and its tab got no user touch between its opening message and the merge. With auto-merge
  off, the merge go-ahead (the message, card or click the `merged` step's `askedIn` names) is no
  touch.
- User touches per merged change. In the change's tab, from its opening message (excluded) to the
  merge: user messages, the user's card answers (questions, approvals the user gave, plans the user
  decided, waits the user resolved) and side-panel actions the log records (Merge into main clicks,
  and Keep or Undo on a request's file; Later is also set on its own, so it isn't counted). The
  Source panel's own git commands (commit, push, discard…) leave no record in the log, so they
  can't be counted here.
- Escaped bugs. A merged change followed within 7 days, on its base in the same repo, by a revert
  of it, or by a commit touching any of its files whose subject matches `\\bfix` (any case) or
  `Fixes #`.
- Wasted quota at reset. Per account window that reset: 100 − used_percent at its last sample
  before the reset; weekly (`seven_day*`, Codex `secondary`) and 5-hour (`five_hour`, Codex
  `primary`) apart.
- Brain quota per window (#135). For each account window, the Brain jobs' share of its tokens
  and that share of its used percent.
- Tokens per merged change (report only). `turn_usage` of the tab and its tasks from the opening
  message to the merge, by model: input + cached input + cache writes + output.

Only typed records are read (#190): a payload must be a JSON object whose `type` is the one its
`kind` stands for; others are skipped and counted in `skipped_untyped`.

usage: bench_metrics.py <data-dir> [--repo DIR] [--until MS] [--json OUT]"""
import argparse, atexit, collections, json, os, re, shutil, sqlite3, subprocess, sys, tempfile

DAY_MS = 86_400_000
ESCAPE_MS = 7 * DAY_MS
# A window's reset time moves by a little between samples of the same window.
RESET_SLACK_MS = 10 * 60_000
FIX = re.compile(r"\bfix|Fixes #", re.IGNORECASE)

# The kinds read, with the `type` each payload must carry.
TYPES = {
    "project.created": "projectCreated",
    "project.updated": "projectUpdated",
    "conversation.created": "conversationCreated",
    "message.appended": "messageAppended",
    "request.updated": "requestUpdated",
    "request.ready": "requestReady",
    "orchestrator.step": "orchestratorStepped",
    "question.updated": "questionUpdated",
    "approval.updated": "approvalUpdated",
    "plan.updated": "planUpdated",
    "waiting.resolved": "waitingResolved",
}


def connect(path):
    """`path` read-only. A WAL database whose daemon is gone has no -shm to open read-only: a
    copy with its WAL is opened instead, so rows still in the WAL count."""
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        db.execute("PRAGMA schema_version").fetchone()
        return db
    except sqlite3.OperationalError:
        db.close()
    copy = os.path.join(tempfile.mkdtemp(prefix="bench-metrics-db-"), os.path.basename(path))
    atexit.register(shutil.rmtree, os.path.dirname(copy), True)
    for suffix in ("", "-wal"):
        if os.path.exists(path + suffix):
            shutil.copyfile(path + suffix, copy + suffix)
    return sqlite3.connect(copy)


def typed(kind, text):
    """The payload of a record of `kind`, or None when it isn't the typed record it should be."""
    try:
        payload = json.loads(text)
    except ValueError:
        return None
    if not isinstance(payload, dict) or payload.get("type") != TYPES.get(kind):
        return None
    return payload


def conversation_of(stream):
    return stream[len("conversation:"):] if stream.startswith("conversation:") else None


class Log:
    """What the event log says, gathered in one pass."""

    def __init__(self, db):
        self.skipped = 0
        self.repos = {}  # project id -> repo path
        self.project_of = {}  # conversation id -> project id
        self.opened = {}  # request id -> (conversation, opening ms)
        self.ready = {}  # request id -> conversation
        self.merges = {}  # (conversation, position or seq) -> merge
        self.touches = collections.defaultdict(list)  # conversation -> [(at, ref, what)]
        answered = set()
        reviewed = {}
        kinds = tuple(TYPES)
        rows = db.execute(
            f"SELECT seq, stream, stream_seq, kind, at_ms, payload FROM events WHERE kind IN "
            f"({','.join('?' * len(kinds))}) ORDER BY seq", kinds)
        for seq, stream, stream_seq, kind, at, text in rows:
            p = typed(kind, text)
            if p is None:
                self.skipped += 1
                continue
            conv = conversation_of(stream)
            if kind in ("project.created", "project.updated"):
                project = p.get("project") or {}
                repos = project.get("repos") or []
                if project.get("id") and repos:
                    self.repos[project["id"]] = repos[0].get("path")
            elif kind == "conversation.created":
                c = p.get("conversation") or {}
                if c.get("id"):
                    self.project_of[c["id"]] = c.get("projectId")
            elif kind == "message.appended":
                m = p.get("message") or {}
                if m.get("role") == "user":
                    conv = m.get("conversationId") or conv
                    self.opened.setdefault(m["id"], (conv, m.get("createdAtMs", at)))
                    self.touches[conv].append((at, m["id"], "message"))
            elif kind == "request.updated":
                r = p.get("request") or {}
                conv = r.get("conversationId") or conv
                self.opened.setdefault(r["id"], (conv, r.get("startedAtMs", at)))
                for path, state in (r.get("review") or {}).items():
                    key = (r["id"], path)
                    # Later is also set on its own, when a later request changes the same file.
                    if state in ("kept", "undone") and reviewed.get(key) != state:
                        self.touches[conv].append((at, f"{r['id']}:{path}", "file"))
                    reviewed[key] = state
            elif kind == "request.ready":
                self.ready.setdefault(p["requestId"], p.get("conversationId") or conv)
            elif kind == "orchestrator.step":
                step = p.get("step") or {}
                k = step.get("kind") or {}
                if k.get("type") == "merged":
                    self.merges[(conv, step.get("position") or stream_seq)] = {
                        "conversation": conv, "at_ms": step.get("atMs", at), "auto": bool(k.get("auto")),
                        "asked_in": k.get("askedIn"), "requests": list(k.get("requests") or []),
                        "commits": k.get("commits"),
                        "base": k.get("base"), "branch": k.get("branch"), "head": k.get("head"),
                        "merge_commit": k.get("mergeCommit"), "pr": k.get("pr")}
                elif k.get("type") == "mergeRequested":
                    self.touches[conv].append((step.get("atMs", at), k.get("id"), "click"))
            elif kind == "question.updated":
                q = p.get("question") or {}
                if q.get("answer") is not None and ("q", q.get("id")) not in answered:
                    answered.add(("q", q.get("id")))
                    self.touches[q.get("conversationId") or conv].append(
                        (q.get("answeredAtMs") or at, q.get("id"), "card"))
            elif kind == "approval.updated":
                a = p.get("approval") or {}
                state = a.get("state") or {}
                if state.get("by") == "user" and ("a", a.get("id")) not in answered:
                    answered.add(("a", a.get("id")))
                    self.touches[a.get("conversationId") or conv].append((at, a.get("id"), "card"))
            elif kind == "plan.updated":
                plan = p.get("plan") or {}
                state = plan.get("state") or {}
                decided = (state.get("type") == "approved" and state.get("by") == "user") or state.get("type") == "rejected"
                if decided and ("p", plan.get("id")) not in answered:
                    answered.add(("p", plan.get("id")))
                    self.touches[plan.get("conversationId") or conv].append(
                        (plan.get("decidedAtMs") or at, plan.get("id"), "card"))
            elif kind == "waiting.resolved":
                if p.get("by") == "user":
                    self.touches[conv].append((at, p.get("id"), "card"))
        self.merge_list = sorted(self.merges.values(), key=lambda m: m["at_ms"])

    def touches_between(self, conv, after, until, opening, merge):
        """The user's touches in `conv` after `after` up to `until`, the opening message and, with
        auto-merge off, the merge go-ahead aside."""
        go_ahead = None if merge is None or merge["auto"] else merge["asked_in"]
        return [t for t in self.touches.get(conv, [])
                if after < t[0] <= until and t[1] != opening and (go_ahead is None or t[1] != go_ahead)]


def first_look(log):
    candidates = dict(log.ready)
    for m in log.merge_list:
        for r in m["requests"]:
            candidates.setdefault(r, m["conversation"])
    rows = []
    for request, conv in sorted(candidates.items()):
        merge = next((m for m in log.merge_list if request in m["requests"]), None)
        opened = log.opened.get(request, (conv, None))[1]
        touches = None
        if merge is not None and opened is not None:
            touches = len(log.touches_between(conv, opened, merge["at_ms"], request, merge))
        rows.append({"request": request, "conversation": conv, "ready": request in log.ready,
                     "merged": merge is not None, "auto": merge["auto"] if merge else None,
                     "touches": touches, "first_look": merge is not None and touches == 0})
    count = sum(r["first_look"] for r in rows)
    return {"population": len(rows), "first_look": count,
            "rate": round(count / len(rows), 4) if rows else None, "requests": rows}


def opening_of(log, merge, before):
    """When the merged change opened, and its opening message: its earliest request's, else the
    tab's previous merge (no message)."""
    opened = [(log.opened[r][1], r) for r in merge["requests"] if r in log.opened]
    return min(opened) if opened else (before, None)


def merged_changes(log, routing):
    changes = []
    last_merge = {}
    for m in log.merge_list:
        conv = m["conversation"]
        opened, opening = opening_of(log, m, last_merge.get(conv, 0))
        last_merge[conv] = m["at_ms"]
        touches = log.touches_between(conv, opened, m["at_ms"], opening, m)
        tokens = collections.Counter()
        if routing is not None:
            for model, raw in routing.execute(
                    "SELECT model, input + cached_input + cache_write + output FROM turn_usage "
                    "WHERE conversation_id = ? AND at_ms BETWEEN ? AND ?", (conv, opened, m["at_ms"])):
                tokens[model] += raw
        changes.append({**m, "opened_ms": opened, "touches": len(touches),
                        "touched_by": sorted(collections.Counter(t[2] for t in touches).items()),
                        "tokens": dict(sorted(tokens.items()))})
    return changes


def git(repo, *args):
    run = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)
    return run.stdout if run.returncode == 0 else None


def landed_change(repo, change, sha):
    """The files the merged change touched, and the commits it put on the base. A two-parent
    merge: against its first parent. A fast-forward (its head is the merge): its last `commits`
    commits. A squash or rebase on GitHub: the branch head's own change since it forked."""
    parents = (git(repo, "rev-list", "--parents", "-n", "1", sha) or "").split()[1:]
    head = change["head"]
    if len(parents) >= 2:
        since, until = f"{sha}^1", sha
    elif head == sha or not head or git(repo, "cat-file", "-e", f"{head}^{{commit}}") is None:
        count = max(change.get("commits") or 1, 1)
        since = f"{sha}~{count}" if git(repo, "rev-parse", "-q", "--verify", f"{sha}~{count}") else f"{sha}^1"
        until = sha
    else:
        since, until = (git(repo, "merge-base", f"{sha}^1", head) or "").strip() or f"{sha}^1", head
    files = set((git(repo, "diff", "--name-only", since, until) or "").split())
    landed = set((git(repo, "rev-list", f"{since}..{sha}") or "").split()) if head == sha else {sha}
    return files, landed


def escaped(change, repo):
    """The commit that shows `change` escaped a bug, or None."""
    sha = change["merge_commit"] or change["head"]
    if not repo or not sha:
        return None
    base = next((ref for ref in (change["base"], f"origin/{change['base']}")
                 if ref and git(repo, "rev-parse", "--verify", "-q", ref + "^{commit}")), None)
    if base is None:
        return None
    files, landed = landed_change(repo, change, sha)
    log = git(repo, "log", "--format=%x1e%H%x1f%ct%x1f%s%x1f%b%x1f", "--name-only", f"{sha}..{base}") or ""
    shas = landed | {s for s in (change["merge_commit"], change["head"]) if s}
    since = change["at_ms"] // 1000
    for entry in log.split("\x1e")[1:]:
        commit, when, subject, body, touched = entry.split("\x1f", 4)
        if not since <= int(when) <= since + ESCAPE_MS // 1000:
            continue
        if any(f"This reverts commit {s}" in body for s in shas):
            return {"commit": commit, "why": "revert", "subject": subject}
        if FIX.search(subject) and files & set(touched.split()):
            return {"commit": commit, "why": "fix", "subject": subject}
    return None


def escaped_bugs(log, changes, repo_override):
    rows = []
    for change in changes:
        repo = repo_override or log.repos.get(log.project_of.get(change["conversation"]))
        hit = escaped(change, repo)
        rows.append({"conversation": change["conversation"], "merge_commit": change["merge_commit"],
                     "escaped": hit is not None, "by": hit})
    return {"merged": len(rows), "escaped": sum(r["escaped"] for r in rows), "changes": rows}


def window_kind(window):
    if window.startswith("seven_day") or window == "secondary":
        return "weekly"
    if window in ("five_hour", "primary"):
        return "five_hour"
    return window


WINDOW_MS = {"five_hour": 5 * 3_600_000, "weekly": 7 * DAY_MS}


def windows(routing, until):
    """Each account window the samples up to `until` saw, by its last sample: provider, account,
    window, reset time, used percent, and whether it reset."""
    samples = collections.defaultdict(list)
    for provider, account, window, used, resets, at in routing.execute(
            "SELECT provider, account, window, used_percent, resets_at_ms, at_ms FROM quota_samples "
            "WHERE at_ms <= ? ORDER BY at_ms", (until,)):
        samples[(provider, account, window)].append((at, used, resets))
    for (provider, account, window), rows in sorted(samples.items()):
        for i, (at, used, resets) in enumerate(rows):
            # The window's last sample, whose reset time passed before the next sample (or the
            # end). A reset time that moves back is another login on the same account slot.
            later = rows[i + 1] if i + 1 < len(rows) else None
            if resets is None:
                last = later is None or later[1] < used
                reset = later is not None and later[1] < used
            elif later is None:
                last, reset = True, resets <= until
            else:
                last = later[2] is None or abs(later[2] - resets) > RESET_SLACK_MS
                reset = last and resets <= min(later[0], until)
            if last:
                yield {"provider": provider, "account": account, "window": window, "resets_at_ms": resets,
                       "used_percent": used, "reset": reset}


def wasted_quota(routing, until):
    out = collections.defaultdict(list)
    if routing is None:
        return {}
    for w in windows(routing, until):
        if w["reset"]:
            out[window_kind(w["window"])].append({k: v for k, v in w.items() if k != "reset"}
                                                 | {"wasted_percent": round(100 - w["used_percent"], 2)})
    return {kind: {"resets": len(rows), "wasted_percent": [r["wasted_percent"] for r in rows],
                   "mean": round(sum(r["wasted_percent"] for r in rows) / len(rows), 2), "windows": rows}
            for kind, rows in sorted(out.items())}


def brain_per_window(routing, until):
    """Brain jobs' share of each account window (#135), from `turn_usage` as it is recorded all
    the time: the window's Brain tokens over all its tokens on that account, times its used
    percent at the last sample, is the Brain's estimated part of the window. A model-scoped
    weekly window (`seven_day_<family>`) counts only that family's rows."""
    if routing is None:
        return []
    columns = {row[1] for row in routing.execute("PRAGMA table_info(turn_usage)")}
    if "step" not in columns:
        return []
    account = "account" if "account" in columns else "''"
    out = []
    for w in windows(routing, until):
        length = WINDOW_MS.get(window_kind(w["window"]))
        if length is None or w["resets_at_ms"] is None:
            continue
        family = w["window"][len("seven_day_"):] if w["window"].startswith("seven_day_") else ""
        total, brain = routing.execute(
            "SELECT COALESCE(SUM(input + cached_input + cache_write + output), 0), "
            "COALESCE(SUM(CASE WHEN step = 'brain' THEN input + cached_input + cache_write + output END), 0) "
            f"FROM turn_usage WHERE provider = ? AND {account} = ? AND at_ms > ? AND at_ms <= ? AND model LIKE ?",
            (w["provider"], w["account"], w["resets_at_ms"] - length, min(w["resets_at_ms"], until),
             f"%{family}%")).fetchone()
        share = brain / total if total else 0.0
        out.append({k: v for k, v in w.items()} | {
            "tokens": total, "brain_tokens": brain, "brain_share": round(share, 4),
            "brain_percent": round(share * w["used_percent"], 2)})
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("data_dir")
    ap.add_argument("--repo", help="the repo every merge is looked up in (default: each tab's project's)")
    ap.add_argument("--until", type=int, help="the end of the bench, in ms (default: the newest event)")
    ap.add_argument("--json")
    args = ap.parse_args()
    db = connect(os.path.join(args.data_dir, "brigadier.db"))
    path = os.path.join(args.data_dir, "routing.sqlite")
    routing = connect(path) if os.path.exists(path) else None
    log = Log(db)
    until = args.until or (db.execute("SELECT max(at_ms) FROM events").fetchone()[0] or 0)
    changes = merged_changes(log, routing)
    touches = [c["touches"] for c in changes]
    result = {
        "first_look": first_look(log),
        "touches_per_merged_change": {
            "merged": len(changes), "mean": round(sum(touches) / len(touches), 4) if touches else None,
            "changes": [{k: c[k] for k in ("conversation", "merge_commit", "auto", "requests", "touches", "touched_by")}
                        for c in changes]},
        "escaped_bugs": escaped_bugs(log, changes, args.repo),
        "wasted_quota_at_reset": wasted_quota(routing, until),
        "brain_quota_per_window": brain_per_window(routing, until),
        "tokens_per_merged_change": [{"conversation": c["conversation"], "merge_commit": c["merge_commit"],
                                      "by_model": c["tokens"], "total": sum(c["tokens"].values())} for c in changes],
        "skipped_untyped": log.skipped,
    }
    text = json.dumps(result, indent=1)
    print(text)
    if args.json:
        with open(args.json, "w") as out:
            out.write(text + "\n")


if __name__ == "__main__":
    sys.exit(main())
