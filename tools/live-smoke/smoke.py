#!/usr/bin/env python3
"""The live smoke: this tree's daemon with the real Claude and Codex CLIs, headless.

It starts its own daemon on a scratch data dir under /tmp, makes two throwaway git projects,
three Claude sessions and one Codex session (cheap models, low effort, tiny prompts), runs the
scenarios below, reads every verdict from the daemon's own view (requests, steps, queue,
messages between sessions, turns metered) and leaves nothing behind.

  1  a long command in the thread, with a follow-up and a steer sent while it runs
  2  a thread's background job outlives its turn and its CLI: the result still reaches the user
  3  Claude -> Codex (idle receiver) and Codex -> Claude (receiver busy with other work),
     each with a wait: the reply is taken once, in the same turn, and carries only its answer
  4  Stop during a wait: the wait is cancelled, the late reply arrives once, no turn starts
  5  every request that finished ends on a real answer, not a narration line

usage: smoke.py --daemon <brigadierd> [--claude-model sonnet] [--codex-model gpt-6-luna]
                [--long 70] [--job 100] [--keep-report PATH] [--tag NAME]
Exit: 0 all passed (or skipped: a CLI isn't signed in), 1 a check failed, 2 the run broke.
"""
import argparse, json, os, re, secrets, shutil, signal, socket, sqlite3, struct, subprocess, sys, threading, time

SRC = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
WATCHDOG_S = 660
SETTLE_S = 12


# ----- the daemon's IPC, with timeouts -------------------------------------------------------

def protocol_of(src):
    text = open(os.path.join(src, "crates/ipc/src/protocol.rs")).read()
    return int(re.search(r"PROTOCOL_VERSION: u32 = (\d+);", text).group(1))


def trust_left(root):
    """The folders under `root` that Claude's or Codex's own settings still trust."""
    roots = {root, root[len("/private"):] if root.startswith("/private/") else "/private" + root}
    under = lambda path: any(path == r or path.startswith(r + "/") for r in roots)
    left = []
    try:
        projects = json.load(open(os.path.expanduser("~/.claude.json"))).get("projects", {})
        left += [p for p, entry in projects.items() if under(p) and entry.get("hasTrustDialogAccepted")]
    except FileNotFoundError:
        pass
    except (OSError, ValueError, AttributeError):  # unchecked: counts as still there
        left.append("~/.claude.json (unreadable)")
    try:
        table = None  # the `[projects."<folder>"]` table the lines are in (either quote style)
        for line in open(os.path.expanduser("~/.codex/config.toml")):
            header = re.match(r"""\s*\[(?:projects\.(?:"([^"]*)"|'([^']*)')|.*)\]\s*(#.*)?$""", line)
            if header:
                table = header.group(1) or header.group(2)
            elif table and under(table) and re.match(r"""\s*trust_level\s*=\s*["']trusted["']""", line):
                left.append(table)
    except FileNotFoundError:
        pass
    except (OSError, ValueError):
        left.append("~/.codex/config.toml (unreadable)")
    return left


class Ipc:
    def __init__(self, data):
        self.data, self.proto = data, protocol_of(SRC)
        self.lock, self.next_id = threading.Lock(), 0

    def _frame(self, s, obj):
        b = json.dumps(obj).encode()
        s.sendall(struct.pack(">I", len(b)) + b)

    def _read(self, s):
        def exactly(n):
            buf = b""
            while len(buf) < n:
                chunk = s.recv(n - len(buf))
                if not chunk:
                    raise ConnectionError("the daemon closed the connection")
                buf += chunk
            return buf
        return json.loads(exactly(struct.unpack(">I", exactly(4))[0]))

    def call(self, request, timeout=30):
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(timeout)
        try:
            s.connect(os.path.join(self.data, "run", "brigadierd.sock"))
            token = open(os.path.join(self.data, "run", "ipc.token")).read().strip()
            self._frame(s, {"type": "hello", "token": token, "protocol": self.proto,
                            "client": {"name": "live-smoke", "pid": os.getpid()}})
            self._read(s)
            with self.lock:
                self.next_id += 1
                rid = self.next_id
            self._frame(s, {"type": "request", "id": rid, "request": request})
            while True:
                r = self._read(s)
                if r.get("type") == "response" and r.get("id") == rid:
                    result = r["result"]
                    if result.get("status") != "ok":
                        raise RuntimeError(f"{request.get('method')}: {json.dumps(result)[:400]}")
                    return result["value"]
        finally:
            s.close()

    def view(self, conv):
        return self.call({"method": "getConversation", "id": conv, "limit": 200})["view"]


# ----- results -------------------------------------------------------------------------------

RESULTS = []
RESULTS_LOCK = threading.Lock()


def check(scenario, name, ok, why_not="", info=""):
    """One verdict; `why_not` is said only when it fails, `info` either way."""
    detail = "; ".join(x for x in (info, "" if ok else why_not) if x)
    with RESULTS_LOCK:
        RESULTS.append({"scenario": scenario, "check": name, "ok": bool(ok), "detail": detail})
    print(f"  {'PASS' if ok else 'FAIL'}  [{scenario}] {name}" + (f": {detail}" if detail else ""), flush=True)
    return ok


