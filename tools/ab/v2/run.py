#!/usr/bin/env python3
"""The V2 continuity battery (PLAN §0 V2, BUILD-PLAN D1.0): several requests in one session, with
items planted early, pushed far out of the thread's verbatim tail, and needed late, across forced
seams of the kind under test, against a dev brigadierd with its own data dir. Every attempt is kept
with its evidence and score; `score.py` rescores a run from its directory alone.

A harness root (under /tmp) holds one build and one provider for the thread and its workers:
  build.json            the daemon (BRIGD), its source (BRIG_SRC), a label and the provider
  template/{data,repo}  the arm right after setup: onboarded, Full access, Fable hidden, Brain
                        enrichment off, every non-review kind of work on the provider's model, the
                        V2 fixture (fixture/build.sh) added as a project and its Brain jobs ended
  arm/{data,repo}       the live arm, restored from the template before every attempt
  runs/<id>/            one attempt: rec/, transcripts/, start.json, orchestrator.jsonl,
                        briefings/, freeze.json, checks.json, tokens.json, manifest.json, score.json
  results.jsonl         one line per attempt (every attempt, failed or not)

usage:
  run.py template --root R --provider claude|codex          (BRIGD, BRIG_SRC from the environment)
  run.py battery --root R --battery thread --seam SEAM [--n 1] [--tag T] [--rebirth-tokens N]
                 [--hard-line-tokens N]
  run.py set --root R --battery thread --seam SEAM --reruns 3 [--first 1]
  run.py cleanup --root R [--export DIR]                    (as tools/ab/v3/run.py cleanup)

Seams (each forced at least twice between the plants and the probe):
  size     the thread is reborn by size: BRIGADIER_REBIRTH_TOKENS (--rebirth-tokens, default 33500),
           with the briefing's cap at BRIGADIER_BRIEFING_TOKENS (--briefing-tokens, default 8000),
           so a short battery's early messages leave the verbatim tail as a long session's do
  inturn   the thread's turn is stopped mid-request and reborn at once: BRIGADIER_HARD_LINE_TOKENS
           (--hard-line-tokens, default 37500), with the briefing's cap as for size (BUILD-PLAN D2.3b)
  restart  the daemon is killed (SIGKILL) mid-request at each seam point, and started again
  kill     the thread's CLI process is killed (SIGKILL) mid-request at each seam point
  context  "prompt is too long" injected into the thread's turn at each seam point
  auth     a failed sign-in injected likewise
  billing  credits run out, injected likewise
  limit    a usage limit injected likewise (the session moves to another account when the data
           dir has one, else waits or falls back as the product does)
  none     no seam (a control)
Models: the thread on Claude Opus or gpt-6.1-sol at high; workers on the same provider's model.
Nothing here touches the installed app, its daemon or its data."""
import argparse, glob, json, os, re, shutil, signal, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
AB = os.path.dirname(HERE)
V3 = os.path.join(AB, "v3")
sys.path.insert(0, AB); sys.path.insert(0, V3)
from bipc import req  # noqa: E402
import run as v3run  # noqa: E402  (the V3 harness's process, template and record helpers)
import v3lib  # noqa: E402
sys.path.insert(0, HERE)
from v2lib import load_battery  # noqa: E402

log, now_ms = v3run.log, v3run.now_ms
MODELS = {"claude": "opus", "codex": "gpt-6.1-sol"}
SEAMS = ["size", "inturn", "restart", "kill", "context", "auth", "billing", "limit", "none"]
INJECTED = {"context": "contextWindow", "auth": "auth", "billing": "billing"}
# A reborn thread started near 29k (instructions, tools and an 8k briefing) until Phase O cut the
# thread's start by about 2.6k (17.5k to 14.9k of instructions and Brigadier tools, docs/evidence/
# benchmark-2026-10-16-phase-o.md): near 26.4k now. The line keeps the 7k of room above the start
# the 36k line had, where every 10-15 size run rebirthed four times between the last plant and the
# probe and never on it (a 10-16 run at 36k on the smaller start rebirthed three times). At 45k the
# second one fell on the probe in two of three runs; score.py now makes such a run invalid and names
# the step each rebirth fell in (seams.at_steps), so a line that drifts again shows (#191).
DEFAULT_REBIRTH_TOKENS = 33_500
DEFAULT_BRIEFING_TOKENS = 8_000
# The in-turn seam's hard line: a reborn thread starts near 26.4k (29k before Phase O; the line keeps
# the 11k above it that 40k had), so each later step that grows past 37.5k mid-turn is stopped and
# continued in a fresh session; the size rebirth (prepared at 300k, swapped at 375k on a 1M window
# since A10) never comes first.
DEFAULT_HARD_LINE_TOKENS = 37_500


