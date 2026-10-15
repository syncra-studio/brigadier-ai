#!/usr/bin/env python3
"""The V3 small-ask harness and V1's accounting: frozen asks on the frozen fixture, run against a dev
brigadierd with its own data dir, each attempt kept with its evidence and score.

A harness root (under /tmp) holds one build:
  build.json                 the daemon (BRIGD), its source (BRIG_SRC) and a label
  template/{data,repo}       the arm right after setup: onboarded, Full access, Fable hidden, Brain
                             enrichment off, every non-review kind of work on Claude Opus (as the
                             bench), the fixture added as a project and its Brain jobs ended
  template-empty/            the same with the project's Brain emptied (--brain empty)
  seed/brain.sqlite          the frozen Brain seed (the template's Brain, or the one passed with --seed)
  arm/{data,repo}            the live arm: restored from a template before every attempt, so every
                             attempt starts from the same state at the same absolute paths
  runs/<id>/                 one attempt: rec/, transcripts/, start.json, freeze.json, checks.json,
                             data/routing.sqlite, tokens.json, manifest.json, score.json
  results.jsonl              one line per attempt (every attempt, failed or not)
  state.json                 when the last session ended (the cold regime's hour)

usage:
  run.py template --root R [--seed brain.sqlite]           (BRIGD, BRIG_SRC from the environment)
  run.py ask --root R --ask q1 --regime warm|cold --brain seeded|empty [--n 1] [--tag base]
  run.py set --root R --asks q1,e1,... --regime ... --brain ... [--reruns 1]
  run.py ab --roots A=R1,B=R2 --asks ... --regime ... --brain ... [--reruns 3]   (A B B A ... per ask)
  run.py cleanup --root R [--export DIR]
Models: the thread on Opus at high; workers on Opus at the product's per-kind effort (as the bench);
reviews as shipped. Nothing here touches the installed app, its daemon or its data."""
import argparse, glob, hashlib, json, os, re, shutil, signal, sqlite3, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
AB = os.path.dirname(HERE)
sys.path.insert(0, AB); sys.path.insert(0, HERE)
from bipc import req  # noqa: E402
import v3lib  # noqa: E402

QUIET_S = 30          # settled: nothing active and no new event for this long
SETTLE_MAX_S = 900
WARMUP_TEXT = "Reply with one word: ready."
HOME = os.path.expanduser("~")
WATCHED = [os.path.join(HOME, d) for d in ("Desktop", "Downloads", "Documents")]


def now_ms():
    return int(time.time() * 1000)


def log(*a):
    print(time.strftime("%T"), *a, flush=True)


# ---------------------------------------------------------------- paths and safety

def check_root(root):
    real = os.path.realpath(root)
    if not re.match(r"^/private/tmp/[^/]+", real):
        sys.exit(f"run.py: the harness root must be under /tmp, not {root}")
    if "Application Support/Brigadier" in real:
        sys.exit("run.py: refusing the installed app's data")
    return real


def safe_rmtree(root, path):
    """Removes `path` only when it is strictly inside the harness root (never the root itself)."""
    real = os.path.realpath(path)
    if not real.startswith(os.path.realpath(root) + "/"):
        raise SystemExit(f"safe_rmtree: refusing {path}")
    if os.path.lexists(real):
        shutil.rmtree(real) if os.path.isdir(real) and not os.path.islink(real) else os.remove(real)


def clone_tree(src, dst):
    subprocess.run(["cp", "-c", "-R", src, dst], check=True)


# ---------------------------------------------------------------- processes (only our own PIDs)

def descendants(pids):
    out = subprocess.run(["ps", "-Ao", "pid=,ppid="], capture_output=True, text=True).stdout
    kids = {}
    for line in out.splitlines():
        p, pp = line.split()
        kids.setdefault(int(pp), []).append(int(p))
    todo, seen = list(pids), []
    while todo:
        p = todo.pop()
        if p not in seen:
            seen.append(p); todo += kids.get(p, [])
    return seen


