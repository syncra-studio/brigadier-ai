"""Probe on the installed `codex app-server`: checkpoint turn → thread/compact/start with a user
turn sent at once behind it → full-payload thread/inject_items → readback from the rollout.

Usage: python3 -I codex_order.py <scratch-dir> [model]
Prints the event order and the readback check; writes the relevant rollout lines (encrypted
compaction payloads dropped) to <scratch-dir>/fixture.jsonl. The thread is deleted at the end.
"""
import hashlib, json, sys, time
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from codex_rpc import C
d = str(Path(sys.argv[1]).resolve()); Path(d).mkdir(parents=True, exist_ok=True)
model = sys.argv[2] if len(sys.argv) > 2 else "gpt-6-luna"
c = C(d)
th = c.call("thread/start", {"model": model, "cwd": d, "approvalPolicy": "never", "sandbox": "danger-full-access"})["thread"]
tid, path = th["id"], th["path"]
KW = {"effort": "low"}
c.wait_turn(c.turn(tid, "Remember codeword LAMBDA-3141. Reply OK.", **KW))
print("CHECKPOINT TURN")
c.wait_turn(c.turn(tid, "Write the line 'goal: probe' to checkpoint.md in the cwd with the shell, then reply DONE.", **KW))
print("COMPACT + USER TURN SENT AT ONCE")
order = []
cid = c.send("thread/compact/start", {"threadId": tid})
uid = c.send("turn/start", {"threadId": tid, "input": [{"type": "text", "text": "Reply with the codeword only."}], **KW})
done_turns = 0
while done_turns < 2:
    v = c.next(); c.show(v)
    m = v.get("method"); it = (v.get("params") or {}).get("item") or {}
    if v.get("id") in (cid, uid) and "method" not in v:
        order.append(("response", "compact" if v["id"] == cid else "user-turn", "error" if "error" in v else "ok", json.dumps(v.get("error") or v.get("result"))[:160]))
        if v["id"] == uid and "error" in v: done_turns += 1
    elif m in ("turn/started", "turn/completed"):
        order.append((m, v["params"]["turn"]["id"][-6:]))
        if m == "turn/completed": done_turns += 1
    elif m in ("item/started", "item/completed") and it.get("type") in ("contextCompaction", "userMessage", "agentMessage"):
        order.append((m, it.get("type"), (it.get("text") or "")[:40]))
print("ORDER"); [print("  ", o) for o in order]
note = "[Brigadier · decisions restored after compaction]\n" + "\n".join(
    f"L{i} · 10-24 16:00 · gen 1 · decision: filler text abcdefghij klmnop qrstuv" for i in range(1, 1001)) + "\nNOTE-END"
fp = hashlib.sha256(note.encode()).hexdigest()[:16]
note += f"\n<!-- brigadier-restore {fp} -->"
print("INJECT", len(note), "chars", c.call("thread/inject_items", {"threadId": tid, "items": [
    {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": note}]}]}))
time.sleep(1)
found = []
keep = []
for line in open(path):
    e = json.loads(line); p = e.get("payload") or {}
    if p.get("type") == "message" and p.get("role") == "developer":
        texts = [x.get("text", "") for x in p.get("content", [])]
        if any(fp in t for t in texts):
            found.append(texts[0] == note)
            keep.append(e)
    if e.get("type") == "compacted":
        keep.append({"type": "compacted", "timestamp": e.get("timestamp"), "payload": {k: v for k, v in p.items() if k in ("window_number", "window_id")}})
    if e.get("type") == "event_msg" and p.get("type") == "item_completed" and "contextCompaction" in json.dumps(p):
        keep.append(e)
print("readback: injections with fingerprint", len(found), "exact text:", found)
c.wait_turn(c.turn(tid, "Without commands: quote the last line of the developer note that ends with NOTE-END (the line before NOTE-END), and the codeword.", **KW))
Path(d, "fixture.jsonl").write_text("".join(json.dumps(e) + "\n" for e in keep))
c.call("thread/delete", {"threadId": tid}); c.p.terminate()
