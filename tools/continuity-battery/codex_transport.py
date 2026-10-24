"""Native app-server transport for opt-in plain comparison arms (no SDK)."""
import json
import queue
import subprocess
import threading
import time

import battery


class Client:
    def __init__(self, run):
        command = ["codex", "app-server", "-c", 'model_reasoning_effort="high"']
        if run.args.arm == "plain@200k":
            command += ["-c", f"model_auto_compact_token_limit={run.args.tokens}"]
        self.proc = subprocess.Popen(command, cwd=run.repo, env=run.environment(), stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                                     start_new_session=True)
        run.processes.append(self.proc)
        self.incoming, self.counter, self.events = queue.Queue(), 0, []
        self.deadline = min(run.deadline, time.monotonic() + getattr(run.args, "turn_timeout", 60))
        threading.Thread(target=self.read, daemon=True).start()
        self.call("initialize", {"clientInfo": {"name": "continuity-battery", "version": "1"}})
        self.write({"method": "initialized"})

    def read(self):
        for line in self.proc.stdout:
            try:
                self.incoming.put(json.loads(line))
            except ValueError:
                continue
        self.incoming.put(None)

    def write(self, value):
        self.proc.stdin.write(json.dumps(value) + "\n")
        self.proc.stdin.flush()

    def next(self):
        try:
            value = self.incoming.get(timeout=max(.01, self.deadline - time.monotonic()))
        except queue.Empty as error:
            raise TimeoutError("Codex app-server timeout") from error
        if value is None:
            raise RuntimeError("Codex app-server closed")
        self.events.append(value)
        if "method" in value and "id" in value:
            # No implicit approval or fabricated tool result: this measurement requests
            # approvalPolicy=never and dangerFullAccess, like the existing plain arm.
            self.write({"id": value["id"], "error": {"code": -32601, "message": "unsupported harness request"}})
        return value

    def call(self, method, params):
        self.counter += 1
        request = self.counter
        self.write({"id": request, "method": method, "params": params})
        while True:
            value = self.next()
            if value.get("id") == request:
                if "error" in value:
                    raise RuntimeError(f"{method}: {value['error']}")
                return value["result"]


def send(run, name, text, fault=None):
    run.tick()
    battery.log(name)
    step = {"name": name, "sent_ms": battery.now()}
    run.steps.append(step)
    if not getattr(run, "codex_client", None):
        run.codex_client = Client(run)
    if not run.native_id:
        started = run.codex_client.call("thread/start", {"model": run.args.model, "cwd": str(run.repo),
                                          "approvalPolicy": "never", "sandbox": "danger-full-access"})
        run.native_id = started["thread"]["id"]
        run.native_sessions[run.native_id] = started["thread"].get("path")
    client = run.codex_client
    client.deadline = min(run.deadline, time.monotonic() + run.args.turn_timeout)
    start = len(client.events)
    turn = client.call("turn/start", {"threadId": run.native_id, "input": [{"type": "text", "text": text}],
                                     "effort": "high", "approvalPolicy": "never",
                                     "sandboxPolicy": {"type": "dangerFullAccess"}})["turn"]
    if fault:
        fault(step)
    # Notifications can precede the turn/start response. Inspect those too.
    index = start
    while True:
        if index == len(client.events):
            client.next()
        event = client.events[index]
        index += 1
        params = event.get("params") or {}
        if event.get("method") == "turn/completed" and (params.get("turn") or {}).get("id") == turn["id"]:
            step["settled"] = params["turn"]["status"] == "completed"
            break
    events = client.events[start:]
    answer = [event["params"]["item"]["text"] for event in events if event.get("method") == "item/completed"
              and (event.get("params", {}).get("item") or {}).get("type") == "agentMessage"]
    run.raw_events.extend(events)
    step.update(ended_ms=battery.now(), answer="\n".join(answer))
    if not step["settled"]:
        run.errors.append(name + ": Codex turn failed")
    path = run.native_sessions.get(run.native_id) or run.native_path()
    run.native_sessions[run.native_id] = str(path) if path else None
    return step
