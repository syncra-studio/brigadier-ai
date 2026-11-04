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


def synthetic(event):
    """An assistant event the CLI wrote itself (an API error, a refused request)."""
    return bool(event.get("error")) or (event.get("message") or {}).get("model") == "<synthetic>"


def reset_time(seconds):
    try:
        return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(int(seconds)))
    except (TypeError, ValueError, OverflowError, OSError):
        return None


def rejected_limit(info):
    """A rejected rate limit, with its window and reset time; None when it was allowed."""
    if not isinstance(info, dict) or info.get("status") != "rejected":
        return None
    reset = reset_time(info.get("resetsAt"))
    return (f"rate limit rejected: {info.get('rateLimitType') or 'unknown window'}" +
            (f", resets {reset}" if reset else ""))


def cli_errors(events, stderr=""):
    """The CLI's own words about why a turn failed: an `is_error` result with its API status,
    a rejected rate limit with its reset time, a synthetic error reply, and the stderr tail."""
    found = []
    for event in events:
        kind = event.get("type")
        if kind == "result" and event.get("is_error"):
            status = event.get("api_error_status")
            found.append(f"{event.get('result') or event.get('subtype') or 'error result'}" +
                         (f" (API status {status})" if status else ""))
        elif kind == "rate_limit_event":
            limit = rejected_limit(event.get("rate_limit_info"))
            if limit:
                found.append(limit)
        elif kind == "assistant" and synthetic(event) and event.get("error"):
            text = " ".join(part.get("text", "") for part in (event.get("message") or {}).get("content", [])
                            if isinstance(part, dict) and part.get("type") == "text").strip()
            found.append(f"synthetic reply: {text or 'no text'} ({event['error']})")
            limit = rejected_limit(((event.get("api_error_params") or {}).get("rate_limit_info")))
            if limit:
                found.append(limit)
    if stderr and stderr.strip():
        found.append("stderr: " + stderr.strip()[-300:])
    return list(dict.fromkeys(found))


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
        # No fault is injected here (see fault()), so every step's errors are real.
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
        timed_out = False
        try:
            stdout, stderr = proc.communicate(text, timeout=min(self.args.turn_timeout, max(1, self.deadline - time.monotonic())))
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            stdout, stderr = proc.communicate()
            timed_out = True
            self.errors.append(name + ": native CLI timeout")
        answer, events = [], []
        for line in stdout.splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            events.append(event)
            self.raw_events.append(event)
            self.native_id = event.get("session_id") or event.get("thread_id") or self.native_id
            # The CLI's own error replies are never the model's answer.
            if event.get("type") == "assistant" and not synthetic(event):
                answer += [part["text"] for part in event.get("message", {}).get("content", []) if part.get("type") == "text"]
            if event.get("type") == "item.completed" and event.get("item", {}).get("type") == "agent_message":
                answer.append(event["item"]["text"])
            if event.get("type") == "result":
                if event.get("result") and not event.get("is_error"):
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
        errors = cli_errors(events, stderr if proc.returncode or timed_out else "")
        outcome = ("timeout" if timed_out else
                   "done" if proc.returncode == 0 and not any(e.get("type") == "result" and e.get("is_error") for e in events)
                   else "failed")
        step.update(ended_ms=battery.now(), settled=outcome == "done", outcome=outcome, answer="\n".join(answer))
        if errors:
            step["error"] = "; ".join(errors)
        if self.native_id:
            path = self.native_path()
            if not hasattr(self, "native_sessions"):
                self.native_sessions = {}
            self.native_sessions[self.native_id] = str(path) if path else None
        if proc.returncode and not timed_out:
            errors.insert(0, f"CLI exit {proc.returncode}")
        self.errors.extend(f"cli: {name}: {error}" for error in errors)
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
