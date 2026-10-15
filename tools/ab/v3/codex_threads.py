#!/usr/bin/env python3
"""Deletes Codex threads a V3 harness root created (a copy of bench/codex_threads.py) (listed ids only), through a throwaway
`codex app-server` over stdio JSON-RPC (`thread/delete` removes the rollout and its state rows).
Run only after the run's evidence has been exported. A thread already gone is fine.
usage: codex_threads.py <thread-id>..."""
import json, subprocess, sys

ids = [i for i in sys.argv[1:] if i]
if not ids:
    sys.exit(0)
p = subprocess.Popen(["codex", "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
n = [0]


def call(method, params):
    n[0] += 1
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": n[0], "method": method, "params": params}) + "\n"); p.stdin.flush()
    while True:
        line = p.stdout.readline()
        if not line:
            return {"error": "app-server closed"}
        m = json.loads(line)
        if m.get("id") == n[0] and "method" not in m:
            return m


try:
    call("initialize", {"clientInfo": {"name": "brigadier-v3", "version": "0"}})
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "initialized"}) + "\n"); p.stdin.flush()
    for tid in ids:
        r = call("thread/delete", {"threadId": tid})
        print(tid, "deleted" if "result" in r else f"not deleted: {json.dumps(r.get('error'))[:200]}")
finally:
    p.stdin.close(); p.terminate(); p.wait(timeout=10)