# ---------------------------------------------------------------- the arm

class Arm:
    """The live daemon and recorder of one attempt. The daemon can be killed and started again on
    the same data dir (the restart seam); the recorder reconnects by itself."""

    def __init__(self, root, rec_dir, log_dir, debug_env=""):
        self.root, self.log_dir = root, log_dir
        self.data = os.path.join(root, "arm", "data"); self.repo = os.path.join(root, "arm", "repo")
        build = json.load(open(os.path.join(root, "build.json")))
        self.env = dict(os.environ, BRIGD=build["brigd"], BRIG_SRC=v3run.pinned_src(root, build), BRIG_DEBUG_ENV=debug_env)
        self.env.pop("BRIG_CLAUDE_HOME", None)
        os.makedirs(log_dir, exist_ok=True)
        self.daemon = None
        self.start_daemon()
        self.recorder = subprocess.Popen([sys.executable, os.path.join(AB, "recorder.py"), self.data, rec_dir],
                                         start_new_session=True, stdout=open(os.path.join(log_dir, "recorder.log"), "a"),
                                         stderr=subprocess.STDOUT)
        self._pid(self.recorder.pid)
        self.events = os.path.join(rec_dir, "events.jsonl")

    def _pid(self, pid):
        with open(os.path.join(self.root, "pids"), "a") as f:
            f.write(f"{pid}\n")

    def start_daemon(self):
        self.daemon = subprocess.Popen([os.path.join(AB, "startd.sh"), self.data], env=self.env, start_new_session=True,
                                       stdout=open(os.path.join(self.log_dir, "daemon.log"), "a"), stderr=subprocess.STDOUT)
        self._pid(self.daemon.pid)
        sock, token = os.path.join(self.data, "run", "brigadierd.sock"), os.path.join(self.data, "run", "ipc.token")
        for _ in range(240):
            if os.path.exists(sock) and os.path.exists(token) and os.path.getsize(token):
                try:
                    req(self.data, {"method": "getCatalog"}); return
                except Exception:
                    pass
            if self.daemon.poll() is not None:
                raise SystemExit(f"daemon exited: see {self.log_dir}/daemon.log")
            time.sleep(0.5)
        raise SystemExit("daemon did not start")

    def kill_daemon(self):
        """SIGKILL to the daemon alone, as a crash: its CLIs are left as they are."""
        pid = self.daemon.pid
        os.kill(pid, signal.SIGKILL)
        self.daemon.wait(30)
        for p in (os.path.join(self.data, "run", "brigadierd.sock"),):
            if os.path.exists(p):
                os.remove(p)
        return pid

    def close(self):
        v3run.stop([self.recorder.pid]); v3run.stop([self.daemon.pid], grace=30)


# ---------------------------------------------------------------- template