def log(msg):
    print(f"{time.strftime('%H:%M:%S')} {msg}", flush=True)


def until(fn, timeout, every=1.0):
    """fn() until it returns something truthy or the time is up (then None)."""
    end = time.time() + timeout
    while True:
        value = fn()
        if value:
            return value
        if time.time() >= end:
            return None
        time.sleep(every)


def now_ms():
    return int(time.time() * 1000)


def code(prefix):
    return prefix + secrets.token_hex(3).upper()


# ----- reading the view ----------------------------------------------------------------------

def messages(v):
    return v["messages"]["messages"]


def tool_steps(v, name=None):
    out = []
    for s in v["orchestratorSteps"]:
        k = s["kind"]
        if k.get("type") == "tool" and (name is None or k["name"] == name):
            out.append({**k, "requestId": s.get("requestId"), "atMs": s["atMs"]})
    return out


def state_of(req):
    return req["state"]["type"]


def assistant_after(v, ms, text):
    """The assistant messages written after `ms` that contain `text` (any case)."""
    return [m for m in messages(v) if m["role"] == "assistant" and m["createdAtMs"] >= ms
            and text.lower() in m["text"].lower()]


def turns(ipc_data, conv, from_ms, to_ms):
    """The thread's own turns the daemon metered for `conv` between the two times."""
    path = os.path.join(ipc_data, "routing.sqlite")
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=10)
    try:
        return db.execute("SELECT COUNT(*) FROM turn_usage WHERE conversation_id = ? AND task_id IS NULL "
                          "AND at_ms BETWEEN ? AND ?", (conv, from_ms, to_ms)).fetchone()[0]
    finally:
        db.close()


# ----- the run -------------------------------------------------------------------------------

