"""Probe: N SessionStart(compact) hooks after a steered /compact on the installed `claude`.

Usage: python3 -I claude_slices.py <scratch-dir> [slots] [chars-per-slot] [model]
Isolated from the user's settings (--setting-sources ""). Prints whether each slice reached the
model whole, and copies the transcript's boundary + hook_additional_context lines to
<scratch-dir>/fixture.jsonl. Delete ~/.claude/projects/<scratch-dir as slug> afterwards.
"""
import json, os, subprocess, sys
from pathlib import Path
here = Path(__file__).resolve().parent
d = Path(sys.argv[1]).resolve(); d.mkdir(parents=True, exist_ok=True)
slots = int(sys.argv[2]) if len(sys.argv) > 2 else 8
size = int(sys.argv[3]) if len(sys.argv) > 3 else 9500
model = sys.argv[4] if len(sys.argv) > 4 else "haiku"
hook = lambda cmd: {"hooks": [{"type": "command", "command": cmd}]}
settings = {"autoMemoryEnabled": False, "idleCompaction": False, "hooks": {
    "SessionStart": [dict(matcher="compact", **hook(f"python3 -I {here}/slice_hook.py {k} {slots} {size}")) for k in range(1, slots + 1)],
    "PostCompact": [hook(f"cat > {d}/postcompact.json")]}}
claude = os.environ.get("CLAUDE_BIN", str(Path.home() / ".local/bin/claude"))
cmd = [claude, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
       "--model", model, "--setting-sources", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
       "--settings", json.dumps(settings), "--dangerously-skip-permissions"]
p = subprocess.Popen(cmd, cwd=d, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
session = None
def turn(text):
    global session
    p.stdin.write(json.dumps({"type": "user", "message": {"role": "user", "content": text}}) + "\n"); p.stdin.flush()
    while line := p.stdout.readline():
        ev = json.loads(line); session = ev.get("session_id") or session
        if ev.get("subtype") == "compact_boundary":
            print("compact_boundary", json.dumps(ev.get("compact_metadata", {}))[:200])
        if ev.get("type") == "result":
            return ev.get("result") or ""
turn("Remember codeword KAPPA-2024. Reply OK.")
turn("/compact Keep the codeword. Write the literal line PROBE-INSTRUCTIONS-KEPT in the summary.")
answer = turn("Without tools: for each block headed '[Brigadier · decisions restored · part k of N]', "
              "give k and the exact final line of that block (a line starting SLICE-). One per line.")
p.stdin.close(); p.wait(timeout=120)
print(answer)
ends = {k: f"SLICE-{k}-END" in answer for k in range(1, slots + 1)}
print("model quoted every slice end:", all(ends.values()), ends)
slug = "-" + str(d).strip("/").replace("/", "-").replace(".", "-")
transcript = next((Path.home() / ".claude/projects").glob(f"{slug}/{session}.jsonl"))
keep = []
for line in transcript.read_text().splitlines():
    e = json.loads(line)
    a = e.get("attachment") or {}
    if e.get("subtype") == "compact_boundary" or a.get("type") == "hook_additional_context" or e.get("isCompactSummary"):
        keep.append({k: e[k] for k in ("type", "subtype", "uuid", "attachment", "compactMetadata", "isCompactSummary", "message") if k in e})
for e in keep:
    for c in (e.get("attachment") or {}).get("content", []):
        print("recorded slice", len(c), c[:48].replace("\n", " "), "| whole:", c.endswith("-END"))
(d / "fixture.jsonl").write_text("".join(json.dumps(e) + "\n" for e in keep))
print("transcript", transcript)