def stop(pids, grace=15):
    """TERM to the PIDs this harness started and all their descendants, then KILL what is left."""
    tree = descendants([p for p in pids if p])
    for p in tree:
        try: os.kill(p, signal.SIGTERM)
        except ProcessLookupError: pass
    for _ in range(grace):
        alive = [p for p in tree if _alive(p)]
        if not alive: return
        time.sleep(1)
    for p in tree:
        try: os.kill(p, signal.SIGKILL)
        except ProcessLookupError: pass


def _alive(p):
    try:
        os.kill(p, 0); return True
    except ProcessLookupError:
        return False


def pinned_src(root, build):
    """The daemon's IPC protocol source, pinned inside the root (`build-src/`): startd.sh reads the
    protocol version from BRIG_SRC, and the tree the daemon was built from may be gone (or have
    moved on) by the time a rerun comes. Taken from the build's tree, else from git at its commit."""
    rel = os.path.join("crates", "ipc", "src", "protocol.rs")
    dst = os.path.join(root, "build-src")
    if not os.path.exists(os.path.join(dst, rel)):
        os.makedirs(os.path.dirname(os.path.join(dst, rel)), exist_ok=True)
        src = os.path.join(build["src"], rel)
        if os.path.exists(src):
            shutil.copyfile(src, os.path.join(dst, rel))
        else:
            text = subprocess.run(["git", "-C", AB, "show", f"{build['src_commit']}:{rel}"], check=True,
                                  capture_output=True, text=True).stdout
            open(os.path.join(dst, rel), "w").write(text)
    return dst


class Arm:
    """The live daemon and recorder of one attempt."""

    def __init__(self, root, rec_dir, log_dir):
        self.root = root; self.data = os.path.join(root, "arm", "data"); self.repo = os.path.join(root, "arm", "repo")
        build = json.load(open(os.path.join(root, "build.json")))
        env = dict(os.environ, BRIGD=build["brigd"], BRIG_SRC=pinned_src(root, build))
        env.pop("BRIG_CLAUDE_HOME", None)  # the bench's default: the machine's own Claude login
        os.makedirs(log_dir, exist_ok=True)
        self.daemon = subprocess.Popen([os.path.join(AB, "startd.sh"), self.data], env=env, start_new_session=True,
                                       stdout=open(os.path.join(log_dir, "daemon.log"), "a"), stderr=subprocess.STDOUT)
        self._pids(self.daemon.pid)
        sock, token = os.path.join(self.data, "run", "brigadierd.sock"), os.path.join(self.data, "run", "ipc.token")
        for _ in range(240):
            if os.path.exists(sock) and os.path.exists(token) and os.path.getsize(token):
                try:
                    req(self.data, {"method": "getCatalog"}); break
                except Exception:
                    pass
            if self.daemon.poll() is not None:
                raise SystemExit(f"daemon exited: see {log_dir}/daemon.log")
            time.sleep(0.5)
        else:
            raise SystemExit("daemon did not start")
        self.recorder = subprocess.Popen([sys.executable, os.path.join(AB, "recorder.py"), self.data, rec_dir],
                                         start_new_session=True, stdout=open(os.path.join(log_dir, "recorder.log"), "a"),
                                         stderr=subprocess.STDOUT)
        self._pids(self.recorder.pid)
        self.events = os.path.join(rec_dir, "events.jsonl")

    def _pids(self, pid):
        with open(os.path.join(self.root, "pids"), "a") as f:
            f.write(f"{pid}\n")

    def close(self):
        stop([self.recorder.pid]); stop([self.daemon.pid], grace=30)


# ---------------------------------------------------------------- template

