"""A minimal JSON-RPC client for the installed `codex app-server`, shared by the probes.

`C(cwd, conf)` starts the app-server in `cwd` (with `-c` values `conf`), logs every event to
<cwd>/events.jsonl and stderr to <cwd>/stderr.log; `C.call(method, params)` sends a request and
waits for its answer. The binary is $CODEX_BIN, else `codex` on PATH.
"""
import json, queue, shutil, subprocess, threading, time, sys, os
class C:
    def __init__(self, cwd, conf=()):
        cmd = [os.environ.get("CODEX_BIN") or shutil.which("codex") or "codex", "app-server"]
        for c in conf: cmd += ["-c", c]
        self.p = subprocess.Popen(cmd, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(cwd + "/stderr.log", "a"), text=True)
        self.q, self.n, self.ev = queue.Queue(), 0, []
        self.log = open(cwd + "/events.jsonl", "a")
        threading.Thread(target=self.read, daemon=True).start()
        self.call("initialize", {"clientInfo": {"name": "dlg-lsm-w12-probe", "version": "1"}, "capabilities": {"experimentalApi": True}})
        self.write({"method": "initialized"})
    def read(self):
        for l in self.p.stdout:
            try: v = json.loads(l)
            except ValueError: continue
            self.log.write(l); self.log.flush(); self.q.put(v)
    def write(self, v): self.p.stdin.write(json.dumps(v) + "\n"); self.p.stdin.flush()
    def next(self, t=300):
        v = self.q.get(timeout=t); self.ev.append(v)
        if "method" in v and "id" in v:
            self.write({"id": v["id"], "error": {"code": -32601, "message": "no"}})
        return v
    def send(self, method, params):
        self.n += 1; self.write({"id": self.n, "method": method, "params": params}); return self.n
    def call(self, method, params):
        i = self.send(method, params)
        while True:
            v = self.next()
            if v.get("id") == i and "method" not in v:
                if "error" in v: raise RuntimeError(f"{method}: {v['error']}")
                return v["result"]
            self.show(v)
    def show(self, v):
        m = v.get("method", "")
        pr = v.get("params") or {}
        it = pr.get("item") or {}
        if m in ("item/started", "item/completed") and it.get("type") in ("contextCompaction", "agentMessage", "commandExecution"):
            extra = (it.get("text") or "")[:400] if it.get("type") == "agentMessage" else ""
            print(f"  {m} {it.get('type')} id={it.get('id')} {extra}", flush=True)
        elif m in ("turn/started", "turn/completed", "thread/compacted", "error"):
            print(f"  {m} {json.dumps(pr)[:300]}", flush=True)
        elif m == "thread/tokenUsage/updated":
            tu = pr.get("tokenUsage") or {}
            print(f"  tokens last={(tu.get('last') or {}).get('totalTokens')} total={(tu.get('total') or {}).get('totalTokens')} window={tu.get('modelContextWindow')}", flush=True)
    def wait_turn(self, tid=None):
        while True:
            v = self.next(); self.show(v)
            if v.get("method") == "turn/completed" and (tid is None or v["params"]["turn"]["id"] == tid): return v
    def turn(self, thread, text, **kw):
        r = self.call("turn/start", {"threadId": thread, "input": [{"type": "text", "text": text}], **kw})
        return r["turn"]["id"]
