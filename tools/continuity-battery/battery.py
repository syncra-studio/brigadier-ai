#!/usr/bin/env python3
"""S21/S22: measure the seam; never implement continuity or gate on recall."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import sqlite3
import subprocess
import sys
import time

sys.dont_write_bytecode = True
SRC = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("live_smoke", SRC / "tools/live-smoke/smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
ITEMS = {
    "rule": ("standing rule applied", [r"Since 0\.5:"]),
    "exception": ("correction applied in exporters", [r"Exporter since 0\.5:"]),
    "decision": ("decision and reason", [r"\b(6|six)\b", r"accounting"]),
    "hash": ("release commit", [r"4e1d9b7c2a03"]),
    "path": ("file and line", [r"src/invoice\.ts:57\b"]),
    "units": ("number with units", [r"4500\s*ms|4,500\s*ms|4\.5\s*(seconds|s)\b"]),
    "failed": ("failed check error", [r"row 17", r"tax_region", r"B-2291"]),
    "ruled_out": ("ruled-out approach and reason", [r"module.level", r"Map", r"test"]),
    "tool_only": ("only in one-time tool output", []),
    "false_worker": ("false worker claim rejected", [r"false|unverified|not verified", r"writeFileSync"]),
    "absent_ledger": ("deliberately absent from ledger", []),
    "large_failure": ("middle of large failing output", []),
}


def now():
    return int(time.time() * 1000)


def log(text):
    print(time.strftime("%H:%M:%S"), text, flush=True)


def parse_answer(text):
    """No whole-answer matching: a value must occur under its own item key."""
    try:
        start = text.index("{")
        obj, _ = json.JSONDecoder().raw_decode(text[start:])
        return obj if isinstance(obj, dict) else {}
    except (ValueError, TypeError):
        return {}


def grade(answer, expected, action):
    parsed = parse_answer(answer)
    rows = {}
    for key, (description, patterns) in ITEMS.items():
        value = str(parsed.get(key, "unknown"))
        pats = patterns or [re.escape(expected[key])]
        passed = all(re.search(p, value, re.I) for p in pats)
        if key in ("rule", "exception"):
            passed = passed and action.get(key, False)
        rows[key] = {"description": description, "verdict": "pass" if passed else
                     "unknown" if "unknown" in value.lower() else "fail", "answer": value,
                     "action": action.get(key)}
    return rows


class Battery:
    def __init__(self, args):
        self.args = args
        self.root = Path("/tmp").resolve() / (args.scratch_prefix + "-" + secrets.token_hex(3))
        self.root.mkdir()
        self.data = self.root / "data"
        self.repo = self.root / "repo"
        self.ipc = smoke.Ipc(str(self.data))
        self.proc = None
        self.conversation = None
        self.project = None
        self.processes = []
        self.steps = []
        self.seams = []
        self.errors = []
        self.cleanup_notes = []
        self.native_files = []
        self.expected = {k: prefix + secrets.token_hex(5).upper() for k, prefix in
                         (("tool_only", "VT-"), ("absent_ledger", "ORCHID-"),
                          ("large_failure", "MIDFAIL-"))}
        self.deadline = time.monotonic() + args.timeout
        self.result = {"started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                       "source": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=SRC, text=True).strip(),
                       "daemon_sha256": hashlib.sha256(Path(args.daemon).read_bytes()).hexdigest(),
                       "model": args.model, "threshold": args.tokens, "arm": "seam",
                       "steps": self.steps,
                       "seams": self.seams, "errors": self.errors, "expected": self.expected}

    def tick(self):
        if time.monotonic() >= self.deadline:
            raise TimeoutError("whole-run watchdog expired")

    def wait(self, fn, seconds=90):
        end = min(self.deadline, time.monotonic() + seconds)
        while time.monotonic() < end:
            value = fn()
            if value:
                return value
            time.sleep(0.5)
        self.tick()
        return None

    def start(self):
        self.data.mkdir(exist_ok=True)
        env = {k: os.environ[k] for k in ("HOME", "USER", "LOGNAME", "PATH") if k in os.environ}
        env.update(SHELL="/bin/zsh", TMPDIR="/tmp", LANG="en_US.UTF-8",
                   BRIGADIER_DATA_DIR=str(self.data), BRIGADIER_SEAM_TOKENS=str(self.args.tokens))
        with (self.root / "daemon.log").open("a") as out:
            self.proc = subprocess.Popen([self.args.daemon, "--foreground", "--data-dir", str(self.data)],
                                         env=env, stdout=out, stderr=subprocess.STDOUT,
                                         stdin=subprocess.DEVNULL, start_new_session=True)
        self.processes.append(self.proc)
        def ready():
            if self.proc.poll() is not None:
                raise RuntimeError("daemon exited; " + (self.root / "daemon.log").read_text()[-1500:])
            try:
                return self.ipc.call({"method": "getCatalog"}, timeout=3)
            except (OSError, RuntimeError, ConnectionError):
                return None
        if not self.wait(ready, 60):
            raise RuntimeError("daemon startup timeout")

    def setup(self):
        self.start()
        settings = self.ipc.call({"method": "getCatalog"})["catalog"]["settings"]
        settings.update(onboarded=True, introSeen=True, defaultPermission="full")
        settings["hiddenModels"] = [{"provider": "claude", "id": "fable"}]
        self.ipc.call({"method": "updateSettings", "settings": settings})
        def signed_in():
            view = self.ipc.call({"method": "getProviders"})["view"]
            return next((p for p in view["providers"] if p["provider"] == "claude" and
                         (p.get("status") or {}).get("loggedIn")), None)
        if not self.wait(signed_in, 60):
            raise RuntimeError("Claude not installed or signed in; no credentials entered")
        self.repo.mkdir()
        (self.repo / "src").mkdir()
        (self.repo / "src/exporters").mkdir()
        (self.repo / "src/storage.ts").write_text(
            "import {writeFileSync} from 'node:fs';\nexport function writeDataFile(p: string, s: string) { writeFileSync(p, s); }\n")
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], check=True)
        subprocess.run(["git", "-C", str(self.repo), "add", "src/storage.ts"], check=True)
        subprocess.run(["git", "-C", str(self.repo), "-c", "user.name=continuity-battery",
                        "-c", "user.email=continuity@localhost", "commit", "-qm", "fixture"], check=True)
        self.project = self.ipc.call({"method": "addProject", "path": str(self.repo), "name": "continuity", "init": False})["project"]["id"]
        self.ipc.call({"method": "setFolderTrust", "id": self.project, "path": None, "trusted": True})
        setup = {"type": "session", "repo": str(self.repo),
                 "environment": {"type": "localCheckout", "branch": "main", "createFrom": None},
                 "permission": "full", "orchestrator": {"provider": "claude", "model": self.args.model, "effort": "high"},
                 "planMode": False}
        self.conversation = self.ipc.call({"method": "createConversation", "kind": "session",
                                           "projectId": self.project, "title": "Continuity baseline", "setup": setup})["conversation"]["id"]

    def view(self):
        return self.ipc.view(self.conversation)

    def entries(self):
        entries, before = [], None
        while True:
            page = self.ipc.call({"method": "listOrchestratorLog", "conversationId": self.conversation,
                                  "before": before, "limit": 500})["page"]
            entries.extend(page["entries"])
            if not page["hasMore"] or not page["entries"]:
                return sorted(entries, key=lambda e: e["streamSeq"])
            before = min(e["streamSeq"] for e in page["entries"])

    def record(self):
        """The session's `record/` folder (data in /tmp: its sessions folder is `<data>-s`)."""
        return self.root / "data-s" / self.conversation[-8:] / "record"

    def seam_file(self):
        try:
            return json.loads((self.record() / "seam.json").read_text())
        except (OSError, ValueError):
            return {}

    def generations(self):
        """Fresh contexts so far: closed seams."""
        file = self.seam_file()
        return len(file.get("seams", [])) + (file.get("phase", {}).get("phase") in ("closed", "starting"))

    def send(self, name, text, fault=None):
        self.tick()
        log(name)
        step = {"name": name, "sent_ms": now()}
        self.steps.append(step)
        outcome = self.ipc.call({"method": "sendMessage", "conversationId": self.conversation,
                                 "text": text, "attachments": [], "mentions": [], "steer": False})["outcome"]
        msg = outcome.get("message") or {}
        step["request"] = msg.get("requestId") or msg.get("id")
        if fault:
            fault(step)
        def ended():
            v = self.view()
            req = next((r for r in v["requests"] if r["id"] == step["request"]), None)
            return v if req and req["state"]["type"] not in ("working", "queued", "deciding") and v["run"] in ("idle", "hibernated", "failed") else None
        v = self.wait(ended, self.args.turn_timeout)
        step["ended_ms"] = now()
        step["settled"] = bool(v)
        if v:
            step["answer"] = "\n".join(m["text"] for m in smoke.messages(v)
                                       if m["role"] == "assistant" and m["createdAtMs"] >= step["sent_ms"])
        else:
            step["answer"] = ""
            step["timeout_view"] = self.view().get("run")
            self.errors.append(name + ": turn did not settle within bound")
            self.ipc.call({"method": "interrupt", "conversationId": self.conversation})
            self.wait(lambda: self.view()["run"] in ("idle", "hibernated", "failed"), 15)
        return step

    def fault(self, kind):
        def inject(step):
            seam = {"kind": kind, "at_ms": now(), "request": step["request"]}
            self.seams.append(seam)
            if kind in ("context", "limit"):
                body = {"method": "debugInjectLimit" if kind == "limit" else "debugInjectError",
                        "target": {"type": "conversation", "conversationId": self.conversation},
                        "provider": "claude", "afterToolCalls": 1}
                body.update({"window": "five_hour", "resetInMinutes": 1} if kind == "limit" else {"error": "contextWindow"})
                def arm():
                    try:
                        self.ipc.call(body)
                        return True
                    except RuntimeError as error:
                        seam["last_error"] = str(error)
                        return False
                seam["armed"] = bool(self.wait(arm, 20))
            else:
                marker = self.repo / (kind + ".waiting")
                seam["wait_point_seen"] = bool(self.wait(marker.exists, 50))
                if not seam["wait_point_seen"]:
                    return
                if kind == "restart":
                    # This Popen belongs to this run; restart at the command's chosen wait.
                    self.proc.kill()
                    self.proc.wait(10)
                    self.start()
                    seam["restarted"] = True
                else:
                    status = self.ipc.call({"method": "getConversationStatus", "conversationId": self.conversation})["status"]
                    native = status.get("nativeId")
                    table = subprocess.check_output(["ps", "-Ao", "pid=,ppid=,command="], text=True)
                    rows = [line.strip().split(None, 2) for line in table.splitlines()]
                    own = {self.proc.pid}
                    for _ in range(20):
                        own.update(int(p) for p, parent, *_ in rows if int(parent) in own)
                    pids = [int(p) for p, _, command in rows if int(p) in own and native and native in command
                            and ("claude" in command) and "--resume" not in command]
                    if not pids:
                        pids = [int(p) for p, _, command in rows if int(p) in own and native and native in command and "claude" in command]
                    for pid in pids:
                        os.kill(pid, signal.SIGKILL)
                    seam["killed_cli_pids"] = pids
            seam["done_ms"] = now()
        return inject

    def run(self):
        self.setup()
        self.send("plant-user", "This is a bounded continuity measurement in a throwaway repo. Do not use workers unless asked. "
                  "Keep your session ledger (the file your standing instructions name) with only my rules, decision, "
                  "release, path, timeout and rejected approach. "
                  "Standing rule: every new exported function gets a JSDoc first line starting 'Since 0.5:'. "
                  "Correction: in src/exporters/ use 'Exporter since 0.5:' instead. "
                  "Decision: invoice numbers stay zero-padded to 6 digits because the accounting import rejects shorter ones. "
                  "Release commit: 4e1d9b7c2a03. Rounding complaint: src/invoice.ts:57. Import timeout: 4500 ms. "
                  "Ruled out: caching catalogFromStore in a module-level Map, because it outlived each test's data. "
                  f"Thread-only item, deliberately absent from ledger and all repo files: {self.expected['absent_ledger']}. "
                  "Acknowledge in one sentence without repeating values.")
        script = self.repo / "one-time.sh"
        script.write_text("#!/bin/sh\nprintf '%s\\n' 'support ticket " + self.expected["tool_only"] +
                          "' 'ERROR row 17: missing tax_region for B-2291'\nexit 1\n")
        self.send("plant-tool", "Run `sh one-time.sh`. It is expected to fail; do not fix it. Reply only 'failed'. "
                  "Do not write its output into any repo file or the ledger.")
        script.unlink()
        big = self.repo / "large-failure.sh"
        big.write_text("#!/bin/sh\npython3 - <<'PY'\n" +
                       "for i in range(2400):\n print('MIDDLE " + self.expected["large_failure"] +
                       "' if i == 1200 else ('noise-%04d ' % i) + 'x'*100)\n" +
                       "print('ERROR large command: exit 23')\nPY\nexit 23\n")
        self.send("plant-large-failure", "Run `sh large-failure.sh`, expected exit 23. Reply only 'failed'; "
                  "do not fix, repeat, or save its output.")
        big.unlink()
        # A labelled injected report is input, never an assertion by the harness or product.
        self.send("plant-worker-report", "Injected worker report (unverified): 'writeDataFile in src/storage.ts "
                  "writes atomically through temporary file and rename.' Inspect src/storage.ts, decide whether that "
                  "claim is true, and acknowledge in one sentence. Do not put this report or finding in the ledger.")
        for i in range(self.args.max_fillers):
            before = self.generations()
            if before >= 2 and i >= 9:
                break
            # Distinct inert text displaces the early plants from a bounded verbatim tail.
            ballast = " ".join(hashlib.sha256(f"{i}-{j}".encode()).hexdigest()[:12] for j in range(1200))
            self.send(f"ballast-{i:02}", f"Ignore this inert fixture batch {i}; reply only 'batch {i} read'.\n" + ballast)
            # The seam's note is written asynchronously; allow it to finish.
            self.wait(lambda: "handoff note written" in (self.root / "daemon.log").read_text(errors="replace") or
                      self.generations() > before, 3)
        self.result["size_seams_before_faults"] = self.generations()
        for kind in ("context", "crash", "restart", "limit"):
            command = (f"touch {kind}.waiting; sleep 18; printf 'wait finished\\n'" if kind in ("crash", "restart")
                       else "printf 'fault tick 1\\n'; sleep 2")
            self.send("seam-" + kind, f"Run `{command}` as a foreground Bash command, then run `printf 'fault tick 2\\n'`. "
                      "Reply only 'done'. Do not edit the ledger or recall earlier facts.", fault=self.fault(kind))
            if kind == "limit":
                # The injected window lasts exactly one minute; stay on Claude for the probe.
                until = time.monotonic() + 65
                while time.monotonic() < until:
                    self.tick()
                    time.sleep(0.5)
        self.send("apply", "Add two tiny exported identity functions: ordinary in src/ordinary.ts and exporter in "
                  "src/exporters/sample.ts. Apply my earlier documentation rule and correction. Don't edit the ledger. "
                  "Do not commit or push; reply only 'done'.")
        def documented(filename, prefix):
            file = self.repo / filename
            return file.exists() and bool(re.search(r"/\*\*\s*\n?\s*\*?\s*" + re.escape(prefix) +
                                                    r"[^*]*(?:\*(?!/)[^*]*)*\*/\s*export\s+(?:async\s+)?function\b", file.read_text()))
        action = {"rule": documented("src/ordinary.ts", "Since 0.5:"),
                  "exception": documented("src/exporters/sample.ts", "Exporter since 0.5:")}
        keys = ", ".join(ITEMS)
        probe = self.send("probe", "Use only this conversation and its existing transcript/Brain lookup tools to answer; "
                          "do not read fixture scripts, run commands, or edit files. Return one JSON object with keys " + keys +
                          ". Values respectively: standing JSDoc rule; its correction; invoice decision and reason; "
                          "release hash; complaint file:line; timeout with units; exact small failed-check error; "
                          "ruled-out approach and reason; one-time support ticket; worker report validity with evidence; "
                          "the item deliberately omitted from ledger; middle marker from large failing output. "
                          "Say 'unknown' for missing evidence. Do not guess.")
        self.result["items"] = grade(probe["answer"], self.expected, action)

    def capture(self):
        if not self.conversation or not self.proc or self.proc.poll() is not None:
            return
        entries = self.entries()
        self.result["provider_events"] = entries
        self.native_files = sorted({event["file"] for e in entries
                                    if (event := e["entry"].get("event") or {}).get("type") == "sessionStarted"
                                    and event.get("file") and e["entry"].get("provider") == "claude"})
        self.result["native_calls"] = native_calls(entries)
        self.result["seam_observations"] = seam_observations(self.result)
        # The seam's own record: each note and what each successor started from.
        file = self.seam_file()
        self.result["seam_file"] = file
        self.result["handoffs"] = [
            {"n": seam["n"], "death": seam.get("death"), "successor": seam.get("successor"),
             "nudged_at_ms": seam.get("nudgedAtMs"), "closed_at_ms": seam.get("closedAtMs"),
             "started_at_ms": seam.get("startedAtMs"),
             "note": read_text(self.record() / "handoff" / f"{seam['n']}.md"),
             "start": read_text(self.record() / "handoff" / f"{seam['n']}-start.md")}
            for seam in file.get("seams", [])]
        # The session's ledger, as the standing instructions name it.
        ledger = self.record() / "ledger.md"
        content = ledger.read_text() if ledger.exists() else ""
        self.result["ledger_in_repo"] = (self.repo / "ledger.md").exists()
        self.result["ledger_bytes"] = len(content.encode())
        self.result["absent_from_ledger"] = self.expected["absent_ledger"] not in content
        with sqlite3.connect(f"file:{self.data}/routing.sqlite?mode=ro", uri=True) as db:
            db.row_factory = sqlite3.Row
            self.result["usage"] = [dict(r) for r in db.execute(
                "SELECT * FROM turn_usage WHERE conversation_id=? ORDER BY at_ms", (self.conversation,))]

    def cleanup(self):
        try:
            if self.proc and self.proc.poll() is None:
                if self.conversation:
                    self.ipc.call({"method": "interrupt", "conversationId": self.conversation})
                    self.ipc.call({"method": "delete", "ids": [self.conversation]})
                    self.wait(lambda: self.conversation not in {c["id"] for c in self.ipc.call({"method": "getCatalog"})["catalog"]["conversations"]}, 30)
                if self.project:
                    report = self.ipc.call({"method": "removeProject", "id": self.project,
                                            "deleteBranches": [], "keepBrain": False}, timeout=40)["report"]
                    self.cleanup_notes.extend(report.get("failures", []))
                self.ipc.call({"method": "shutdown"})
                self.proc.wait(15)
        except Exception as error:
            self.cleanup_notes.append(str(error))
        finally:
            for proc in self.processes:
                if proc.poll() is None:
                    proc.terminate()
                    try:
                        proc.wait(5)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                        proc.wait(5)
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        trusted = smoke.trust_left(str(self.root))
        for filename in self.native_files:
            file = Path(filename)
            # Only exact files the run's own provider events identified, after IPC Delete.
            if file.exists():
                file.unlink()
                sidecar = file.with_suffix("")
                if sidecar.is_dir():
                    shutil.rmtree(sidecar)
                self.cleanup_notes.append("IPC Delete left a native session file; removed " + filename)
        if trusted:
            self.cleanup_notes.append("Kept scratch records to undo remaining folder trust: " + str(self.root))
        self.result["cleanup"] = self.cleanup_notes
        # Evidence is intentionally private; only the concise text report is committed.
        if self.args.evidence:
            Path(self.args.evidence).write_text(json.dumps(self.result, indent=2))
        if not trusted:
            shutil.rmtree(self.root)


