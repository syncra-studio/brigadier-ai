"""Plain CLI transport for the existing battery's scripts and scorers.

No seam, ledger instructions, compaction command or compaction threshold is installed.
Each request resumes the same native session; each CLI compacts at its own default.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import time

import battery
from lineages import compactions


class PlainBattery(battery.Battery):
    def setup(self):
        self.fixture()
        self.native_id = None
        self.raw_events = []
        self.native_sessions = {}

    def environment(self):
        env = dict(os.environ)
        for key in ("DISABLE_AUTO_COMPACT", "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"):
            env.pop(key, None)
        return env

    def record(self):
        # Plain CLI has no Brigadier record. If explicitly asked for a ledger by the recall
        # script, it may keep one in its cwd; the decision block never asks for one.
        return self.repo

    def native_path(self):
        if not getattr(self, "native_id", None):
            return None
        known = getattr(self, "native_sessions", {}).get(self.native_id)
        if known:
            return Path(known)
        home = Path(os.environ.get("CLAUDE_CONFIG_DIR", Path.home() / ".claude") if self.args.provider == "claude"
                    else os.environ.get("CODEX_HOME", Path.home() / ".codex"))
        root = home / ("projects" if self.args.provider == "claude" else "sessions")
        return next(root.rglob(f"*{self.native_id}.jsonl"), None)

    def generations(self):
        path = self.native_path()
        return compactions(path, self.args.provider) if path else 0

    def fault(self, kind):
        def record(step):
            self.seams.append({"kind": kind, "request": step.get("request"), "skipped":
                               "native CLI transport does not inject Brigadier daemon faults"})
        return record

    def send(self, name, text, fault=None, *, tool_less=False):
        if self.args.provider == "codex" and getattr(self.args, "shared_transition", False):
            from codex_transport import send
            return send(self, name, text, fault)
        self.tick()
        battery.log(name)
        step = {"name": name, "sent_ms": battery.now()}
        self.steps.append(step)
        if self.args.provider == "claude":
            command = ["claude", "-p", "--model", self.args.model, "--effort", "high",
                       "--dangerously-skip-permissions", "--output-format", "stream-json", "--verbose"]
            if getattr(self.args, "shared_transition", False) and self.args.arm == "plain@200k":
                command += ["--autocompact", str(self.args.tokens)]
            if self.native_id:
                command += ["--resume", self.native_id]
            if tool_less:
                command += ["--tools", "", "--strict-mcp-config"]
        else:
            command = ["codex", "exec"] + (["resume", self.native_id] if self.native_id else [])
            command += ["--model", self.args.model, "-c", 'model_reasoning_effort="high"',
                        "--dangerously-bypass-approvals-and-sandbox", "--json", "-"]
        proc = subprocess.Popen(command, cwd=self.repo, env=self.environment(), stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.processes.append(proc)
        if fault:
            fault(step)
        try:
            stdout, stderr = proc.communicate(text, timeout=min(self.args.turn_timeout, max(1, self.deadline - time.monotonic())))
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            stdout, stderr = proc.communicate()
            self.errors.append(name + ": native CLI timeout")
        answer = []
        for line in stdout.splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            self.raw_events.append(event)
            self.native_id = event.get("session_id") or event.get("thread_id") or self.native_id
            if event.get("type") == "assistant":
                answer += [part["text"] for part in event.get("message", {}).get("content", []) if part.get("type") == "text"]
            if event.get("type") == "item.completed" and event.get("item", {}).get("type") == "agent_message":
                answer.append(event["item"]["text"])
            if event.get("type") == "result":
                if event.get("result"):
                    answer = [event["result"]]
                u = event.get("usage", {})
                self.result.setdefault("usage", []).append({"at_ms": battery.now(), "kind": "thread",
                    "input": u.get("input_tokens"), "cached_input": u.get("cache_read_input_tokens"),
                    "cache_write": u.get("cache_creation_input_tokens"), "output": u.get("output_tokens"),
                    "cost_usd": event.get("total_cost_usd")})
            if event.get("type") == "turn.completed":
                u = event.get("usage", {})
                self.result.setdefault("usage", []).append({"at_ms": battery.now(), "kind": "thread",
                    "input": max(0, u.get("input_tokens", 0) - u.get("cached_input_tokens", 0)),
                    "cached_input": u.get("cached_input_tokens"), "output": u.get("output_tokens"), "cost_usd": None})
        step.update(ended_ms=battery.now(), settled=proc.returncode == 0, answer="\n".join(answer))
        if self.native_id:
            path = self.native_path()
            if not hasattr(self, "native_sessions"):
                self.native_sessions = {}
            self.native_sessions[self.native_id] = str(path) if path else None
        if proc.returncode:
            self.errors.append(f"{name}: CLI exit {proc.returncode}: {stderr[-300:]}")
        if not self.native_id:
            raise RuntimeError("native CLI did not return a session id")
        return step

    def wait(self, fn, seconds=90):
        # Nothing asynchronous runs between plain CLI turns.
        return fn()

    def capture(self):
        path = self.native_path()
        sessions = getattr(self, "native_sessions", {}) or {getattr(self, "native_id", None): str(path) if path else None}
        self.native_files = [file for file in sessions.values() if file]
        self.native_sources = [(self.args.provider, file) for file in self.native_files]
        # Include every old session: summary requests belong to the session they compacted.
        entries = [{"entry": {"provider": self.args.provider,
                    "event": {"type": "sessionStarted", "file": file or str(self.root / f"missing-{sid}.jsonl")}}}
                   for sid, file in sessions.items() if sid]
        if entries:
            self.result["roles"] = battery.capture_roles(entries)
        self.result.update(compactions=sum(compactions(file, self.args.provider) for file in self.native_files),
                           native_session_ids=list(sessions), native_session_id=getattr(self, "native_id", None),
                           plain_events=getattr(self, "raw_events", []), ledger=battery.read_text(self.repo / "ledger.md"))

    def cleanup_native(self):
        client = getattr(self, "codex_client", None)
        if client:
            for native_id in self.native_sessions:
                client.deadline = min(self.deadline, time.monotonic() + 10)
                try:
                    client.call("thread/delete", {"threadId": native_id})
                except (OSError, RuntimeError, TimeoutError) as error:
                    self.cleanup_notes.append("Codex thread cleanup: " + str(error))

    def cleanup(self):
        # Reuse the battery's own process/evidence/scratch cleanup, with no daemon IPC.
        super().cleanup()