class Smoke:
    def __init__(self, args):
        self.args = args
        stamp = time.strftime("%Y%m%d-%H%M%S") + f"-{os.getpid()}"
        self.root = os.path.realpath("/tmp") + f"/live-smoke-{args.tag + '-' if args.tag else ''}{stamp}"
        self.data = self.root + "/data"
        self.proc = None
        self.convs = {}           # name -> conversation id
        self.native = {}          # name -> (provider, CLI session id)
        self.projects = []        # project ids, removed at cleanup
        self.expected = {}        # conversation -> {request id: [codes]}
        self.inputs = {}          # (name, item id) -> the tool call's input
        self.ipc = Ipc(self.data)

    # -- setup --

    def repo(self, name):
        path = f"{self.root}/{name}"
        os.makedirs(path + "/.smoke")
        git = ["git", "-C", path, "-c", "user.name=live-smoke", "-c", "user.email=live-smoke@localhost"]
        subprocess.run(["git", "init", "-q", "-b", "main", path], check=True)
        open(path + "/README.md", "w").write(f"# {name}\n\nA throwaway project of the live smoke.\n")
        open(path + "/.git/info/exclude", "a").write(".smoke/\n")
        subprocess.run(git + ["add", "README.md"], check=True)
        subprocess.run(git + ["commit", "-qm", "init"], check=True)
        return path

    def start_daemon(self):
        os.makedirs(self.data)
        path = ":".join(p for p in os.environ.get("PATH", "").split(":") if "cmux" not in p)
        env = {"HOME": os.environ["HOME"], "USER": os.environ.get("USER", ""), "LOGNAME": os.environ.get("LOGNAME", ""),
               "SHELL": "/bin/zsh", "TMPDIR": "/tmp", "LANG": "en_US.UTF-8", "PATH": path,
               "BRIGADIER_DATA_DIR": self.data}
        out = open(self.root + "/daemon.log", "w")
        self.proc = subprocess.Popen([self.args.daemon, "--foreground", "--data-dir", self.data], env=env,
                                     stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                                     start_new_session=True)
        log(f"daemon {self.proc.pid} on {self.data}")

        def ready():
            if self.proc.poll() is not None:
                raise RuntimeError(f"the daemon exited ({self.proc.returncode}); see {self.root}/daemon.log")
            try:
                return self.ipc.call({"method": "getCatalog"}, timeout=5)
            except (OSError, ConnectionError, RuntimeError):
                return None
        if not until(ready, 60, 0.5):
            raise RuntimeError("the daemon didn't answer within 60 s")

    def signed_in(self):
        """Which CLI isn't installed or signed in, as the daemon sees them (none: both are)."""
        def checked():
            view = self.ipc.call({"method": "getProviders"}, timeout=90)["view"]
            status = {p["provider"]: p for p in view["providers"]}
            return status if all((status.get(k) or {}).get("status") for k in ("claude", "codex")) else None
        status = until(checked, 60, 2) or {}
        missing = [k for k in ("claude", "codex") if not (((status.get(k) or {}).get("status")) or {}).get("loggedIn")]
        return missing, status

    def pick(self, status, provider, wanted):
        models = [m for m in (status[provider].get("models") or {}).get("models") or [] if not m.get("legacy")]
        ids = [m["id"] for m in models]
        if wanted in ids:
            return wanted
        fallback = next((m["id"] for m in models if m.get("isDefault")), ids[0] if ids else wanted)
        log(f"{provider}: model {wanted} not offered, using {fallback}")
        return fallback

    def setup(self):
        settings = self.ipc.call({"method": "getCatalog"})["catalog"]["settings"]
        settings.update(onboarded=True, introSeen=True, enrichBrain=False, defaultPermission="full")
        settings["hiddenModels"] = [{"provider": "claude", "id": i} for i in ("fable", "claude-fable-5", "claude-fable-5-1")]
        self.ipc.call({"method": "updateSettings", "settings": settings})
        missing, status = self.signed_in()
        if missing:
            return missing
        claude = self.pick(status, "claude", self.args.claude_model)
        codex = self.pick(status, "codex", self.args.codex_model)
        a, b = self.repo("projA"), self.repo("projB")
        projects = {}
        for name, path in (("A", a), ("B", b)):
            projects[name] = self.ipc.call({"method": "addProject", "path": path, "name": "", "init": False})["project"]["id"]
            # Trusted, as a user answers on entering the project; the project's removal at cleanup
            # takes the CLIs' trust entries this writes back out.
            self.ipc.call({"method": "setFolderTrust", "id": projects[name], "path": None, "trusted": True})
            self.projects.append(projects[name])
        for name, project, path, provider, model in (("C1", "A", a, "claude", claude), ("C2", "A", a, "claude", claude),
                                                     ("C3", "A", a, "claude", claude),
                                                     ("X", "B", b, "codex", codex)):
            setup = {"type": "session", "repo": path, "environment": {"type": "localCheckout", "branch": "main", "createFrom": None},
                     "permission": "full", "orchestrator": {"provider": provider, "model": model, "effort": "low"},
                     "planMode": False}
            conv = self.ipc.call({"method": "createConversation", "kind": "session", "projectId": projects[project],
                                  "title": f"Smoke {name}", "setup": setup})["conversation"]["id"]
            self.convs[name] = conv
            self.native[name] = (provider, None)
            self.expected[conv] = {}
        log(f"sessions: C1, C2, C3 Claude {claude} low in A; X Codex {codex} low in B")
        self.projA, self.projB = a, b
        return []

    def send(self, name, text, steer=False, codes=()):
        conv = self.convs[name]
        out = self.ipc.call({"method": "sendMessage", "conversationId": conv, "text": text, "attachments": [],
                             "mentions": [], "steer": steer})["outcome"]
        msg = out.get("message") or {}
        rid = msg.get("requestId") or msg.get("id")
        if rid:
            self.expected[conv].setdefault(rid, []).extend(codes)
        return out, rid

    def note_native(self):
        for name, conv in self.convs.items():
            try:
                status = self.ipc.call({"method": "getConversationStatus", "conversationId": conv})["status"]
                if status.get("nativeId"):
                    self.native[name] = (status["provider"], status["nativeId"])
            except Exception:
                pass

    def view(self, name):
        return self.ipc.view(self.convs[name])

    def idle(self, name):
        v = self.view(name)
        return v["run"] in ("idle", "hibernated") and not any(state_of(r) == "working" for r in v["requests"]) and v

    def runs(self, name, step, needle):
        """Whether the tool call `step` of session `name` runs a command containing `needle`. A
        named command's row shows its name, not the command: the call's input has the command."""
        if needle in (step.get("detail") or ""):
            return True
        key = (name, step["itemId"])
        if not self.inputs.get(key):
            try:
                item = self.ipc.call({"method": "getThreadItem", "conversationId": self.convs[name],
                                      "itemId": step["itemId"]})["item"]
            except RuntimeError:
                return False
            self.inputs[key] = item.get("input") or ""
        return needle in self.inputs[key]

    def running_command(self, name, needle):
        v = self.view(name)
        return next((s for s in tool_steps(v) if s["status"] == "inProgress" and self.runs(name, s, needle)), None)

    # -- scenario 1: a long command, a follow-up and a steer during it --

    def scenario_long_command(self):
        sc = "1 long command"
        long = self.args.long
        cmd = f"sleep {long} && head -c6 /dev/urandom | xxd -p > .smoke/t1.txt && cat .smoke/t1.txt"
        t0 = now_ms()
        _, root = self.send("C1", f"Run exactly this shell command in the foreground and wait for it to finish, then tell me "
                                  f"what it printed: `{cmd}`")
        step = until(lambda: self.running_command("C1", f"sleep {long}"), 90)
        if not check(sc, "the command runs in the thread", step, "" if step else "no running `sleep` step within 90 s"):
            return
        cmd_seen = now_ms()
        time.sleep(3)
        follow_code, steer_code = code("F"), code("S")
        follow_out, _ = self.send("C1", f"Also, put the code {follow_code} in your answer.", codes=[follow_code])
        time.sleep(5)
        _, steer_id = self.send("C1", f"And put the code {steer_code} in your answer too.", steer=True, codes=[steer_code])
        steer_at = now_ms()
        deciding_since, deciding_max = {}, 0
        deferred, deferred_after, cmd_end = {}, None, None
        follow_deciding = False
        deadline = time.time() + long + 150
        t1 = None
        while time.time() < deadline:
            v = self.view("C1")
            for item in v["queue"]["items"]:
                if item.get("deciding") and follow_code in item["text"]:
                    follow_deciding = True
                if item.get("deciding"):
                    deciding_since.setdefault(item["id"], time.time())
                    deciding_max = max(deciding_max, time.time() - deciding_since[item["id"]])
                else:
                    deciding_since.pop(item["id"], None)
            running = any(s["status"] == "inProgress" and self.runs("C1", s, f"sleep {long}") for s in tool_steps(v))
            if not running and cmd_end is None:
                cmd_end = now_ms()
            steer_req = next((r for r in v["requests"] if r["id"] == steer_id), None)
            # heldBy, on the steered request or its queue item, names the command it is read after.
            held = [o["heldBy"] for o in [steer_req or {}] + [i for i in v["queue"]["items"] if steer_code in i["text"]]
                    if o.get("heldBy")]
            extra = {"heldBy": held[0]} if held else {}
            if running and extra:
                deferred.update(extra)
            if not running and deferred and deferred_after is None:
                deferred_after = extra
            if os.path.exists(self.projA + "/.smoke/t1.txt") and t1 is None:
                t1 = open(self.projA + "/.smoke/t1.txt").read().strip()
            if cmd_end and t1 and not v["queue"]["items"] and self.idle("C1"):
                break
            time.sleep(1)
        v = self.view("C1")
        if t1:
            self.expected[self.convs["C1"]].setdefault(root, []).append(t1)
        check(sc, "the follow-up isn't left Deciding (30 s at most)", deciding_max <= 30,
              f"Deciding for {deciding_max:.0f} s" + (" (still at the end)" if deciding_since else ""))
        reqs = {r["id"]: r for r in v["requests"]}
        # A message sent while the session works waits in the queue, Deciding, while the thread
        # judges it (#437); leaving the queue it becomes a message with a request of its own.
        check(sc, "the follow-up waits in the queue, Deciding", follow_out.get("type") == "queued" and follow_deciding,
              f"sent as {follow_out.get('type')}" + ("" if follow_deciding else ", never seen Deciding"))
        follow_msg = next((m for m in messages(v) if m["role"] == "user" and follow_code in m["text"]), {})
        follow_id = follow_msg.get("requestId") or follow_msg.get("id")
        if follow_id:
            self.expected[self.convs["C1"]].setdefault(follow_id, []).append(follow_code)
        routes = []
        for step in tool_steps(v, "route_follow_up"):
            try:
                routes.append(self.ipc.call({"method": "getThreadItem", "conversationId": self.convs["C1"],
                                             "itemId": step["itemId"]})["item"].get("input") or "")
            except RuntimeError:
                pass

        def joined(rid):
            # A thread inside a long command can't judge it, so after 20 s it joins the working
            # answer: steered into it, and a later message into the one before, so the chain
            # leads back to the running request. A thread that judged it to wait gives it its own.
            chain, seen = [], set()
            while rid in reqs and reqs[rid].get("steeredInto") and rid not in seen:
                seen.add(rid)
                rid = reqs[rid]["steeredInto"]
                chain.append(rid)
            return chain
        waited = any("wait" in str(r).lower() for r in routes)
        check(sc, "the follow-up joins the running request (or the thread judged it to wait)",
              root in joined(follow_id) or (waited and follow_id),
              f"steeredInto chain {joined(follow_id) or 'none'}" + ("" if follow_id else ", no message for it"),
              info="the thread judged it to wait" if waited and root not in joined(follow_id) else "")
        check(sc, "the steer goes into the running request", root in joined(steer_id),
              f"steeredInto chain {joined(steer_id) or 'none'}")
        check(sc, "while the command runs, the steer says it is read when the command ends", deferred,
              "no heldBy on the steered request or its queue item",
              info=f"heldBy {deferred['heldBy']}" if deferred else "")
        if deferred:
            check(sc, "that state clears once the command ended", not deferred_after, f"still {deferred_after}" if deferred_after else "")
        check(sc, "the steer is answered", assistant_after(v, steer_at, steer_code), f"no answer with {steer_code}")
        check(sc, "the follow-up is answered", assistant_after(v, cmd_seen, follow_code), f"no answer with {follow_code}")
        check(sc, "the command's real output reaches the user", t1 and assistant_after(v, t0, t1),
              "the command never finished" if not t1 else f"no answer with its output {t1}")
        stuck = [r["preview"][:40] for r in v["requests"] if state_of(r) == "working"]
        check(sc, "every request ended", not stuck, f"still working: {stuck}")

    # -- scenario 2: a thread's background job outlives its turn --

    def scenario_background_job(self):
        sc = "2 background job"
        long = self.args.job
        out = self.projA + "/.smoke/t2.txt"
        cmd = f"sleep {long} && head -c6 /dev/urandom | xxd -p > .smoke/t2.txt && cat .smoke/t2.txt"
        t0 = now_ms()
        _, root = self.send("C2", f"Start this shell command as a background job so that it keeps running after your turn ends: `{cmd}`. "
                        f"Don't wait for it: end your turn right away with one line saying it started. When it finishes, "
                        f"tell me what it printed.")
        turn_end, retired = None, None

        def started_then_rested():
            nonlocal turn_end, retired
            v = self.view("C2")
            started = any(self.runs("C2", s, f"sleep {long}") for s in tool_steps(v))
            if started and v["run"] != "running" and turn_end is None:
                turn_end = now_ms()
            if turn_end and retired is None and now_ms() - turn_end > 5000 and not os.path.exists(out):
                # What the idle timer does to a resting session: its CLI goes. A job the thread
                # still owes must survive that (or keep the CLI from going).
                try:
                    self.ipc.call({"method": "hibernate", "id": self.convs["C2"]})
                    retired = "its CLI was retired while the job ran"
                except RuntimeError as err:
                    retired = f"retiring its CLI was refused ({str(err)[:80]})"
            return os.path.exists(out)
        until(started_then_rested, long + 90)
        finished = os.path.exists(out)
        finished_ms = int(os.path.getmtime(out) * 1000) if finished else None
        check(sc, "the turn ended while the job still ran", turn_end and (not finished or turn_end < finished_ms),
              "the turn never ended" if not turn_end else ("" if not finished or turn_end < finished_ms
                                                           else "the turn lasted until the command finished (it ran in the foreground)"))
        if check(sc, "the job finished after its turn ended, its CLI retired meanwhile", finished,
                 f"no output {long + 90} s after it started: the job died with its turn or its CLI", info=retired or ""):
            token = open(out).read().strip()
            heard = until(lambda: assistant_after(self.view("C2"), finished_ms, token), 90)
            check(sc, "its result reaches the thread and the user hears it", heard, f"no answer with its output {token} 90 s after it")
            if heard:
                check(sc, "it is told in its request's block", heard[0].get("requestId") == root,
                      f"the answer belongs to {heard[0].get('requestId') or 'no request'}")
            self.expected[self.convs["C2"]].setdefault(root, []).append(token)
        until(lambda: self.idle("C2"), 60)
        v = self.view("C2")
        stuck = [r["preview"][:40] for r in v["requests"] if state_of(r) == "working"]
        check(sc, "its request ended", not stuck, f"still working: {stuck}")
        if retired and retired.startswith("its CLI was retired"):
            # The session goes on as usual after its CLI went: the next request runs and is answered in its own block.
            next_code = code("K")
            _, rid = self.send("C2", f"Reply with the code {next_code} and nothing else.", codes=[next_code])
            until(lambda: self.idle("C2"), 90)
            v = self.view("C2")
            mine = [m for m in messages(v) if m["role"] == "assistant" and m.get("requestId") == rid
                    and next_code.lower() in m["text"].lower()]
            stray = [m["text"][:80] for m in messages(v) if m["role"] == "assistant" and not m.get("requestId")]
            check(sc, "after its CLI went, the next request is answered in its own block", mine,
                  f"no answer with {next_code} in it" + (f"; answers outside any request: {stray}" if stray else ""))

    # -- scenario 3: messages with a wait, both ways --

    def message_with_wait(self, sc, sender, receiver, reply_code, unrelated=None):
        to = self.convs[receiver]
        start = now_ms()
        prompt = (f"Find the session titled \"Smoke {receiver}\" with list_sessions and use message_session to send it this "
                  f"message: \"Reply with the code {reply_code} and nothing else.\" Then call wait_for_replies, and tell me "
                  f"the reply you got.")
        _, rid = self.send(sender, prompt, codes=[reply_code])
        until(lambda: self.idle(sender) and self.idle(receiver), 150)
        time.sleep(SETTLE_S)
        v, rv = self.view(sender), self.view(receiver)
        out = [p for p in v["peerMessages"] if p["outgoing"] and p["peer"] == to and p.get("requestId") == rid]
        sent = out[0] if out else None
        check(sc, f"{sender} sent one message to {receiver}", len(out) == 1, f"{len(out)} sent")
        if not sent:
            return
        replies = [p for p in v["peerMessages"] if not p["outgoing"] and p.get("replyTo") == sent["messageId"]]
        check(sc, "its state is replied", sent["state"]["type"] == "replied", f"state {sent['state']['type']}")
        waits = [s for s in tool_steps(v, "wait_for_replies") if s.get("requestId") == rid]
        completed = any(s["status"] == "completed" for s in waits)
        check(sc, "a wait_for_replies ran and completed", completed,
              f"waits: {[s['status'] for s in waits]}" + ("" if completed else f"; {self.wait_calls(sender, rid)}"))
        check(sc, "exactly one reply came back", len(replies) == 1, f"{len(replies)} replies")
        hold = (replies[0].get("hold") or {}).get("type") if replies else None
        check(sc, "the wait took the reply", hold == "taken", f"hold {hold}")
        # One turn: one stretch of work and one answer (a reply pushed after the turn would add both);
        # a Claude thread's turns are also metered one each (Codex meters every model call).
        req = next((r for r in v["requests"] if r["id"] == rid), {})
        said = [m for m in messages(v) if m["role"] == "assistant" and m.get("requestId") == rid]
        # A CLI that marks its final answer (Codex) also keeps its progress notes as assistant
        # messages, folded into the work: only the marked ones are answers then (#63).
        answers = [m for m in said if m.get("answerAtMs")] or said
        spans = len(req.get("worked") or [])
        n = turns(self.data, self.convs[sender], start, now_ms()) if self.native[sender][0] == "claude" else 1
        check(sc, f"{sender} answered in one turn", spans == 1 and len(answers) == 1 and n == 1,
              f"{spans} stretches of work, {len(answers)} answers, {n} turns metered",
              info=f"{len(said) - len(answers)} progress notes" if len(said) > len(answers) else "")
        check(sc, "the answer carries the reply", assistant_after(v, start, reply_code), f"no answer with {reply_code}")
        got = [p for p in rv["peerMessages"] if not p["outgoing"] and p["peer"] == self.convs[sender]
               and p["messageId"] == sent["messageId"]]
        check(sc, f"{receiver} got it once", len(got) == 1, f"{len(got)} received")
        back = [p for p in rv["peerMessages"] if p["outgoing"] and p.get("replyTo") == sent["messageId"]]
        check(sc, f"{receiver} replied once", len(back) == 1, f"{len(back)} replies")
        if back:
            # The receiver's record of what it sent: the sender's own record of a reply keeps no text.
            text = back[0]["text"]
            check(sc, "the reply carries its answer", reply_code.lower() in text.lower(), f"reply: {text[:120]!r}")
            if unrelated:
                check(sc, "the reply leaves out the receiver's other work", unrelated.lower() not in text.lower(),
                      f"reply: {text[:160]!r}")
        for p in got:
            if p.get("requestId"):
                self.expected[to].setdefault(p["requestId"], []).append(reply_code)

    def wait_calls(self, name, rid):
        """What `name`'s wait_for_replies calls in request `rid` were given and answered, as its
        tool log holds them: a failed wait's error says which way it ended (#456)."""
        try:
            page = self.ipc.call({"method": "listThreadItems", "conversationId": self.convs[name],
                                  "requestId": rid, "turn": None, "after": None, "limit": 200})["page"]
        except Exception as err:
            return f"its calls couldn't be read: {err}"
        calls = [json.dumps(e)[:600] for e in page["entries"] if "wait_for_replies" in json.dumps(e)]
        return "calls: " + (" | ".join(calls) or "none in its tool log")

    def scenario_messages(self):
        self.message_with_wait("3a Claude->Codex, idle", "C3", "X", code("A"))
        # Codex -> Claude while the Claude thread is busy with other work.
        unrel = code("U")
        open(self.projA + "/.smoke/unrel.txt", "w").write(unrel + "\n")
        _, rid = self.send("C1", "Run exactly this shell command and wait for it, then tell me what it printed: "
                                 "`sleep 40 && cat .smoke/unrel.txt`", codes=[unrel])
        if not check("3b Codex->Claude, busy", "the receiver's own command runs", until(lambda: self.running_command("C1", "sleep 40"), 90),
                     "no running `sleep 40` step within 90 s"):
            return
        self.message_with_wait("3b Codex->Claude, busy", "X", "C1", code("B"), unrelated=unrel)

    # -- scenario 4: Stop during a wait --

    def scenario_stop(self):
        sc = "4 stop during a wait"
        to = self.convs["C1"]
        # The receiver is busy with its own user's command, so its reply comes late. A Claude thread takes a
        # message steered into its turn only once its command returns; a Codex thread can answer it mid-command,
        # at once, so a Codex receiver doesn't make the reply late.
        busy = code("W")
        self.send("C1", f"Run exactly this shell command in the foreground and wait for it, then reply with the code {busy}: "
                        f"`sleep 45`", codes=[busy])
        if not check(sc, "the receiver is busy with a command", until(lambda: self.running_command("C1", "sleep 45"), 60),
                     "no running `sleep 45` step on C1 within 60 s"):
            return
        late = code("R")
        _, rid = self.send("C3", f"Find the session titled \"Smoke C1\" with list_sessions and use message_session to send it "
                                 f"this message: \"Reply with the code {late}.\" Then call wait_for_replies, and tell me "
                                 f"the reply you got.")
        waiting = until(lambda: next((s for s in tool_steps(self.view("C3"), "wait_for_replies")
                                      if s.get("requestId") == rid and s["status"] == "inProgress"), None), 120)
        if not check(sc, "the wait starts", waiting, "no running wait_for_replies within 120 s"):
            return
        self.ipc.call({"method": "interrupt", "conversationId": self.convs["C3"]})
        stopped_at = now_ms()

        def late_reply():
            v = self.view("C3")
            ids = {p["messageId"] for p in v["peerMessages"] if p["outgoing"] and p.get("requestId") == rid}
            return [p for p in v["peerMessages"] if not p["outgoing"] and p.get("replyTo") in ids]
        until(late_reply, 120)
        replies = late_reply()
        time.sleep(SETTLE_S + 8)
        v = self.view("C3")
        req = next((r for r in v["requests"] if r["id"] == rid), None)
        check(sc, "the request is stopped", req and state_of(req) == "stopped", f"state {state_of(req) if req else 'none'}")
        waits = [s["status"] for s in tool_steps(v, "wait_for_replies") if s.get("requestId") == rid]
        check(sc, "the wait is cancelled", waits and all(s == "failed" for s in waits), f"waits: {waits}")
        check(sc, "the late reply arrives once", len(replies) == 1, f"{len(replies)} replies")
        if replies:
            sent_back = [p for p in self.ipc.view(to)["peerMessages"] if p["outgoing"] and p["messageId"] == replies[0]["messageId"]]
            text = sent_back[0]["text"] if sent_back else ""
            check(sc, "it carries its answer", late.lower() in text.lower(), f"reply: {text[:120]!r}")
            shown = [m for m in messages(v) if m["role"] == "peer" and m["createdAtMs"] >= waiting["atMs"]]
            check(sc, "it shows once as a message", len(shown) == 1, f"{len(shown)} messages")
            arrived = min((m["createdAtMs"] for m in shown), default=now_ms())
            check(sc, "it came after the Stop", arrived > stopped_at, f"it came {(stopped_at - arrived) / 1000:.1f} s before")
            n = turns(self.data, self.convs["C3"], arrived + 1000, now_ms())
            after = [m for m in messages(v) if m["role"] == "assistant" and m["createdAtMs"] > arrived]
            check(sc, "no turn starts for it", n == 0 and not after, f"{n} turns metered, {len(after)} answers after it")
        new = [r for r in v["requests"] if r["startedAtMs"] > stopped_at]
        check(sc, "no request starts after the Stop", not new, f"{len(new)} new")
        until(lambda: self.idle("C1"), 60)

    # -- scenario 5: every finished request ends on a real answer --

    def scenario_answers(self):
        sc = "5 final answers"
        for name, conv in self.convs.items():
            v = self.ipc.view(conv)
            reqs = {r["id"]: r for r in v["requests"]}
            families = {}
            for r in v["requests"]:
                root = r["id"]
                while reqs.get(root, {}).get("steeredInto") in reqs:
                    root = reqs[root]["steeredInto"]
                families.setdefault(root, []).append(r["id"])
            for root, members in families.items():
                if state_of(reqs[root]) != "done":
                    continue
                said = [m for m in messages(v) if m["role"] == "assistant" and m.get("requestId") in members]
                codes = [c for rid in members for c in self.expected[conv].get(rid, [])]
                # A message queued before it became a request: its code is in its own words.
                known = {c for cs in self.expected[conv].values() for c in cs}
                codes += [c for c in known if any(c in reqs[rid]["preview"] for rid in members) and c not in codes]
                preview = reqs[root]["preview"][:50]
                if not said:
                    check(sc, f"{name}: “{preview}” has an answer", False, "no assistant message")
                    continue
                last = max(said, key=lambda m: m["seq"])["text"].strip()
                ok = bool(last) and not last.startswith("[quiet]") and (not codes or any(c.lower() in last.lower() for c in codes))
                check(sc, f"{name}: “{preview}” ends on its answer", ok,
                      f"last text {last[:100]!r}" + (f", expected one of {codes}" if codes else ""))

    # -- cleanup --

    def claude_dirs(self):
        base = os.path.expanduser("~/.claude/projects")
        return [os.path.join(base, re.sub(r"[^A-Za-z0-9]", "-", f"{self.data}/orch/{conv}")) for conv in self.convs.values()]

    def codex_files(self, thread):
        found = []
        for top in (os.path.expanduser("~/.codex/sessions"), os.path.expanduser("~/.codex/archived_sessions")):
            for d, _, files in os.walk(top):
                found += [os.path.join(d, f) for f in files if thread in f]
        return found

    def codex_delete(self, thread):
        """Deletes a Codex thread with the CLI's own app-server (`thread/delete`)."""
        p = subprocess.Popen(["codex", "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, text=True)
        try:
            for msg in ({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "live-smoke", "version": "0"}}},
                        {"method": "initialized"},
                        {"id": 2, "method": "thread/delete", "params": {"threadId": thread}}):
                p.stdin.write(json.dumps(msg) + "\n")
                p.stdin.flush()
            end = time.time() + 20
            while time.time() < end:
                line = p.stdout.readline()
                if not line:
                    break
                if json.loads(line).get("id") == 2:
                    break
        finally:
            p.kill()
            p.wait()

    def cleanup(self):
        log("cleaning up")
        left = []
        if self.proc and self.proc.poll() is None:
            for conv in self.convs.values():
                try:
                    self.ipc.call({"method": "interrupt", "conversationId": conv}, timeout=10)
                except Exception:
                    pass
            if self.convs:
                try:
                    self.ipc.call({"method": "delete", "ids": list(self.convs.values())}, timeout=30)

                    def gone():
                        listed = {c["id"] for c in self.ipc.call({"method": "getCatalog"}, timeout=10)["catalog"]["conversations"]}
                        return not (listed & set(self.convs.values()))
                    if not until(gone, 60, 1):
                        left.append("the daemon didn't finish deleting the sessions within 60 s")
                    time.sleep(3)
                except Exception as err:
                    left.append(f"deleting the sessions failed: {err}")
            for project in self.projects:
                try:
                    report = self.ipc.call({"method": "removeProject", "id": project, "deleteBranches": [],
                                            "keepBrain": True}, timeout=60)["report"]
                    left += [f"removing a project: {failure}" for failure in report.get("failures", [])]
                except Exception as err:
                    left.append(f"removing a project failed (its folder trust may stay in the CLIs' settings): {err}")
            try:
                self.ipc.call({"method": "shutdown"}, timeout=15)
            except Exception:
                pass
            try:
                self.proc.wait(20)
            except subprocess.TimeoutExpired:
                self.proc.terminate()
                try:
                    self.proc.wait(10)
                except subprocess.TimeoutExpired:
                    self.proc.kill()
                    self.proc.wait()
        if self.proc:
            try:  # the CLIs it started, in its own process group
                os.killpg(self.proc.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
        for d in self.claude_dirs():
            if os.path.exists(d):
                left.append(f"Claude's session dir was left by the delete (removed now): {d}")
                shutil.rmtree(d, ignore_errors=True)
        for name, (provider, thread) in self.native.items():
            if provider == "codex" and thread and self.codex_files(thread):
                self.codex_delete(thread)
                files = self.codex_files(thread)
                left.append(f"Codex thread {thread} was left by the delete" + (f"; still there: {files}" if files else " (deleted now)"))
        trusted = trust_left(self.root)
        if trusted:
            # Its daemon's records undo that trust: kept, for a daemon on this data folder to finish.
            left.append(f"the CLIs' settings still trust {trusted}; {self.root} is kept (its records undo that)")
        elif os.path.isdir(self.root):
            if self.args.keep_report:
                shutil.copy(self.root + "/daemon.log", self.args.keep_report + ".daemon.log")
            shutil.rmtree(self.root, ignore_errors=True)
        if os.path.exists(self.root):
            left.append(f"couldn't remove {self.root}")
        for line in left:
            log(f"cleanup: {line}")
        log("nothing left behind" if not left else f"cleanup notes: {len(left)}")
        return left


class Watchdog(Exception):
    pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--daemon", required=True)
    ap.add_argument("--claude-model", default="sonnet")
    ap.add_argument("--codex-model", default="gpt-6-luna")
    ap.add_argument("--long", type=int, default=70, help="seconds the long command runs")
    # Well past the minute after which a command goes on in the background, so the turn that
    # started it has time to end before it does (#279).
    ap.add_argument("--job", type=int, default=100, help="seconds the background job runs")
    ap.add_argument("--keep-report", help="also write the results as JSON here")
    ap.add_argument("--tag", default="", help="a name for its scratch dir: /tmp/live-smoke-<tag>-<time>-<pid>")
    args = ap.parse_args()
    smoke = Smoke(args)

    def expired(*_):
        raise Watchdog()
    signal.signal(signal.SIGALRM, expired)
    signal.signal(signal.SIGTERM, expired)
    signal.alarm(WATCHDOG_S)
    broke, skipped, notes = None, None, []
    started = time.time()
    try:
        smoke.start_daemon()
        missing = smoke.setup()
        if missing:
            skipped = f"SKIPPED: {' and '.join(missing)} not signed in (sign in, then run it again)"
        else:
            log("phase 1: a long command with a follow-up and a steer (C1), a background job (C2)")
            # A scenario that raises breaks the run, as it would on the main thread.
            raised = []

            def scenario(f):
                try:
                    f()
                except Exception as err:
                    raised.append(err)
            threads = [threading.Thread(target=scenario, args=(f,), daemon=True)
                       for f in (smoke.scenario_long_command, smoke.scenario_background_job)]
            for t in threads:
                t.start()
            for t in threads:
                while t.is_alive():
                    t.join(1)
            if raised:
                raise raised[0]
            smoke.note_native()
            log("phase 2: messages with a wait, both ways")
            smoke.scenario_messages()
            log("phase 3: Stop during a wait")
            smoke.scenario_stop()
            smoke.note_native()
            smoke.scenario_answers()
            if args.keep_report:
                smoke.views = {name: smoke.ipc.view(conv) for name, conv in smoke.convs.items()}
    except Watchdog:
        broke = f"the run passed its {WATCHDOG_S} s limit"
    except Exception as err:
        import traceback
        traceback.print_exc()
        broke = f"{type(err).__name__}: {err}"
    finally:
        signal.alarm(120)
        try:
            notes = smoke.cleanup()
        except Watchdog:
            notes = ["cleanup passed its 120 s limit"]
        signal.alarm(0)
    took = time.time() - started
    failed = [r for r in RESULTS if not r["ok"]]
    print()
    print(f"live smoke: {len(RESULTS) - len(failed)} passed, {len(failed)} failed, {took:.0f} s")
    for r in failed:
        print(f"  FAIL [{r['scenario']}] {r['check']}: {r['detail']}")
    if broke:
        print(f"  BROKE: {broke}")
    if skipped:
        print(f"  {skipped}")
    if args.keep_report:
        json.dump({"results": RESULTS, "broke": broke, "skipped": skipped, "cleanup": notes, "seconds": round(took),
                   "views": getattr(smoke, "views", None)},
                  open(args.keep_report, "w"), indent=1)
    if broke:
        sys.exit(2)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