def read_text(path):
    try:
        return path.read_text()
    except OSError:
        return ""


LOOKUP = re.compile(r"search_transcript|query_brain|read_report")
LOOKUP_COMMAND = re.compile(r"brigadier\s+(brain|transcript|ledger)\b")


def is_lookup(event):
    """A completed lookup of the record: an old tool, or a `brigadier brain|transcript|ledger` command."""
    if event.get("status") != "completed":
        return False
    if event.get("type") == "command":
        return bool(LOOKUP_COMMAND.search(event.get("command") or ""))
    return event.get("type") == "toolCall" and bool(
        LOOKUP.search(event.get("name", "")) or LOOKUP_COMMAND.search(event.get("input") or ""))


def native_calls(entries):
    """Read only this run's recorded native files; deduplicate streaming message fragments."""
    sessions = {}
    for entry in entries:
        event = entry["entry"].get("event") or {}
        if event.get("type") != "sessionStarted" or not event.get("file"):
            continue
        calls = {}
        try:
            for line in Path(event["file"]).read_text().splitlines():
                row = json.loads(line)
                message = row.get("message") or {}
                if row.get("type") == "assistant" and message.get("id") and message.get("usage"):
                    previous = calls.get(message["id"], {})
                    calls[message["id"]] = {"usage": message["usage"], "timestamp": row.get("timestamp"),
                                            "tools": sorted(set(previous.get("tools", []) + [
                                                c.get("name") for c in message.get("content", [])
                                                if c.get("type") == "tool_use" and c.get("name")]))}
            sessions[event["nativeId"]] = list(calls.values())
        except (OSError, ValueError):
            sessions[event["nativeId"]] = []
    return sessions


