#!/usr/bin/env python3
"""/delegator session manifest for an arm, from the run's own records, not from cwd alone:
- the coordinator: the session in the clone whose first prompt is the /delegator command;
- every worker (successors included): Claude ids from session.json, Codex ids from codex.log;
- every checkout the run used: the clone, its git worktrees, and each worker's cwd
  (session.json, launch.sh);
- every Codex session started in the window in one of those checkouts (reviews, plan reviews);
- every review file the run saved (msgs/*review*.md) must match the final message of a counted
  Codex session; one that matches none is listed under unmatched_reviews (its usage is unknown).
Codex sessions started in the window elsewhere are listed apart, not counted, for a manual look.
--also DIR (repeatable): a folder the run's workers used for model turns of their own, such as a
dev Brigadier's data dir they ran the app on: every Claude session under it (by project slug) and
every Codex session started in it during the window counts too.
usage: dlg_manifest.py <arm-dir> <end_ms> [--also DIR ...] > manifest.json"""
import datetime, glob, json, os, re, subprocess, sys

arm = os.path.realpath(sys.argv[1]); end = int(sys.argv[2]); H = os.path.expanduser("~")
also = [sys.argv[i + 1] for i, v in enumerate(sys.argv) if v == "--also"]
st = json.load(open(arm + "/start.json")); t0 = st["t0_ms"]
repo = arm + "/repo"


def spellings(path):
    path = os.path.realpath(path)
    return {path, path[len("/private"):]} if path.startswith("/private/") else {path}


def ts(s):
    return int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def norm(text):
    return re.sub(r"\s+", " ", text or "").strip()[:400]


def renderings(last):
    """A review's final message as `dlg review` may save it: verbatim, or (for `codex exec
    review`'s JSON verdict) its explanation or a finding's title/body."""
    out = [norm(last)]
    try:
        v = json.loads(last)
    except (TypeError, ValueError):
        return out
    if isinstance(v, dict):
        out.append(norm(v.get("overall_explanation")))
        for f in v.get("findings") or []:
            if isinstance(f, dict):
                out += [norm(f.get("title")), norm(f.get("body"))]
    return [t for t in out if t]


checkouts = set(spellings(repo))
for line in subprocess.run(["git", "-C", repo, "worktree", "list", "--porcelain"], capture_output=True, text=True).stdout.splitlines():
    if line.startswith("worktree "):
        checkouts |= spellings(line[len("worktree "):])

run = None
for env in glob.glob(f"{H}/.claude/delegator/runs/*/run.env"):
    m = re.search(r"^REPO=['\"]?([^'\"\n]+)", open(env).read(), re.M)
    if m and os.path.realpath(m.group(1)) in {os.path.realpath(c) for c in checkouts} and os.path.getmtime(env) * 1000 >= t0 - 60000:
        run = os.path.dirname(env)
out = []


def add(label, role, provider, sid, extra=None):
    out.append({"label": label, "role": role, "provider": provider, "id": sid, **(extra or {})})


workers = sorted(glob.glob(run + "/workers/w*")) if run else []
for w in workers:
    if os.path.exists(w + "/session.json"):
        cwd = json.load(open(w + "/session.json")).get("cwd")
        if cwd:
            checkouts |= spellings(cwd)
    if os.path.exists(w + "/launch.sh"):
        m = re.search(r"^cd (.+)$", open(w + "/launch.sh").read(), re.M)
        if m:
            path = m.group(1).strip().strip("'\"")
            if os.path.isdir(path):
                checkouts |= spellings(path)


def inside(cwd):
    return any(cwd == c or cwd.startswith(c + "/") for c in checkouts)


for c in sorted(checkouts):
    for f in sorted(glob.glob(f"{H}/.claude/projects/{re.sub(r'[^A-Za-z0-9]', '-', c)}/*.jsonl")):
        if os.path.getmtime(f) * 1000 < t0:
            continue
        if "<command-name>/delegator</command-name>" in open(f, errors="replace").read(200000):
            add("Delegator", "coordinator", "claude", os.path.basename(f)[:-6], {"dir": os.path.dirname(f)})
for w in workers:
    wid = os.path.basename(w)
    agent = open(w + "/agent").read().strip() if os.path.exists(w + "/agent") else "?"
    title = open(w + "/title").read().strip() if os.path.exists(w + "/title") else ""
    if agent == "claude" and os.path.exists(w + "/session.json"):
        add(f"{wid} {title}", "worker", "claude", json.load(open(w + "/session.json"))["session_id"])
    elif agent == "codex" and os.path.exists(w + "/codex.log"):
        for sid in sorted(set(re.findall(r"session id: ([0-9a-f-]+)", open(w + "/codex.log").read()))):
            add(f"{wid} {title}", "worker", "codex", sid)
    else:
        add(f"{wid} {title}", "worker", agent, "UNKNOWN-" + wid)

for d in also:
    for c in spellings(d) | {d.rstrip("/")}:
        checkouts.add(c)
        slug = re.sub(r"[^A-Za-z0-9]", "-", c)
        for f in sorted(glob.glob(f"{H}/.claude/projects/{slug}*/*.jsonl")):
            if os.path.getmtime(f) * 1000 >= t0 and os.path.basename(f)[:-6] not in {s["id"] for s in out}:
                add(f"dev app {os.path.basename(os.path.dirname(f))[:60]}", "worker-dev-app", "claude",
                    os.path.basename(f)[:-6], {"dir": os.path.dirname(f)})

listed = {s["id"] for s in out}
finals = {}; elsewhere = []
for f in glob.glob(f"{H}/.codex/sessions/*/*/*/rollout-*.jsonl"):
    if os.path.getmtime(f) * 1000 < t0:
        continue
    p = json.loads(open(f).readline()).get("payload", {})
    if not (t0 <= ts(p.get("timestamp", "1970-01-01T00:00:00Z")) <= end):
        continue
    last = None
    for line in open(f):
        r = json.loads(line); q = r.get("payload") or {}
        if q.get("type") == "task_complete" and q.get("last_agent_message"):
            last = q["last_agent_message"]
    finals[p["id"]] = (renderings(last) if last else [], p.get("cwd") or "")
    if p["id"] in listed:
        continue
    if inside(p.get("cwd") or ""):
        add("codex " + str(p.get("source", "?")) + " " + str(p.get("originator")), "review/codex", "codex", p["id"],
            {"started": p["timestamp"], "cwd": p.get("cwd")})
    else:
        elsewhere.append({"id": p["id"], "cwd": p.get("cwd"), "started": p["timestamp"]})

reviews = sorted(glob.glob(run + "/msgs/*review*.md")) if run else []
counted = {s["id"] for s in out}
unmatched = []
for r in reviews:
    text = norm(open(r).read())
    hits = [sid for sid, (lasts, _) in finals.items()
            if any(t[:200] == text[:200] or (len(t) >= 40 and t[:200] in text) for t in lasts)]
    if not hits:
        unmatched.append(os.path.basename(r))
    for sid in hits:
        if sid not in counted:
            add(f"codex review {os.path.basename(r)}", "review/codex", "codex", sid, {"cwd": finals[sid][1]})
            counted.add(sid)
print(json.dumps({"run": run, "checkouts": sorted(checkouts), "sessions": out,
                  "review_outputs": [os.path.basename(r) for r in reviews], "unmatched_reviews": unmatched,
                  "codex_elsewhere_not_counted": elsewhere}, indent=1))