def template(a):
    root = v3run.check_root(a.root)
    brigd, src = os.environ.get("BRIGD"), os.environ.get("BRIG_SRC") or os.path.realpath(os.path.join(AB, "..", ".."))
    if not brigd or brigd.startswith("/Applications/"):
        sys.exit("template: set BRIGD to a dev brigadierd (never the installed app's)")
    os.makedirs(root, exist_ok=True)
    if os.path.exists(os.path.join(root, "template")):
        sys.exit(f"template: {root}/template exists")
    sha = subprocess.run(["git", "-C", src, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(["git", "-C", src, "status", "--porcelain", "--", "crates"], capture_output=True, text=True).stdout.strip()
    json.dump({"brigd": os.path.realpath(brigd), "src": os.path.realpath(src), "src_commit": sha, "src_crates_dirty": bool(dirty),
               "label": a.tag or sha[:8], "brigd_sha256": v3run.sha256(brigd), "provider": a.provider, "made_ms": now_ms()},
              open(os.path.join(root, "build.json"), "w"), indent=1)
    os.makedirs(os.path.join(root, "arm"), exist_ok=True)
    out = subprocess.run([os.path.join(HERE, "fixture", "build.sh"), os.path.join(root, "arm", "repo"), os.path.join(root, "nm-cache")],
                         check=True, capture_output=True, text=True).stdout
    log("fixture", out.strip())
    meta = os.path.join(root, "template-meta"); os.makedirs(meta, exist_ok=True)
    arm = Arm(root, os.path.join(meta, "rec"), meta)
    try:
        subprocess.run([sys.executable, os.path.join(AB, "setup.py"), arm.data, arm.repo, "--permission", "fullAccess"],
                       check=True, stdout=open(os.path.join(meta, "setup.log"), "w"))
        s = req(arm.data, {"method": "getCatalog"})["value"]["catalog"]["settings"]
        s["enrichBrain"] = False
        s["routingOverrides"] = [{"id": "v2-workers", "effect": "only",
                                  "target": {"type": "model", "provider": a.provider, "id": MODELS[a.provider]},
                                  "categories": ["scout", "research", "implement", "merge", "verify", "operate"],
                                  "areas": [], "projectId": None, "createdAtMs": now_ms()}]
        log("settings:", req(arm.data, {"method": "updateSettings", "settings": s})["status"])
        subprocess.run([sys.executable, os.path.join(AB, "brainwait.py"), meta, "--max", "300"], check=True,
                       stdout=open(os.path.join(meta, "brainwait.log"), "w"))
        log("Brain jobs ended")
    finally:
        arm.close()
    v3run.clone_tree(os.path.join(root, "arm"), os.path.join(root, "template"))
    log("template ready")


# ---------------------------------------------------------------- one attempt

def create_conversation(data, repo, provider):
    cat = req(data, {"method": "getCatalog"})["value"]["catalog"]
    real = os.path.realpath(repo)
    proj = [p for p in cat["projects"] if p["repos"] and os.path.realpath(p["repos"][0]["path"]) == real][0]
    setup = {"type": "session", "repo": proj["repos"][0]["path"], "environment": {"type": "newWorktree", "base": "main", "branch": None},
             "permission": cat["settings"]["defaultPermission"],
             "orchestrator": {"provider": provider, "model": MODELS[provider], "effort": "high"}, "planMode": False}
    r = req(data, {"method": "createConversation", "kind": "session", "projectId": proj["id"], "title": None, "setup": setup})
    return r["value"]["conversation"]["id"]


def workspace(evpath, conv):
    ws = None
    for e in v3run.read_events(evpath):
        if e["type"] == "conversationSetUp" and e["id"] == conv:
            ws = ((e.get("setup") or {}).get("environment") or {}).get("path") or ws
    return ws


def orchestrator_log(data, conv):
    """The conversation's whole orchestrator log, oldest first."""
    entries, before = [], None
    while True:
        r = req(data, {"method": "listOrchestratorLog", "conversationId": conv, "before": before, "limit": 500})
        page = r["value"]["page"]
        entries += page["entries"]
        if not page["hasMore"] or not page["entries"]:
            break
        before = min(e["streamSeq"] for e in page["entries"])
    return sorted(entries, key=lambda e: e["streamSeq"])


def thread_native_ids(data, conv):
    ids = []
    for e in orchestrator_log(data, conv):
        ev = (e["entry"].get("event") if e["entry"].get("type") == "provider" else None) or {}
        if ev.get("type") == "sessionStarted" and ev.get("nativeId") and ev["nativeId"] not in ids:
            ids.append(ev["nativeId"])
    return ids


def kill_thread_cli(arm, conv):
    """SIGKILL to the thread's running CLI: a process under the daemon whose command line names the
    thread's newest CLI session."""
    ids = thread_native_ids(arm.data, conv)
    if not ids:
        return None
    tree = set(v3run.descendants([arm.daemon.pid]))
    out = subprocess.run(["ps", "-Ao", "pid=,command="], capture_output=True, text=True).stdout
    for line in out.splitlines():
        pid, _, command = line.strip().partition(" ")
        if int(pid) in tree and ids[-1] in command:
            os.kill(int(pid), signal.SIGKILL)
            return {"pid": int(pid), "session": ids[-1]}
    return None


def turn_started(data, conv, since_ms):
    """Whether the thread's CLI reported anything since `since_ms` (it began the turn)."""
    try:
        entries = orchestrator_log(data, conv)
    except Exception:  # noqa: BLE001 - a daemon still starting
        return False
    return any(e["atMs"] >= since_ms and e["entry"].get("type") == "provider"
               and (e["entry"].get("event") or {}).get("type") not in ("sessionStarted", None) for e in entries)


def seam(arm, kind, conv, rid, sent_ms, deadline, record):
    """Forces one seam into the running request `rid` (sent at `sent_ms`)."""
    entry = {"kind": kind, "request": rid, "at_ms": now_ms()}
    if kind in INJECTED or kind == "limit":
        provider = json.load(open(os.path.join(arm.root, "build.json")))["provider"]
        if kind == "limit":
            body = {"method": "debugInjectLimit", "target": {"type": "conversation", "conversationId": conv},
                    "provider": provider, "window": "five_hour" if provider == "claude" else "primary",
                    "resetInMinutes": 60, "afterToolCalls": 1}
        else:
            body = {"method": "debugInjectError", "target": {"type": "conversation", "conversationId": conv},
                    "provider": provider, "error": INJECTED[kind], "afterToolCalls": 1}
        while time.time() < deadline:
            r = req(arm.data, body)
            if r.get("status") == "ok":
                entry["armed_ms"] = now_ms(); break
            entry["last_error"] = json.dumps(r)[:300]
            time.sleep(1)
    elif kind in ("restart", "kill"):
        # Mid-request: once the thread has begun the turn (or 20 s after the send).
        while time.time() < deadline and not turn_started(arm.data, conv, sent_ms) and now_ms() - sent_ms < 20_000:
            time.sleep(1)
        if kind == "restart":
            entry["killed_pid"] = arm.kill_daemon()
            time.sleep(2)
            arm.start_daemon()
        else:
            entry["killed"] = kill_thread_cli(arm, conv)
    entry["done_ms"] = now_ms()
    record.append(entry)
    log("seam", kind, json.dumps(entry)[:200])


def one(root, b, seam_kind, n, tag, rebirth_tokens, briefing_tokens=DEFAULT_BRIEFING_TOKENS,
        hard_line_tokens=DEFAULT_HARD_LINE_TOKENS):
    build = json.load(open(os.path.join(root, "build.json")))
    label = tag or build["label"]
    rid_ = f"{b['name']}-{seam_kind}-{build['provider']}-{label}-{n}"
    run = os.path.join(root, "runs", rid_)
    if os.path.exists(os.path.join(run, "score.json")):
        log(rid_, "scored already"); return json.load(open(os.path.join(run, "score.json")))
    if os.path.exists(run):
        v3run.safe_rmtree(root, run)
    os.makedirs(run)
    shutil.copyfile(b["path"], os.path.join(run, "battery.md"))
    v3run.restore(root, "seeded")
    debug_env = {"size": f"BRIGADIER_REBIRTH_TOKENS={rebirth_tokens} BRIGADIER_BRIEFING_TOKENS={briefing_tokens}",
                 "inturn": f"BRIGADIER_HARD_LINE_TOKENS={hard_line_tokens} BRIGADIER_BRIEFING_TOKENS={briefing_tokens}",
                 }.get(seam_kind, "")
    conditions = subprocess.run(["bash", "-c", "sysctl -n vm.loadavg; claude --version; codex --version; node --version"],
                                capture_output=True, text=True).stdout.split("\n")
    arm = Arm(root, os.path.join(run, "rec"), run, debug_env)
    h = b["harness"]
    start = {"arm": "brigadier", "battery": b["name"], "seam": seam_kind, "provider": build["provider"], "n": n,
             "build": build, "label": label, "debug_env": debug_env, "conditions_start": conditions,
             "steps": [], "requests": [], "sent_ms": [], "seams": [], "feed": {}, "planted_learned": None}
    try:
        time.sleep(5)
        conv = create_conversation(arm.data, arm.repo, build["provider"])
        deadline = time.time() + b["timeout"]
        start["conversation"] = conv
        timed_out = False
        for i, step in enumerate(b["steps"]):
            if step["id"] == h.get("feed", {}).get("copy_before"):
                ws = workspace(arm.events, conv)
                feed = os.path.join(ws, ".import-feed")
                os.makedirs(feed, exist_ok=True)
                for f in sorted(os.listdir(os.path.join(HERE, "fixture", "feed"))):
                    shutil.copyfile(os.path.join(HERE, "fixture", "feed", f), os.path.join(feed, f))
                start["feed"]["copied_ms"] = now_ms(); start["feed"]["path"] = feed
            if step["id"] == h.get("plant_learned", {}).get("before"):
                r = req(arm.data, {"method": "debugPlantLearned", "conversationId": conv, "text": h["plant_learned"]["text"]})
                start["planted_learned"] = {"status": r.get("status"), "at_ms": now_ms()}
            t, rid = v3run.send(arm.data, conv, step["text"] + " " + v3lib.SAFETY)
            if i == 0:
                start["t0_ms"] = t
            start["steps"].append(step["id"]); start["requests"].append(rid); start["sent_ms"].append(t)
            json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
            log(rid_, f"{step['id']} sent ({i + 1}/{len(b['steps'])})")
            if seam_kind not in ("size", "inturn", "none") and step["id"] in h.get("seam_points", []):
                seam(arm, seam_kind, conv, rid, t, deadline, start["seams"])
            r = v3run.wait_request(arm.events, conv, rid, deadline)
            if r is None:
                timed_out = True; break
            log(rid_, f"{step['id']} ended:", r["state"]["type"])
            if step["id"] == h.get("feed", {}).get("delete_after") and start["feed"].get("path"):
                v3run.settle(arm.events, conv, deadline)
                shutil.rmtree(start["feed"]["path"])
                start["feed"]["deleted_ms"] = now_ms()
            if i + 1 < len(b["steps"]):
                v3run.settle(arm.events, conv, deadline)
        settled = False if timed_out else v3run.settle(arm.events, conv, deadline + v3run.SETTLE_MAX_S)
        start["timed_out"] = timed_out; start["settled_quietly"] = settled
        json.dump(start, open(os.path.join(run, "start.json"), "w"), indent=1)
        freeze(root, run, arm, conv)
    finally:
        arm.close()
    v3run.backup(os.path.join(arm.data, "routing.sqlite"), v3run.ensure(os.path.join(run, "data")) + "/routing.sqlite")
    run_checks(root, run, h)
    v3run.after(run)
    score = json.loads(subprocess.run([sys.executable, os.path.join(HERE, "score.py"), run], check=True,
                                      capture_output=True, text=True).stdout)
    json.dump(score, open(os.path.join(run, "score.json"), "w"), indent=1, sort_keys=True)
    with open(os.path.join(root, "results.jsonl"), "a") as f:
        f.write(json.dumps({"run": rid_, "root": root, **score}, sort_keys=True) + "\n")
    log(rid_, "ALL PASS" if score["all_pass"] else "NOT ALL PASS", json.dumps(score["by_item"]))
    return score


def freeze(root, run, arm, conv):
    """While the arm still stands: the orchestrator log, every rebirth's briefing and note, and the
    session workspace's files and git state. The scorer reads only these and the record."""
    entries = orchestrator_log(arm.data, conv)
    with open(os.path.join(run, "orchestrator.jsonl"), "w") as f:
        for e in entries:
            f.write(json.dumps(e) + "\n")
    out = v3run.ensure(os.path.join(run, "briefings"))
    for e in entries:
        if e["entry"].get("type") != "rebirth":
            continue
        record = e["entry"]["record"]
        for key, name in (("briefingBlob", "briefing"), ("handoffBlob", "note")):
            if record.get(key):
                text = req(arm.data, {"method": "readBlobText", "hash": record[key]})["value"]["text"]
                open(os.path.join(out, f"{record['generation']:02d}-{name}.txt"), "w").write(text)
    ws = workspace(arm.events, conv); git = {}
    if ws and os.path.isdir(ws):
        files = subprocess.run(["git", "-C", ws, "ls-files", "-co", "--exclude-standard", "-z"], capture_output=True).stdout
        result = v3run.ensure(os.path.join(run, "result"))
        subprocess.run(["tar", "-C", ws, "--null", "-T", "-", "-cf", os.path.join(run, "result.tar")], input=files, check=True)
        subprocess.run(["tar", "-C", result, "-xf", os.path.join(run, "result.tar")], check=True)
        os.remove(os.path.join(run, "result.tar"))
        g = lambda *a: subprocess.run(["git", "-C", ws, *a], capture_output=True, text=True).stdout.strip()
        git = {"head": g("rev-parse", "HEAD"), "ahead_of_main": g("log", "--oneline", "main..HEAD").splitlines(),
               "status": g("status", "--porcelain").splitlines()}
    json.dump({"at_ms": now_ms(), "workspace": ws, "git": git}, open(os.path.join(run, "freeze.json"), "w"), indent=1, sort_keys=True)


def run_checks(root, run, harness):
    """Each edit step's hidden check, once, on a copy of the session workspace as the run left it."""
    out = {}
    for step, cmd in sorted(harness.get("checks", {}).items()):
        result = os.path.join(run, "result")
        if not os.path.isdir(result):
            out[step] = {"command": cmd, "rc": None, "output": "no workspace was recorded"}
            continue
        cdir = os.path.join(run, "checkdir")
        v3run.clone_tree(result, cdir)
        v3run.clone_tree(os.path.join(root, "nm-cache", "node_modules"), os.path.join(cdir, "node_modules"))
        p = subprocess.run(["bash", "-c", cmd], cwd=cdir, capture_output=True, text=True, timeout=600,
                           env=dict(os.environ, CHECKS=os.path.join(HERE, "checks")))
        text = p.stdout + p.stderr
        m = re.search(r"^RESULT (\{.*\})$", text, re.M)
        out[step] = {"command": cmd, "rc": p.returncode, "result": json.loads(m.group(1)) if m else None, "output": text[-4000:]}
        v3run.safe_rmtree(root, cdir)
    json.dump(out, open(os.path.join(run, "checks.json"), "w"), indent=1)


# ---------------------------------------------------------------- commands

def attempt(root, b, seam_kind, n, tag, rebirth_tokens, briefing_tokens, hard_line_tokens):
    try:
        return one(root, b, seam_kind, n, tag, rebirth_tokens, briefing_tokens, hard_line_tokens)
    except Exception as err:  # noqa: BLE001
        log(b["name"], "HARNESS ERROR", repr(err))
        with open(os.path.join(root, "results.jsonl"), "a") as f:
            f.write(json.dumps({"run": f"{b['name']}-{seam_kind}-{n}", "root": root, "battery": b["name"], "seam": seam_kind,
                                "n": n, "all_pass": False, "why": f"harness error: {err!r}"}, sort_keys=True) + "\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("template"); t.add_argument("--root", required=True); t.add_argument("--tag")
    t.add_argument("--provider", choices=sorted(MODELS), required=True)
    for name in ("battery", "set"):
        p = sub.add_parser(name)
        p.add_argument("--root", required=True); p.add_argument("--battery", default="thread")
        p.add_argument("--seam", choices=SEAMS, required=True); p.add_argument("--tag")
        p.add_argument("--rebirth-tokens", type=int, default=DEFAULT_REBIRTH_TOKENS)
        p.add_argument("--briefing-tokens", type=int, default=DEFAULT_BRIEFING_TOKENS)
        p.add_argument("--hard-line-tokens", type=int, default=DEFAULT_HARD_LINE_TOKENS)
        if name == "battery":
            p.add_argument("--n", type=int, default=1)
        else:
            p.add_argument("--reruns", type=int, default=3); p.add_argument("--first", type=int, default=1)
    c = sub.add_parser("cleanup"); c.add_argument("--root", required=True); c.add_argument("--export")
    a = ap.parse_args()
    if a.cmd == "template":
        template(a)
    elif a.cmd == "cleanup":
        v3run.cmd_cleanup(a)
    else:
        root = v3run.check_root(a.root)
        b = load_battery(a.battery)
        if a.cmd == "battery":
            one(root, b, a.seam, a.n, a.tag, a.rebirth_tokens, a.briefing_tokens, a.hard_line_tokens)
        else:
            for r in range(a.first, a.first + a.reruns):
                attempt(root, b, a.seam, r, a.tag, a.rebirth_tokens, a.briefing_tokens, a.hard_line_tokens)


if __name__ == "__main__":
    main()