def seam_observations(result):
    out = []
    for i, seam in enumerate(result["seams"]):
        end = result["seams"][i + 1]["at_ms"] if i + 1 < len(result["seams"]) else float("inf")
        events = [(e["atMs"], e["entry"].get("event") or {}) for e in result.get("provider_events", [])
                  if seam["at_ms"] <= e["atMs"] < end]
        errors = [event["error"]["kind"] for _, event in events if event.get("type") == "error"]
        out.append({"kind": seam["kind"], "errors": errors,
                    "sessions_started": sum(event.get("type") == "sessionStarted" for _, event in events),
                    "completed_turns": sum(event.get("type") == "turnCompleted" and
                                           event.get("status") == "completed" for _, event in events)})
    return out


def report(result):
    rows = result.get("items", {})
    usage = result.get("usage", [])
    probe = next((s for s in result["steps"] if s["name"] == "probe"), None)
    events = result.get("provider_events", [])
    lookup = {e["entry"]["event"]["itemId"]: e["entry"]["event"] for e in events if probe and
              e["atMs"] >= probe["sent_ms"] and is_lookup(e["entry"].get("event") or {})}.values()
    lines = ["# Claude continuity, seam arm — " + result["started_at"][:10], "",
             "Report only; no recall result gates a session or changes continuity.", "",
             f"Source: `{result['source']}`; daemon SHA-256: `{result['daemon_sha256']}`.",
             f"Claude `{result['model']}`, high effort; headless dev daemon; one session, several requests.",
             f"Seam (`BRIGADIER_SEAM_TOKENS={result['threshold']}`); the bootstrap's tail budget is about 15k tokens.",
             "Mandatory sections and the minimum tail can exceed that budget; observed sizes follow below.",
             "A labelled, injected unverified worker report supplies the false claim; no real worker is spawned.",
             "Random one-time facts are removed from the fixture after observation. Inert batches displace the early items.", "",
             "| Item | Result | In final verbatim tail? | Successor searched? | Answer / evidence |", "|---|---|---|---|---|"]
    if rows:
        passed = sum(row["verdict"] == "pass" for row in rows.values())
        lines.insert(4, f"{passed}/{len(ITEMS)} planted items passed. The final probe was allowed to search existing records.\n")
    # The successor's verbatim part: the first message, the latest exchange and the tail.
    last = (result.get("handoffs") or [{}])[-1].get("start", "")
    recent = last.partition("## The session's first message")[2].partition("## Left out")[0]
    markers = {"rule": "Since 0.5:", "exception": "Exporter since 0.5:", "decision": "accounting import rejects",
               "hash": "4e1d9b7c2a03", "path": "src/invoice.ts:57", "units": "4500 ms",
               "failed": "B-2291", "ruled_out": "module-level Map", "false_worker": "writes atomically through"}
    markers.update(result.get("expected", {}))
    for key, (description, _) in ITEMS.items():
        row = rows.get(key, {"verdict": "not tested", "answer": "run ended before probe"})
        answer = row["answer"].replace("|", "\\|").replace("\n", " ")
        tail = ("yes" if markers[key].lower() in recent.lower() else "no") if recent else "unavailable"
        retrieved = any(markers[key].lower() in (call.get("output") or "").lower() for call in lookup)
        attempted = key == "large_failure" and any("large-failure" in (call.get("input") or "") for call in lookup)
        searched = "yes, matching output" if retrieved else "yes, attempted" if attempted else "none observed"
        lines.append(f"| {description} | {row['verdict']} | {tail} | {searched} | {answer} |")
    starts = [e["entry"] for e in result.get("provider_events", []) if
              (e["entry"].get("event") or {}).get("type") == "sessionStarted"]
    models = sorted({(entry.get("provider", "unknown"), entry["event"].get("model", "unknown")) for entry in starts})
    lines += ["", "Observed session providers/models: " + ", ".join(f"`{p}/{m}`" for p, m in models) + ".",
              "The usage-limit injection may use today's stand-in path; the requested Claude model returns after its window resets."]
    lines += ["", "| Forced seam | Observed |", "|---|---|",
              f"| Size ×2 | {result.get('size_seams_before_faults', 0)} "
              "closed seams before fault injections |"]
    observations = seam_observations(result)
    for seam, observed in zip(result["seams"], observations):
        lines.append("| " + seam["kind"] + " | `" + json.dumps({**seam, **observed}, sort_keys=True) + "` |")
    lines += ["", f"Session ledger (`record/ledger.md`): {result.get('ledger_bytes', 'unavailable')} bytes; deliberate item absent: "
              f"{result.get('absent_from_ledger', 'unavailable')}; a repo `ledger.md` written too: {result.get('ledger_in_repo', 'unavailable')}.",
              "Note and hand-off token estimates use the runtime's bytes/4 convention; first-call sizes use metering.", "",
              "| Seam | Note bytes | Hand-off tokens | Nudge→close ms | Close→successor ms | Successor first context | Death |",
              "|---|---:|---:|---:|---:|---:|---|"]
    for handoff in result.get("handoffs", []):
        calls = result.get("native_calls", {}).get(handoff.get("successor"), [])
        first = sum(calls[0]["usage"].get(k, 0) for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")) if calls else None
        nudged, closed, started = handoff.get("nudged_at_ms"), handoff.get("closed_at_ms"), handoff.get("started_at_ms")
        lines.append(f"| {handoff['n']} | {len(handoff['note'].encode())} | {len(handoff['start'].encode()) // 4} | "
                     f"{closed - nudged if nudged and closed else 'no nudge'} | {started - closed if started and closed else ''} | "
                     f"{first} | {handoff.get('death') or ''} |")
    during_probe = [e["entry"] for e in events if probe and probe["sent_ms"] <= e["atMs"] <= probe["ended_ms"] and
                    (e["entry"].get("event") or {}).get("type") == "sessionStarted"]
    prior_probe = [e["entry"] for e in events if probe and e["atMs"] < probe["sent_ms"] and
                   (e["entry"].get("event") or {}).get("type") == "sessionStarted"]
    probe_start = (during_probe[:1] or prior_probe[-1:] or [{}])[0]
    lines += ["", f"Final probe provider/model: `{probe_start.get('provider', 'unavailable')}/{(probe_start.get('event') or {}).get('model', 'unavailable')}`."]
    lines += ["", f"Probe lookup calls observed: {len(lookup)}. Item search evidence means its marker appeared in a lookup result, or an explicit query attempted it; one result can serve several items.",
                            "Seam cost below includes all metered handoff calls; ordinary thread calls are separate (successor bootstrap is included in thread use)."]
    for kind in ("handoff", "thread"):
        mine = [u for u in usage if u.get("step") == kind]
        total = {k: sum(u.get(k) or 0 for u in mine) for k in ("input", "cached_input", "cache_write", "output", "cost_usd")}
        total["cost_usd"] = round(total["cost_usd"], 6)
        missing_cost = sum(u.get("cost_usd") is None for u in mine)
        lines.append(f"- {kind}: {len(mine)} usage rows; `{json.dumps(total, sort_keys=True)}`; {missing_cost} rows without reported dollar cost")
    lines += ["", "Limits: a short debug-threshold baseline, not the ≥400k token A/B or a nightly result. "
              "Both-provider A/B, cold-return ratio, and weekly real-lineage samples belong to S25/L4. "
              "No fewer-tokens claim is established. Later seams share this session, so recall cannot be attributed to one seam alone."]
    if rows.get("large_failure", {}).get("verdict") == "unknown":
        lines += ["The middle marker of the >250 KB failing command was not recovered. The retained command output "
                  "was already excerpted; lookup attempts returned that excerpt. This measures the existing output path's limit."]
    lines += ["", "Run problems: " + ("; ".join(result["errors"]) or "none"),
              "Cleanup: " + ("; ".join(map(str, result.get("cleanup", []))) or "own sessions, project, daemon and scratch root removed"), ""]
    if any(step.get("timeout_view") == "failed" for step in result["steps"]):
        lines += ["Harness note: this first run spent a timeout waiting on the crash/restart requests after they "
                  "were already Failed. The waiter now accepts Failed as terminal. Subsequent requests completed; "
                  "these waits do not imply the session stayed stuck.", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon", required=True)
    parser.add_argument("--model", default="opus")
    parser.add_argument("--tokens", type=int, default=43750, help="the seam threshold (BRIGADIER_SEAM_TOKENS)")
    parser.add_argument("--max-fillers", type=int, default=18)
    parser.add_argument("--timeout", type=int, default=1500)
    parser.add_argument("--turn-timeout", type=int, default=100)
    parser.add_argument("--report", help="the text report (default reports/<date>-claude-seam.md)")
    parser.add_argument("--evidence", help="optional private JSON evidence, outside the repo")
    parser.add_argument("--scratch-prefix", default="continuity-battery", help="names the /tmp scratch root")
    args = parser.parse_args()
    if not args.report:
        args.report = str(Path(__file__).parent / "reports" / (time.strftime("%Y-%m-%d") + "-claude-seam.md"))
    if "fable" in args.model.lower() or args.tokens <= 0 or args.max_fillers < 9:
        parser.error("use a permitted model, positive threshold and at least nine fillers")
    if str(Path(args.daemon).resolve()).startswith("/Applications/"):
        parser.error("use a debug daemon built from this checkout")
    if args.evidence and Path(args.evidence).resolve().is_relative_to(SRC):
        parser.error("keep detailed evidence outside the repository")
    battery = Battery(args)
    try:
        battery.run()
    except Exception as error:
        log(f"run problem: {error}")
        battery.errors.append(str(error))
    finally:
        try:
            battery.capture()
        except Exception as error:
            battery.errors.append("capture: " + str(error))
        battery.cleanup()
        Path(args.report).parent.mkdir(parents=True, exist_ok=True)
        Path(args.report).write_text(report(battery.result))
        log("report: " + args.report)
    # A partial/infrastructure run is distinguishable; recall failures never affect exit.
    return 0 if "items" in battery.result else 2


if __name__ == "__main__":
    sys.exit(main())