def template(a):
    root = check_root(a.root)
    brigd, src = os.environ.get("BRIGD"), os.environ.get("BRIG_SRC") or os.path.realpath(os.path.join(AB, "..", ".."))
    if not brigd or brigd.startswith("/Applications/"):
        sys.exit("template: set BRIGD to a dev brigadierd (never the installed app's)")
    os.makedirs(root, exist_ok=True)
    if os.path.exists(os.path.join(root, "template")):
        sys.exit(f"template: {root}/template exists")
    sha = subprocess.run(["git", "-C", src, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(["git", "-C", src, "status", "--porcelain", "--", "crates"], capture_output=True, text=True).stdout.strip()
    json.dump({"brigd": os.path.realpath(brigd), "src": os.path.realpath(src), "src_commit": sha, "src_crates_dirty": bool(dirty),
               "label": a.tag or sha[:8], "brigd_sha256": sha256(brigd), "made_ms": now_ms()},
              open(os.path.join(root, "build.json"), "w"), indent=1)
    os.makedirs(os.path.join(root, "arm"), exist_ok=True)
    out = subprocess.run([os.path.join(HERE, "fixture", "build.sh"), os.path.join(root, "arm", "repo"), os.path.join(root, "nm-cache")],
                         check=True, capture_output=True, text=True).stdout
    log("fixture", out.strip().replace("\n", " "))
    meta = os.path.join(root, "template-meta"); os.makedirs(meta, exist_ok=True)
    arm = Arm(root, os.path.join(meta, "rec"), meta)
    try:
        subprocess.run([sys.executable, os.path.join(AB, "setup.py"), arm.data, arm.repo, "--permission", "fullAccess",
                        "--claude-workers", "opus"], check=True, stdout=open(os.path.join(meta, "setup.log"), "w"))
        s = req(arm.data, {"method": "getCatalog"})["value"]["catalog"]["settings"]
        s["enrichBrain"] = False  # no Brain enrichment inside a measured window (V1: "or keep enrichment off and say so")
        log("enrichment off:", req(arm.data, {"method": "updateSettings", "settings": s})["status"])
        subprocess.run([sys.executable, os.path.join(AB, "brainwait.py"), meta, "--max", "300"], check=True,
                       stdout=open(os.path.join(meta, "brainwait.log"), "w"))
        log("Brain jobs ended")
    finally:
        arm.close()
    brains = glob.glob(os.path.join(arm.data, "brains", "*", "brain.sqlite"))
    if len(brains) != 1:
        sys.exit(f"template: expected one project Brain, found {brains}")
    os.makedirs(os.path.join(root, "seed"), exist_ok=True)
    seed = os.path.join(root, "seed", "brain.sqlite")
    if a.seed:
        backup(a.seed, seed)
        for suffix in ("", "-wal", "-shm"):
            if os.path.exists(brains[0] + suffix): os.remove(brains[0] + suffix)
        backup(seed, brains[0])
        log("Brain seed from", a.seed)
    else:
        backup(brains[0], seed)
    json.dump(brain_digest(seed), open(os.path.join(root, "seed", "seed.json"), "w"), indent=1)
    clone_tree(os.path.join(root, "arm"), os.path.join(root, "template"))
    log("template ready:", json.load(open(os.path.join(root, "seed", "seed.json"))))


def backup(src, dst):
    """A consistent copy of a SQLite database (WAL included) through the backup API."""
    s = sqlite3.connect(src); d = sqlite3.connect(dst)
    s.backup(d); d.close(); s.close()


def brain_digest(path):
    """What a Brain holds, independent of the file's bytes: node count by kind and origin, and a
    hash over every node's kind, title and body (sorted)."""
    db = sqlite3.connect(f"file:{path}?mode=ro&immutable=1", uri=True)
    rows = sorted(db.execute("SELECT kind, title, body FROM nodes"))
    origins = dict(db.execute("SELECT origin, COUNT(DISTINCT node_id) FROM node_sources GROUP BY origin"))
    kinds = dict(db.execute("SELECT kind, COUNT(*) FROM nodes GROUP BY kind"))
    h = hashlib.sha256(json.dumps(rows, ensure_ascii=False).encode()).hexdigest()
    return {"nodes": len(rows), "kinds": kinds, "origins": origins, "content_sha256": h}


def empty_template(root):
    """template-empty: the seeded template with the project's Brain emptied. One inert node with a
    `skeleton` source is kept, because the daemon reruns the skeleton pass (a model job) on start
    whenever no skeleton-origin node is left; the code index's structure nodes come back at start,
    as on any start."""
    dst = os.path.join(root, "template-empty")
    if os.path.exists(dst):
        return dst
    clone_tree(os.path.join(root, "template"), dst)
    brain = glob.glob(os.path.join(dst, "data", "brains", "*", "brain.sqlite"))[0]
    db = sqlite3.connect(brain); db.execute("PRAGMA foreign_keys = ON")
    keep = db.execute("SELECT node_id FROM node_sources WHERE origin = 'skeleton' ORDER BY node_id LIMIT 1").fetchone()
    if not keep:
        sys.exit("empty_template: the seed has no skeleton node")
    db.execute("DELETE FROM nodes WHERE id != ?", keep)
    db.execute("UPDATE nodes SET title = 'Project', body = '' WHERE id = ?", keep)
    db.execute("DELETE FROM node_sources WHERE node_id = ? AND origin != 'skeleton'", keep)
    db.execute("DELETE FROM transcript")
    db.commit(); db.execute("VACUUM"); db.close()
    json.dump(brain_digest(brain), open(os.path.join(dst, "brain.json"), "w"), indent=1)
    return dst


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# ---------------------------------------------------------------- one attempt

def restore(root, brain):
    src = os.path.join(root, "template") if brain == "seeded" else empty_template(root)
    for part in ("data", "repo"):
        safe_rmtree(root, os.path.join(root, "arm", part))
        clone_tree(os.path.join(src, part), os.path.join(root, "arm", part))


def state(root):
    p = os.path.join(root, "state.json")
    return json.load(open(p)) if os.path.exists(p) else {}


def set_state(root, **kw):
    s = state(root); s.update(kw)
    json.dump(s, open(os.path.join(root, "state.json"), "w"), indent=1)


def last_session_any(roots):
    return max([state(r).get("last_session_ms", 0) for r in roots] or [0])


def read_events(path):
    if not os.path.exists(path):
        return []
    out = []
    with open(path) as f:
        for line in f:
            if line.endswith("\n"):
                out.append(json.loads(line)["event"])
    return out


def conv_state(evpath, conv):
    reqs, tasks, reviews = {}, {}, {}
    for e in read_events(evpath):
        t = e["type"]
        if t == "requestUpdated" and e["request"]["conversationId"] == conv:
            reqs[e["request"]["id"]] = e["request"]
        elif t == "taskUpdated" and e["task"]["conversationId"] == conv:
            tasks[e["task"]["id"]] = e["task"]["state"]
        elif t == "reviewUpdated" and e["review"]["conversationId"] == conv:
            reviews[e["review"]["id"]] = e["review"]["state"]["type"]
    return reqs, tasks, reviews


ACTIVE_TASK = {"queued", "starting", "running", "landing"}


def busy(evpath, conv):
    reqs, tasks, reviews = conv_state(evpath, conv)
    task_states = [t if isinstance(t, str) else t.get("type") for t in tasks.values()]
    return ([r["id"] for r in reqs.values() if r["state"]["type"] == "working"] +
            [s for s in task_states if s in ACTIVE_TASK] + [r for r in reviews.values() if r == "running"])


def create_conversation(data, repo):
    cat = req(data, {"method": "getCatalog"})["value"]["catalog"]
    real = os.path.realpath(repo)
    proj = [p for p in cat["projects"] if p["repos"] and os.path.realpath(p["repos"][0]["path"]) == real][0]
    setup = {"type": "session", "repo": proj["repos"][0]["path"], "environment": {"type": "newWorktree", "base": "main", "branch": None},
             "permission": cat["settings"]["defaultPermission"],
             "orchestrator": {"provider": "claude", "model": "opus", "effort": "high"}, "planMode": False}
    r = req(data, {"method": "createConversation", "kind": "session", "projectId": proj["id"], "title": None, "setup": setup})
    return r["value"]["conversation"]["id"], setup["permission"]


def send(data, conv, text):
    t = now_ms()
    r = req(data, {"method": "sendMessage", "conversationId": conv, "text": text, "attachments": [], "mentions": [], "steer": False})
    out = (r.get("value") or {}).get("outcome") or {}
    rid = (out.get("message") or {}).get("id")
    if not rid:
        raise SystemExit(f"sendMessage: {json.dumps(r)[:400]}")
    return t, rid


def wait_request(evpath, conv, rid, deadline):
    """Until request `rid` leaves "working" (answered, waiting on the user, failed or stopped)."""
    while time.time() < deadline:
        reqs, _, _ = conv_state(evpath, conv)
        r = reqs.get(rid)
        if r and r["state"]["type"] != "working":
            return r
        time.sleep(3)
    return None


def settle(evpath, conv, deadline):
    """Until nothing of the conversation is active and no event came for QUIET_S (a quiet log)."""
    start = time.time()
    while time.time() < min(deadline, start + SETTLE_MAX_S):
        if not busy(evpath, conv) and time.time() - os.path.getmtime(evpath) >= QUIET_S:
            return True
        time.sleep(3)
    return False


def listing():
    out = {}
    for d in WATCHED:
        try: out[d] = sorted(os.listdir(d))
        except OSError: out[d] = None
    return out


def clipboard_hash():
    try:
        return hashlib.sha256(subprocess.run(["pbpaste"], capture_output=True, timeout=5).stdout).hexdigest()
    except Exception:
        return None


def one(root, ask, regime, brain, n, tag, roots_for_cold):
    build = json.load(open(os.path.join(root, "build.json")))
    label = tag or build["label"]
    rid_ = f"{ask['id']}-{regime}-{brain}-{label}-{n}"
    run = os.path.join(root, "runs", rid_)
    if os.path.exists(os.path.join(run, "score.json")):
        log(rid_, "scored already"); return json.load(open(os.path.join(run, "score.json")))
    if os.path.exists(run):
        safe_rmtree(root, run)
    os.makedirs(run)
    if regime == "cold":
        gap = (now_ms() - last_session_any(roots_for_cold)) / 1000
        if gap < 3600:
            log(f"cold: waiting {int(3600 - gap)} s (no session with these flags within the last hour)")
            time.sleep(3600 - gap)
    restore(root, brain)
    conditions = subprocess.run(["bash", "-c", "sysctl -n vm.loadavg; claude --version; codex --version; node --version"],
                                capture_output=True, text=True).stdout.split("\n")
    arm = Arm(root, os.path.join(run, "rec"), run)
    start = {"arm": "brigadier", "ask": ask["id"], "regime": regime, "brain": brain, "n": n, "build": build, "label": label,
             "conditions_start": conditions}
    try:
        time.sleep(5)  # the start-up index scan of a 30-file repo
        if regime == "warm":
            wconv, _ = create_conversation(arm.data, arm.repo)
            wt, wrid = send(arm.data, wconv, WARMUP_TEXT)
            wreq = wait_request(arm.events, wconv, wrid, time.time() + 600)
            settle(arm.events, wconv, time.time() + 300)
            start["warmup"] = {"conversation": wconv, "sent_ms": wt, "request": wrid,
                               "state": (wreq or {}).get("state"), "ended_ms": (wreq or {}).get("endedAtMs")}
            log(rid_, "warm-up", (wreq or {}).get("state"))
        before = {"listing": listing(), "clipboard": clipboard_hash()}
        conv, permission = create_conversation(arm.data, arm.repo)
        deadline = time.time() + ask["timeout"]
        start.update({"conversation": conv, "permission": permission, "requests": [], "sent_ms": []})
        timed_out = False
        for i, text in enumerate(ask["requests"]):
            t, rid = send(arm.data, conv, text + " " + v3lib.SAFETY)
            if i == 0:
                start["t0_ms"] = t
            start["requests"].append(rid); start["sent_ms"].append(t)
            json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
            log(rid_, f"request {i + 1}/{len(ask['requests'])} sent")
            r = wait_request(arm.events, conv, rid, deadline)
            if r is None:
                timed_out = True; break
            log(rid_, f"request {i + 1} ended:", r["state"]["type"])
            if i + 1 < len(ask["requests"]):
                settle(arm.events, conv, deadline)
        settled = False if timed_out else settle(arm.events, conv, deadline + SETTLE_MAX_S)
        start["timed_out"] = timed_out; start["settled_quietly"] = settled
        json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
        freeze(root, run, ask, arm, before)
    finally:
        arm.close()
    backup(os.path.join(arm.data, "routing.sqlite"), ensure(os.path.join(run, "data")) + "/routing.sqlite")
    run_checks(root, run, ask)
    after(run)
    score = json.loads(subprocess.run([sys.executable, os.path.join(HERE, "score.py"), run], check=True,
                                      capture_output=True, text=True).stdout)
    json.dump(score, open(os.path.join(run, "score.json"), "w"), indent=1, sort_keys=True)
    with open(os.path.join(root, "results.jsonl"), "a") as f:
        f.write(json.dumps({"run": rid_, "root": root, **score}, sort_keys=True) + "\n")
    times = json.load(open(os.path.join(run, "times.json")))
    set_state(root, last_session_ms=max(now_ms() if timed_out else times["settled_ms"], state(root).get("last_session_ms", 0)))
    log(rid_, "PASS" if score["pass"] else "FAIL", score["why"], f"{score['answer_s']}s", f"calls {score['model_calls']['total']}")
    return score


def ensure(d):
    os.makedirs(d, exist_ok=True); return d


def freeze(root, run, ask, arm, before):
    """While the arm still stands, after settlement: which linked paths exist, the stored outputs,
    the linked deliverables' contents, the session workspace's files, and what changed in the
    user's Desktop/Downloads/Documents and clipboard. The scorer reads only this and the record."""
    rec = v3lib.Record(run)
    texts = [m["text"] for m in rec.messages] + [t for _, _, t in rec.worker_texts()]
    # The thread's and workers' Claude transcripts (addenda and tool results name paths too). They
    # are hard-linked into the run dir only later (after()), so read them where Claude keeps them:
    # the project folders named after this root, written since t0.
    for v in {root, root[len("/private"):] if root.startswith("/private/") else root}:
        slug = re.sub(r"[^A-Za-z0-9]", "-", v)
        for f in glob.glob(os.path.join(HOME, ".claude", "projects", glob.escape(slug) + "*", "**", "*.jsonl"), recursive=True):
            if os.path.getmtime(f) * 1000 >= rec.t0:
                texts.append(open(f, errors="replace").read())
    paths = set()
    for t in texts:
        paths |= v3lib.abs_paths(t)
    probe = {}
    for p in sorted(paths):
        full = os.path.expanduser(p)
        probe[p] = {"exists": os.path.isfile(full) or os.path.isdir(full), "file": os.path.isfile(full)}
        if os.path.isfile(full) and os.path.getsize(full) < 5 << 20:
            probe[p]["sha256"] = sha256(full)
    name = (ask["rule"] or {}).get("file")
    linked = ensure(os.path.join(run, "linked"))
    if name:
        for p, v in probe.items():
            if v.get("sha256") and os.path.basename(p) == name:
                shutil.copyfile(os.path.expanduser(p), os.path.join(linked, v["sha256"]))
    blobs = ensure(os.path.join(run, "blobs")); stored = []
    for t in rec.tasks.values():
        for art in (t.get("outputs") or []) + ((t.get("report") or {}).get("outputs") or []):
            h = art.get("id") if isinstance(art, dict) else None
            if not h or not re.fullmatch(r"[0-9a-f]{64}", h):
                continue
            hits = glob.glob(os.path.join(arm.data, "**", h[:2], h), recursive=True)
            if hits and not os.path.exists(os.path.join(blobs, h)):
                shutil.copyfile(hits[0], os.path.join(blobs, h))
            stored.append({"task": t["id"], "kind": t["kind"], "fileName": art.get("fileName"), "title": art.get("title"),
                           "blob": h, "copied": bool(hits)})
    ws = rec.workspace(); git = {}
    if ws and os.path.isdir(ws):
        files = subprocess.run(["git", "-C", ws, "ls-files", "-co", "--exclude-standard", "-z"], capture_output=True).stdout
        result = ensure(os.path.join(run, "result"))
        subprocess.run(["tar", "-C", ws, "--null", "-T", "-", "-cf", os.path.join(run, "result.tar")], input=files, check=True)
        subprocess.run(["tar", "-C", result, "-xf", os.path.join(run, "result.tar")], check=True)
        os.remove(os.path.join(run, "result.tar"))
        g = lambda *a: subprocess.run(["git", "-C", ws, *a], capture_output=True, text=True).stdout.strip()
        git = {"head": g("rev-parse", "HEAD"), "ahead_of_main": g("log", "--oneline", "main..HEAD").splitlines(),
               "status": g("status", "--porcelain").splitlines()}
    after_listing = listing()
    new = {d: sorted(set(after_listing[d] or []) - set(before["listing"][d] or [])) for d in WATCHED}
    json.dump({"at_ms": now_ms(), "probe": probe, "stored_outputs": stored, "workspace": ws, "git": git,
               "home_new_entries": new, "clipboard_changed": clipboard_hash() != before["clipboard"]},
              open(os.path.join(run, "freeze.json"), "w"), indent=1, sort_keys=True)
    for d, names in new.items():
        if names:
            log(f"NOTE: new entries in {d} during the run: {names} (left as they are; check by hand)")


def run_checks(root, run, ask):
    """An edit's hidden check, once, on a copy of the session workspace as the run left it (with the
    fixture's node_modules), recorded in checks.json."""
    cmd = (ask["rule"] or {}).get("check")
    out = {"command": cmd}
    if cmd:
        result = os.path.join(run, "result")
        if not os.path.isdir(result):
            out.update(rc=None, output="no workspace was recorded")
        else:
            cdir = os.path.join(run, "checkdir")
            clone_tree(result, cdir)
            clone_tree(os.path.join(root, "nm-cache", "node_modules"), os.path.join(cdir, "node_modules"))
            p = subprocess.run(["bash", "-c", cmd], cwd=cdir, capture_output=True, text=True, timeout=600,
                               env=dict(os.environ, CHECKS=os.path.join(HERE, "checks")))
            out.update(rc=p.returncode, output=(p.stdout + p.stderr)[-4000:])
            safe_rmtree(root, cdir)
    json.dump(out, open(os.path.join(run, "checks.json"), "w"), indent=1)


def after(run):
    """The run's time boundaries and its usage through settlement."""
    with open(os.path.join(run, "times.json"), "w") as f:
        subprocess.run([sys.executable, os.path.join(AB, "times.py"), "brigadier", run], check=True, stdout=f)
    settled = json.load(open(os.path.join(run, "times.json")))["settled_ms"]
    with open(os.path.join(run, "tokens.log"), "w") as f:
        subprocess.run([sys.executable, os.path.join(HERE, "v3tokens.py"), run, str(settled)], stdout=f, stderr=subprocess.STDOUT)


# ---------------------------------------------------------------- commands

def attempt(root, ask, regime, brain, n, tag, roots):
    """One attempt inside a batch: a harness error is kept as a failed attempt, and the batch goes on."""
    try:
        return one(root, ask, regime, brain, n, tag, roots)
    except Exception as err:  # noqa: BLE001
        label = tag or json.load(open(os.path.join(root, "build.json")))["label"]
        log(ask["id"], "HARNESS ERROR", repr(err))
        with open(os.path.join(root, "results.jsonl"), "a") as f:
            f.write(json.dumps({"run": f"{ask['id']}-{regime}-{brain}-{label}-{n}", "root": root, "ask": ask["id"],
                                "regime": regime, "brain": brain, "label": label, "n": n, "completed": False,
                                "pass": False, "why": f"harness error: {err!r}"}, sort_keys=True) + "\n")


def cmd_ask(a):
    root = check_root(a.root)
    for ask in v3lib.ask_files(a.ask):
        one(root, ask, a.regime, a.brain, a.n, a.tag, [root])


def cmd_set(a):
    root = check_root(a.root)
    for r in range(1, a.reruns + 1):
        for ask in v3lib.ask_files(a.asks):
            attempt(root, ask, a.regime, a.brain, a.first + r - 1, a.tag, [root])


def cmd_ab(a):
    """Two builds, alternating: for ask i and rerun r the pair runs A then B when i + r is even,
    B then A otherwise (A B B A ...), so neither build always goes first."""
    roots = dict(x.split("=", 1) for x in a.roots.split(","))
    names = sorted(roots)
    if len(names) != 2:
        sys.exit("ab: two builds, as --roots A=R1,B=R2")
    for name in names:
        roots[name] = check_root(roots[name])
    for r in range(1, a.reruns + 1):
        for i, ask in enumerate(v3lib.ask_files(a.asks)):
            order = names if (i + r) % 2 == 0 else names[::-1]
            for name in order:
                attempt(roots[name], ask, a.regime, a.brain, r, name, list(roots.values()))


def cmd_cleanup(a):
    """Stops anything this root started, exports the evidence (optional), deletes the Codex threads
    the root's runs made, the Claude project folders named after the root, Brigadier's temp folders
    owned by its data dirs, and then the root itself."""
    root = check_root(a.root)
    pidf = os.path.join(root, "pids")
    if os.path.exists(pidf):
        stop([int(x) for x in open(pidf).read().split()])
    if a.export:
        os.makedirs(a.export, exist_ok=True)
        for run in sorted(glob.glob(os.path.join(root, "runs", "*"))):
            dst = os.path.join(a.export, os.path.basename(run))
            if not os.path.exists(dst):
                shutil.copytree(run, dst, ignore=shutil.ignore_patterns("result", "checkdir"))
        for f in ("build.json", "results.jsonl", "seed"):
            p = os.path.join(root, f)
            if os.path.isdir(p):
                shutil.copytree(p, os.path.join(a.export, f), dirs_exist_ok=True)
            elif os.path.exists(p):
                shutil.copyfile(p, os.path.join(a.export, f))
        log("exported to", a.export)
    threads = set()
    for m in glob.glob(os.path.join(root, "runs", "*", "manifest-all.json")) + glob.glob(os.path.join(root, "template-meta", "manifest-all.json")):
        threads |= {s["id"] for s in json.load(open(m)) if s["provider"] == "codex"}
    for evf in glob.glob(os.path.join(root, "runs", "*", "rec", "events.jsonl")) + glob.glob(os.path.join(root, "template-meta", "rec", "events.jsonl")):
        for e in read_events(evf):
            if e["type"] == "cleanupRecorded" and e["artifact"]["type"] == "codexThread":
                threads.add(e["artifact"]["threadId"])
    threads = sorted(t for t in threads if re.fullmatch(r"[0-9a-f-]{36}", t))
    if threads:
        subprocess.run([sys.executable, os.path.join(HERE, "codex_threads.py"), *threads])
    for v in {root, root[len("/private"):]}:
        slug = re.sub(r"[^A-Za-z0-9]", "-", v)
        assert "-tmp-" in slug
        for d in glob.glob(os.path.join(HOME, ".claude", "projects", glob.escape(slug) + "*")):
            shutil.rmtree(d, ignore_errors=True); log("removed", d)
    for d in glob.glob("/tmp/brigadier-*") + glob.glob("/private/tmp/brigadier-*"):
        try:
            text = open(os.path.join(d, ".brigadier-owner")).read()
        except OSError:
            continue
        if root in text or root[len("/private"):] in text:
            shutil.rmtree(d, ignore_errors=True); log("removed", d)
    shutil.rmtree(root)
    log("removed", root)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("template"); t.add_argument("--root", required=True); t.add_argument("--seed"); t.add_argument("--tag")
    for name in ("ask", "set", "ab"):
        p = sub.add_parser(name)
        if name == "ab":
            p.add_argument("--roots", required=True)
        else:
            p.add_argument("--root", required=True); p.add_argument("--tag")
        p.add_argument("--ask" if name == "ask" else "--asks", required=(name == "ask"))
        p.add_argument("--regime", choices=["warm", "cold"], required=True)
        p.add_argument("--brain", choices=["seeded", "empty"], required=True)
        if name == "ask":
            p.add_argument("--n", type=int, default=1)
        else:
            p.add_argument("--reruns", type=int, default=1)
        if name == "set":
            p.add_argument("--first", type=int, default=1, help="number of the first rerun (to add reruns later)")
    c = sub.add_parser("cleanup"); c.add_argument("--root", required=True); c.add_argument("--export")
    a = ap.parse_args()
    {"template": template, "ask": cmd_ask, "set": cmd_set, "ab": cmd_ab, "cleanup": cmd_cleanup}[a.cmd](a)


if __name__ == "__main__":
    main()
